# LCR Code — project automation.
#
# Two layers, split by whether a task needs the project's own types:
#
#   * Recipes here orchestrate shells — builds, size gates, artifact generation.
#   * `xtask` holds the tasks that must call into the crates. `capacity` recomputes the
#     §6.1 table from `lcr_core::capacity` rather than restating it, and `ldpc` generates
#     the §8.2 base graphs, which the spec forbids building at runtime. No command runner
#     can do either.
#
# The recipes below wrap xtask so `just --list` is the single front door.

set shell := ["bash", "-uc"]

# §14.2's budget for the decode-only component, gated on **two** bases (open decision #16).
#
# §14.2 says "< 200 KB gzipped" and marks it [TBD: validate]. That single number admits two
# readings, and which one you take changes the answer by 2.5×:
#
#   * **Raw** — what a host loads from disk and holds in memory. D8 made the artifact a
#     Component Model component rather than a browser bundle, so this is the figure for an
#     embedded host.
#   * **Compressed** — what crosses a network. §17.3's whole argument for the budget is "a
#     web app or POS vendor", which is a transfer-size case, and FIREFOX.md documents
#     exactly that path.
#
# Gating on only one was hiding the other: at 199,453 bytes the scanner read 97% of budget
# raw and 38% gzipped. Both now have a limit and both are reported.
#
# The raw limit started as §14.2's 200 KB taken literally, and was **raised to 250 KB** on
# 2026-10-05 for D23/D24: ring geometry that survives damaged corners and lens distortion,
# and decision-feedback equalisation against blur, which together took the scanner past
# 200 KB. **The compressed limit is a proposal**, not a measurement: half the raw budget, on
# the grounds that a gate that never fires is decoration, and it moved with the raw one. It
# wants justifying against a real embedder's constraint before #16 can actually close.
scanner_budget_raw := "256000"
scanner_budget_gz := "128000"

wasm_target := "wasm32-unknown-unknown"
wasm_out := "target/wasm32-unknown-unknown/release/lcr_wasm.wasm"

_default:
    @just --list --unsorted

# ---------------------------------------------------------------- check

# Everything CI should run.
#
# `clippy` sits second because it is the cheapest thing that fails: it compiles in the dev
# profile while `test` needs release, so a lint error surfaces in seconds rather than after
# the full suite. It was missing until 2026-09-08, and six lints had accumulated in
# untouched code from toolchain drift while this file claimed zero warnings — a gate nobody
# runs is not a gate.
ci: fmt-check clippy test test-debug wasm-size check-compare ffi-header-check ffi-test

# What `.githooks/pre-push` runs: the checks that fail most, at a cost you will sit
# through before every push.
#
# A subset of `ci`, deliberately. The steps left out — the debug-assertion pass, the wasm
# size gate, the compare component, the C header and C smoke test — are slow and rarely
# the thing that broke, and GitHub Actions runs every one of them on each push and pull
# request. Make `just ci` a required check on `main` and nothing unchecked can merge.
[doc("The fast gate the pre-push hook runs: fmt-check, clippy, test.")]
pre-push: fmt-check clippy test

# Type-check the comparison component.
#
# `test` exercises the `compare` feature on the *host*, and `wasm-size` builds only the two
# shipped artifacts — so nothing was compiling `component.rs` under `compare`, and moving QR
# into `lcr-render` broke that binding silently. It surfaced only when `just web` was run,
# which is the worst place to find it: mid-setup for a capture session.
#
# `check` rather than a full build, so the gate stays cheap.
[doc("Type-check the QR comparison component, which no other gate builds.")]
check-compare:
    cargo check -p lcr-wasm --target {{wasm_target}} --features compare

# Dependency policy from `deny.toml`: licences, RustSec advisories, sources.
#
# Not part of `ci`, deliberately. The advisory check fetches the RustSec database, so it
# needs the network and can start failing with no change here when a new advisory lands —
# the right behaviour for a CI job, the wrong one for a pre-push hook. CI runs it in
# `.github/workflows/deny.yml`, on every push and daily.
[doc("Check dependency licences and security advisories (needs cargo-deny).")]
deny:
    cargo deny check

# Depends on `component` because `lcr-wasm/tests/component.rs` loads the built artifact
# from dist/ and runs it under wasmtime. Without that ordering the component tests would
# either fail confusingly or, worse, pass against a stale build.

# The whole test suite, including the component under wasmtime.
test: component
    cargo test --workspace --profile release-tests
    # `qr` and `compare` are off by default so no shipped artifact carries QR, which also
    # means the default run never builds their tests. Run them explicitly: the QR round
    # trip against an independent decoder is the only thing standing behind both the
    # density comparison and §17.2 dual-render.
    cargo test -p lcr-render --profile release-tests --features qr qr::
    cargo test -p lcr-wasm --profile release-tests --features compare

# Exercise `debug_assert!`, which the release suite above compiles out.
#
# This is not redundant with `test`. Release builds strip every `debug_assert!`, so a
# release-only suite cannot see them — and one of them was failing unnoticed: the §4.2
# reachability check in `lcr-core/src/palette.rs` had been red since the palettes were
# pinned, because `just test` is the only command anyone runs and it builds optimised.
#
# `--lib` rather than the whole suite: the assertions sit in crate internals that the
# unit tests reach directly, while the integration suites are dominated by the §15
# degradation sweep — ~5 minutes unoptimised, for no additional assertion coverage.
[doc("Run the unit tests unoptimised, so debug_assert! is live.")]
test-debug:
    cargo test --workspace --lib

# Just the native tests, skipping the component build. Faster inner loop.
test-fast:
    cargo test --workspace --profile release-tests --exclude lcr-wasm
    cargo test -p lcr-wasm --profile release-tests --test surface

# Build everything, including examples and tests, and fail on any warning.
build:
    cargo build --workspace --release --examples --tests

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# ---------------------------------------------------------------- component

# Build both wasm components: the full build and the decode-only scanner.
component: _component-full _component-scanner
    @ls -l dist/*.wasm | awk '{printf "  %-32s %8d bytes\n", $9, $5}'

_dist:
    @mkdir -p dist

# Binaryen's size pass, run on the **core module** before `wasm-tools component new`:
# binaryen does not read component-model binaries, so optimising the component fails.
#
# Optional by design. Without binaryen the build still works and prints a note; with it, a
# failure is a hard failure rather than a silently unoptimised artifact.
#
# `-Oz` here is not D15's declined `opt-level = "z"`. That changed rustc's codegen and cost
# 11% median decode time; this is a post-pass over the same opt-level 3 output, and what it
# mainly removes is code the linker retained but nothing reaches — 11 KB of float formatting
# kept alive by a function-table entry with no live caller (D17).
# Exactly the features rustc's `wasm32-unknown-unknown` output uses — **not** `-all`.
# `-all` let binaryen optimise into instructions wasmtime rejects by default, and the
# component then failed to parse at load: `wasm-tools validate --features=component-model`
# passed it, so the gate that caught it was `cargo test -p lcr-wasm`, which actually
# instantiates the shipped artifact.
wasm_opt_flags := "-Oz --enable-bulk-memory --enable-sign-ext --enable-mutable-globals --enable-nontrapping-float-to-int"

# Shipped artifacts must not carry the builder's filesystem. Panic locations embed source
# paths, and for the standard library and every crate from the registry those are absolute
# paths under the home directory — the scanner shipped `/Users/<name>/.rustup/…` and
# `/Users/<name>/.cargo/registry/…` until this (D27). Remapping the home directory to `~`
# covers the toolchain, the registry and this repository at once.
wasm_rustflags := "--remap-path-prefix=" + env_var("HOME") + "=~"

# Decode + encode.
_component-full: _dist
    RUSTFLAGS="{{wasm_rustflags}}" cargo build -p lcr-wasm --release --target {{wasm_target}}
    cp {{wasm_out}} target/full-core.wasm
    @if command -v wasm-opt >/dev/null; then wasm-opt {{wasm_opt_flags}} -o target/full-core.opt.wasm target/full-core.wasm && mv target/full-core.opt.wasm target/full-core.wasm; else echo "  wasm-opt not found — skipping the size pass (brew install binaryen)"; fi
    wasm-tools component new target/full-core.wasm -o dist/lcr-code.wasm
    wasm-tools strip --all dist/lcr-code.wasm -o dist/lcr-code.wasm
    wasm-tools validate --features=component-model dist/lcr-code.wasm

# Decode only. §14.1 keeps the encoder out so a scanner cannot acquire it by accident.
_component-scanner: _dist
    RUSTFLAGS="{{wasm_rustflags}}" cargo build -p lcr-wasm --release --target {{wasm_target}} \
        --no-default-features --features decode
    cp {{wasm_out}} target/scanner-core.wasm
    @if command -v wasm-opt >/dev/null; then wasm-opt {{wasm_opt_flags}} -o target/scanner-core.opt.wasm target/scanner-core.wasm && mv target/scanner-core.opt.wasm target/scanner-core.wasm; else echo "  wasm-opt not found — skipping the size pass (brew install binaryen)"; fi
    wasm-tools component new target/scanner-core.wasm -o dist/lcr-code-scanner.wasm
    # `--all` rather than plain `strip`: the default leaves the `name` section, which on
    # the scanner is 19 KB — 10% of the artifact — of debug symbols no shipped decoder
    # needs. Plain `strip` saved 1,753 bytes; `--all` saves 20,733. The core module at
    # {{wasm_out}} keeps its names, so a trap is still debuggable there.
    wasm-tools strip --all dist/lcr-code-scanner.wasm -o dist/lcr-code-scanner.wasm
    wasm-tools validate --features=component-model dist/lcr-code-scanner.wasm

# Decode + encode + QR, for the `display.html` density comparison.
#
# Deliberately not part of `component`, so neither shipped artifact carries a competing
# symbology. Only `just web` builds this, and it is the only artifact with QR in it.
_component-compare: _dist
    RUSTFLAGS="{{wasm_rustflags}}" cargo build -p lcr-wasm --release --target {{wasm_target}} --features compare
    wasm-tools component new {{wasm_out}} -o dist/lcr-code-compare.wasm
    wasm-tools strip --all dist/lcr-code-compare.wasm -o dist/lcr-code-compare.wasm
    wasm-tools validate --features=component-model dist/lcr-code-compare.wasm

# Print the WIT interface each component actually exports.
component-wit: component
    @echo "=== full"
    @wasm-tools component wit dist/lcr-code.wasm | grep -E '^\s*(import|export)'
    @echo "=== scanner"
    @wasm-tools component wit dist/lcr-code-scanner.wasm | grep -E '^\s*(import|export)'

# Gate the scanner component against §14.2's size budget (open decision #16).
#
# zstd rather than gzip for the transfer figure: ~10% smaller on these artifacts, and the
# format anyone shipping a component today would reach for.
[doc("Gate the scanner component against §14.2's size budget.")]
wasm-size: component
    #!/usr/bin/env bash
    set -euo pipefail
    # gzip as well as zstd: §14.2 says "gzipped", so that is the figure the compressed
    # budget is stated against. zstd stays because it is ~10% smaller on these artifacts
    # and is what anyone shipping a component today would actually reach for.
    for f in dist/lcr-code.wasm dist/lcr-code-scanner.wasm; do
        raw=$(wc -c < "$f")
        gz=$(gzip -9 -c "$f" | wc -c)
        zs=$(zstd -19 -c "$f" 2>/dev/null | wc -c)
        printf "  %-28s raw %7d  gzip %7d  zstd %7d\n" "$(basename $f)" "$raw" "$gz" "$zs"
    done
    echo
    raw=$(wc -c < dist/lcr-code-scanner.wasm)
    gz=$(gzip -9 -c dist/lcr-code-scanner.wasm | wc -c)
    praw=$(( raw * 100 / {{scanner_budget_raw}} ))
    pgz=$(( gz * 100 / {{scanner_budget_gz}} ))
    printf "  scanner raw   %7d / %7d  %3d%%  (embedded host, D8)\n" \
        "$raw" "{{scanner_budget_raw}}" "$praw"
    printf "  scanner gzip  %7d / %7d  %3d%%  (web delivery, §17.3)\n" \
        "$gz" "{{scanner_budget_gz}}" "$pgz"
    over=0
    if [ "$raw" -gt "{{scanner_budget_raw}}" ]; then
        echo "  OVER RAW BUDGET" >&2
        over=1
    fi
    if [ "$gz" -gt "{{scanner_budget_gz}}" ]; then
        echo "  OVER COMPRESSED BUDGET" >&2
        over=1
    fi
    exit $over

# Pinned rather than `@latest`, so CI and a laptop transpile the same bindings and a jco
# release cannot change the deployed demo without a commit. To move it: bump, `just web`,
# check every page still loads.
jco := "@bytecodealliance/jco@1.37.0"

# Transpile the comparison component for the browser, into web/lcr, plus the sample frame.
#
# `jco` turns the component into a core module plus JS bindings for the Component Model
# ABI. Shared by `web`, `tunnel` and `site`, so the three cannot drift apart.
#
# Node comes from Homebrew explicitly: nvm shadows it with a v14 that cannot run jco. The
# prefix is harmless where /opt/homebrew does not exist, which is how CI gets its own node.
[doc("Build the browser demo into web/: jco bindings plus the sample frame.")]
web-build:
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH=/opt/homebrew/bin:$PATH
    just component _component-compare
    # The compare component, not the scanner: `display.html` needs `encoder` and now
    # `compare` for the QR side-by-side, and `scan.html` needs `codec.cbor-to-json` to
    # show a CBOR body as JSON. A shipping scanner would take `lcr-code-scanner.wasm` and
    # render host-side instead — and would carry no QR, which is why the comparison build
    # is a third artifact rather than a change to the other two.
    # `--name lcr-code` so the emitted module keeps its filename regardless of which
    # artifact fed it; all three pages import `./lcr/lcr-code.js` and should not have to care.
    npx --yes {{jco}} transpile dist/lcr-code-compare.wasm \
        --name lcr-code -o web/lcr --no-typescript
    cargo run -p lcr-decode --release --example frame -- web/frame.rgba

# Assemble the GitHub Pages site into target/site: the explainer as the front page, and
# the demo pages beside it.
#
# The explainer is written in the repository as `docs/explainer.html`, beside `docs/img/`,
# so it previews correctly from a checkout. Here it becomes `index.html` at the site root:
# `docs/img/` moves up with it to `img/`, and its one `../examples/` figure is rewritten to
# `examples/`. Its links to the demo pages are root-relative already (`scan.html`, …), so
# they only resolve here — which is the point: the site is where the demos run.
[doc("Assemble the GitHub Pages site (explainer + demos) into target/site.")]
site: web-build
    rm -rf target/site
    mkdir -p target/site/examples
    cp -R web/. target/site/
    sed 's#"\.\./examples/#"examples/#g' docs/explainer.html > target/site/index.html
    cp -R docs/img target/site/
    cp examples/anatomy-c3.png target/site/examples/
    find target/site -name .DS_Store -delete
    @echo "  wrote target/site — serve with: python3 -m http.server -d target/site"

# Build the browser demo and serve it.
#
# Served from localhost because a camera needs a secure context — a plain LAN address
# loads the page but the browser refuses `getUserMedia`.
[doc("Build the browser demo and serve it on localhost.")]
web port="8000":
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH=/opt/homebrew/bin:$PATH
    just site
    echo
    echo "  explainer: http://localhost:{{port}}/                (start here)"
    echo "  display:   http://localhost:{{port}}/display.html    (laptop)"
    echo "  transfer:  http://localhost:{{port}}/transfer.html   (laptop — file drop)"
    echo "  scan:      http://localhost:{{port}}/scan.html       (phone — needs HTTPS, see \`just tunnel\`)"
    echo "  receive:   http://localhost:{{port}}/receive.html    (phone — needs HTTPS)"
    echo "  sweep:     http://localhost:{{port}}/sweep.html      (phone — records trials)"
    echo "  ^C to stop"
    cd target/site && python3 -m http.server {{port}}

# Serve the demo over an HTTPS tunnel, for the file-transfer pages.
#
# A camera needs a secure context, so `just web` tells the phone to use localhost — which
# a phone does not have. `cloudflared` gives the laptop's server a public HTTPS name for
# the length of the session, which is the shortest path to a phone actually decoding.
#
# Nothing is uploaded to the tunnel but the page itself: encode and decode both run in the
# browser, and the file never leaves the two devices.
[doc("Serve the demo over an HTTPS cloudflared tunnel, so a phone can use its camera.")]
tunnel port="8000":
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH=/opt/homebrew/bin:$PATH
    command -v cloudflared >/dev/null || { echo "cloudflared not found (brew install cloudflared)" >&2; exit 1; }
    just site
    ( cd target/site && python3 -m http.server {{port}} >/dev/null 2>&1 ) &
    server=$!
    trap 'kill $server 2>/dev/null || true' EXIT
    echo
    echo "  starting tunnel — the https URL appears below"
    echo "  sender:   <url>/transfer.html   (laptop)"
    echo "  receiver: <url>/receive.html    (phone)"
    echo
    cloudflared tunnel --url http://localhost:{{port}}

# Check the WIT interface parses and round-trips.
wit-check:
    wasm-tools component wit crates/lcr-wasm/wit/lcr.wit > /dev/null && echo "  wit ok"

# ---------------------------------------------------------------- hosts

# Build and run the C host trial against the built component.
#
# Proves §17.3's "embedding is cheap" claim from a non-Rust host. Needs the wasmtime C
# API; `WASMTIME_C_API` may point at an unpacked release, otherwise this falls back to
# the copy bundled in the wasmtime-go module cache — which is there because that module
# ships a component-capable C library its own Go bindings do not expose (see D9).
[doc("Build and run the C host trial against the built component.")]
c-host: component
    #!/usr/bin/env bash
    set -euo pipefail
    root="${WASMTIME_C_API:-}"
    if [ -z "$root" ]; then
        root=$(ls -d "$(go env GOMODCACHE 2>/dev/null)"/github.com/bytecodealliance/wasmtime-go/v*@* 2>/dev/null | tail -1)/build
    fi
    if [ ! -d "$root/include" ]; then
        echo "wasmtime C API not found. Set WASMTIME_C_API to an unpacked" >&2
        echo "wasmtime-c-api release, or 'go get github.com/bytecodealliance/wasmtime-go/v34'." >&2
        exit 1
    fi
    case "$(uname -s)-$(uname -m)" in
        Darwin-arm64) plat=macos-aarch64;   libs="-framework Security -framework CoreFoundation" ;;
        Darwin-x86_64) plat=macos-x86_64;   libs="-framework Security -framework CoreFoundation" ;;
        Linux-aarch64) plat=linux-aarch64;  libs="-lpthread -ldl -lm" ;;
        Linux-x86_64) plat=linux-x86_64;    libs="-lpthread -ldl -lm" ;;
        *) echo "unsupported platform $(uname -s)-$(uname -m)" >&2; exit 1 ;;
    esac
    mkdir -p target/c-host
    cc -std=c11 -O2 -Wall -I"$root/include" examples/c-host/main.c \
        "$root/$plat/libwasmtime.a" $libs -o target/c-host/lcr-host
    ./target/c-host/lcr-host

# ---------------------------------------------------------------- C API (lcr-ffi)

ffi_header_args := "--config crates/lcr-ffi/cbindgen.toml --crate lcr-ffi --output crates/lcr-ffi/include/lcr.h crates/lcr-ffi"

# Regenerate the committed C header from crates/lcr-ffi/src/lib.rs.
#
# Committed rather than generated at build time, so a C user gets a header from a checkout
# with no Rust tooling — and generated by `cbindgen` the CLI rather than `cbindgen` the
# library in a build script, which would put an MPL-2.0 crate and its parser into every
# build of the library for the sake of a file that changes only when the API does.
[doc("Regenerate crates/lcr-ffi/include/lcr.h from the Rust source (needs cbindgen).")]
ffi-header:
    cbindgen {{ffi_header_args}}

# In `ci`: a header that has drifted from the Rust signatures still compiles on both
# sides, and then corrupts memory at the boundary.
[doc("Fail if the committed C header is stale (needs cbindgen).")]
ffi-header-check:
    @cbindgen --verify {{ffi_header_args}} 2>/dev/null \
        || { echo "crates/lcr-ffi/include/lcr.h is stale: run 'just ffi-header'" >&2; exit 1; }

# Build the static library, compile tests/smoke.c against the committed header with
# warnings as errors, link, and run it.
#
# The link flags come from rustc (`--print native-static-libs`) rather than a table here,
# because they are whatever the standard library needs on the host — which changes with
# the platform and occasionally with the toolchain.
[doc("Compile and run the C smoke test against the static library and header.")]
ffi-test:
    #!/usr/bin/env bash
    set -euo pipefail
    # `--color never`: CI sets CARGO_TERM_COLOR=always, and a colour reset appended to the
    # last flag (`-lc\e[0m`) is a library no linker can find.
    libs=$(cargo rustc -p lcr-ffi --profile release-tests --color never --lib --crate-type staticlib \
        -- --print native-static-libs 2>&1 \
        | sed -n 's/.*native-static-libs: //p' | tail -1)
    # macOS: the driver links libSystem itself, and naming it again only draws an ld warning.
    libs=${libs//-lSystem/}
    mkdir -p target/ffi
    cc -std=c11 -O2 -Wall -Wextra -Werror -pedantic -Icrates/lcr-ffi/include \
        crates/lcr-ffi/tests/smoke.c target/release-tests/liblcr_ffi.a $libs -o target/ffi/smoke
    ./target/ffi/smoke

# ---------------------------------------------------------------- artifacts

# Regenerate examples/ — a symbol per payload, each decoded before it is written.
gallery:
    cargo run -p lcr-render --release --example gallery -- examples

# Regenerate examples/anatomy-c3.png — a C3 symbol with every part boxed and explained.
#
# The example writes an HTML page whose boxes are computed from lcr-core's geometry, and
# headless Chrome turns it into the PNG, because nothing in the workspace renders text.
# The window size must match `PAGE` in `crates/lcr-render/examples/anatomy.rs`. Chrome's
# macOS path by default; elsewhere, `just chrome=/path/to/chrome anatomy`.
chrome := "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

[doc("Regenerate examples/anatomy-c3.png, the annotated C3 reference figure.")]
anatomy:
    cargo run -p lcr-render --release --example anatomy -- target/anatomy
    "{{chrome}}" --headless=new --disable-gpu --hide-scrollbars --window-size=1900,1250 \
        --screenshot="$PWD/examples/anatomy-c3.png" "file://$PWD/target/anatomy/anatomy-c3.html" \
        2>/dev/null
    @echo "  wrote examples/anatomy-c3.png"

# Regenerate examples/anatomy-expanded-c3.png — every cell coloured by the job it does.
#
# The companion to `anatomy`: that figure points at features, this one colours the whole
# symbol, including where each layer's information and parity bits physically land. Same
# Chrome dependency, same window-size-must-match-PAGE rule.
[doc("Regenerate examples/anatomy-expanded-c3.png, the whole-symbol anatomy.")]
anatomy-expanded:
    cargo run -p lcr-render --release --example anatomy_expanded -- target/anatomy
    "{{chrome}}" --headless=new --disable-gpu --hide-scrollbars --window-size=1340,1700 \
        --screenshot="$PWD/examples/anatomy-expanded-c3.png" \
        "file://$PWD/target/anatomy/anatomy-expanded-c3.html" 2>/dev/null
    @echo "  wrote examples/anatomy-expanded-c3.png"

# Regenerate examples/anatomy-ldpc-c3.png — the luma layer's LDPC code, followed through a
# real decode of a defocused, noisy capture.
#
# The third anatomy figure: one real parity check, the decoder's own confidences, and the
# decoder stopped after each iteration until every check is satisfied. Same Chrome
# dependency, same window-size-must-match-PAGE rule.
[doc("Regenerate examples/anatomy-ldpc-c3.png, the error-correction explainer.")]
anatomy-ldpc:
    cargo run -p lcr-render --release --example anatomy_ldpc -- target/anatomy
    "{{chrome}}" --headless=new --disable-gpu --hide-scrollbars --window-size=1340,3000 \
        --screenshot="$PWD/examples/anatomy-ldpc-c3.png" \
        "file://$PWD/target/anatomy/anatomy-ldpc-c3.html" 2>/dev/null
    @echo "  wrote examples/anatomy-ldpc-c3.png"

# Regenerate docs/img/ — the figures docs/explainer.html reads.
#
# Every figure is rendered by the encoder and damaged by the §15 harness at the same
# settings the explainer's quoted success rates were measured on, so a figure and its
# caption cannot drift apart. The page links these rather than embedding them, so it
# stays a 46 KB diff instead of a 2.7 MB one.
[doc("Regenerate the figures used by docs/explainer.html.")]
figures:
    cargo run -p lcr-decode --release --example explain

# What JSON-shaped payloads cost, end to end.
payload-sizes:
    cargo run -p lcr-decode --release --example json

# Sweep each degradation to find where decoding stops working.
#
# Reports success *rate* per cell, which is what §15 asks for and what comparing two
# builds requires. Optional argument is the trial count (default 16); the standard error
# is 0.5/sqrt(n), so pass 64 or more when a difference matters.
[doc("Sweep each degradation for success rate vs magnitude (§15).")]
envelope trials="":
    cargo run -p lcr-decode --release --example envelope -- {{trials}}

# Measure where each layer fails, to calibrate the §8.2 rate split (open decision #5).
#
# Second argument is the profile — `just tune 12 C3`. Worth running per profile: chroma is
# 11% of C2's payload and 60% of C3's, so the same rate table lands very differently.
[doc("Measure where each layer fails, per profile, for the §8.2 rate split.")]
tune trials="" profile="":
    cargo run -p lcr-decode --release --example tune -- {{trials}} {{profile}}

# One symbol size, each profile loaded to its own capacity — the comparison that says
# which profile to reach for. This is what established D11.
[doc("Compare the colour profiles head to head at one symbol size.")]
profiles trials="":
    cargo run -p lcr-decode --release --example profiles -- {{trials}}

# Print detection internals for a degraded capture.
diagnose:
    cargo run -p lcr-decode --release --example diagnose

# ---------------------------------------------------------------- codegen (xtask)

# Regenerate the §8.2 LDPC base graphs. §8.2 forbids generating these at runtime.
ldpc:
    cargo run --release -p xtask -- ldpc > crates/lcr-core/src/ecc/tables.rs
    cargo fmt --all

# Recompute the §6.1 capacity table from the real layout (open decision #8).
capacity:
    cargo run --release -p xtask -- capacity

# Regenerate docs/ldpc.md — AWGN decoding thresholds per rate (§14.5 step 5).
#
# Slow (~4 min): it is a Monte-Carlo waterfall, 240 frames at each of 16 SNR points for
# six rates at three layer lengths. Deliberately not in `ci` for that reason — the output
# is committed, and it only changes when the base graphs do.
[doc("Regenerate docs/ldpc.md — AWGN decoding thresholds per rate.")]
thresholds:
    cargo run --release -p xtask -- thresholds > docs/ldpc.md
    @echo "  wrote docs/ldpc.md"

# Fuzz an untrusted-input entry point. Targets: decode_rgb, cbor, json, frame.
#
# Needs nightly and cargo-fuzz. The corpus is seeded from the golden vectors and a real
# capture, so the fuzzer starts from valid symbols rather than from noise — which matters
# most for `decode_rgb`, where a random buffer is rejected long before it reaches anything
# interesting.
#
# A crash worth keeping becomes a unit test in the crate it belongs to, not a file here.
[doc("Fuzz an untrusted-input entry point (decode_rgb / cbor / json / frame).")]
fuzz target="decode_rgb" secs="120":
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p fuzz/corpus/{{target}}
    python3 - <<'SEED'
    import json, pathlib, struct
    d = json.load(open('vectors/golden-v1.json'))
    t = "{{target}}"
    out = pathlib.Path(f"fuzz/corpus/{t}")
    if t == "frame":
        for v in d['vectors']:
            if 'modules' in v:
                (out / f"{v['name']}.bin").write_bytes(bytes.fromhex(v['modules']))
    elif t == "json":
        for v in d['vectors']:
            if v['input'] == 'json':
                (out / f"{v['name']}.json").write_text(v['payload'])
    elif t == "decode_rgb":
        p = pathlib.Path('web/frame.rgba')
        if p.exists():
            raw = p.read_bytes()
            w, h = struct.unpack_from('<II', raw, 0)
            hdr = struct.pack('<HHH', w, h, w * 4) + bytes([1])
            (out / 'real-symbol.bin').write_bytes(hdr + raw[8:])
    SEED
    cd fuzz && cargo +nightly fuzz run {{target}} -- -max_total_time={{secs}} -print_final_stats=1

# Regenerate the wire-format freeze, vectors/golden-v1.json (§15).
#
# Only run this when a wire-format change is *intended*. `golden_vectors_still_match`
# fails when the encoder's output moves, and the point is to notice; regenerating to make
# a red test go green throws away the signal. The diff is the record of what changed.
[doc("Regenerate the golden vectors. Only when a wire-format change is intended.")]
vectors:
    cargo run --release -p xtask -- vectors > vectors/golden-v1.json
    @echo "  wrote vectors/golden-v1.json — review the diff"

# Regenerate docs/border-sequence.md — position sequences for the §6.4 border.
#
# Groundwork for reclaiming the finder area: a border whose every k-cell window is unique
# is an absolute position reference, so it can carry orientation, sizing and the homography
# correspondences that three 7x7 finders currently pay 192 modules for. Nothing consumes
# these tables yet — this generates them and proves their properties first.
#
# Slow (~1 min): 32 backtracking searches plus a randomised arc-localisation measurement.
# On-demand, like `thresholds`, and not part of `ci`.
[doc("Regenerate docs/border-sequence.md — §6.4 border position sequences.")]
debruijn:
    cargo run --release -p xtask -- debruijn > docs/border-sequence.md
    @echo "  wrote docs/border-sequence.md"

# Regenerate the committed sequence table.
#
# §8.2's rule for the LDPC graphs applies here for the same reason: the construction is a
# backtracking search with a budget and has no business inside a decoder. Generated
# offline, committed, and re-checked in the crate that ships it by
# `every_sequence_window_is_unique` — because the generator is not what a decoder runs
# against, the table is.
[doc("Regenerate the committed §6.4 border sequence table.")]
debruijn-table:
    cargo run --release -p xtask -- debruijn --table > crates/lcr-core/src/border_seq.rs
    @echo "  wrote crates/lcr-core/src/border_seq.rs — review the diff"

# Reduce a real-camera capture log to §15's success-rate metric.
#
# The log comes from `web/sweep.html` (served by `just web`), which records one line per
# decode attempt against the geometry it was attempted at. This is the only measurement in
# the project that is not self-referential: every other figure comes from the §15 harness,
# which is code. See docs/capture-sweep.md for how to run one.
#
# `log` is a path to a fresh log, or the name of one already archived in
# docs/data/capture-sweeps.tar.zst (`just sweep sweep2.json`); with no argument this lists
# the archive. The archive is a ZSTA file written by zalt, but it is an ordinary .tar.zst,
# so it is read here with stock `zstd` and `tar` — reading it needs nothing zalt-specific.
[doc("Reduce a capture-sweep log from web/sweep.html to success rate vs geometry.")]
sweep log="":
    #!/usr/bin/env bash
    set -euo pipefail
    archive=docs/data/capture-sweeps.tar.zst
    log="{{log}}"
    if [ -z "$log" ]; then
        echo "usage: just sweep <log.json | archived name>; archived sweeps:"
        zstd -dc "$archive" | tar -tf - | sed 's/^/  /'
        exit 0
    fi
    if [ ! -f "$log" ]; then
        mkdir -p target/sweeps
        zstd -dc "$archive" | tar -xf - -C target/sweeps "$log"
        log="target/sweeps/$log"
    fi
    cargo run --release -q -p xtask -- sweep "$log"

# Search for the §4 palette constants against the sRGB gamut (open decisions #1 and #2).
#
# Reports the best configuration per profile, what the §4.2 luma gap costs in chroma, and
# what each hue orientation is worth — the last so D1's green-against-magenta choice can
# be priced against the alternatives rather than assumed.
[doc("Search the §4 palette constants against the sRGB gamut.")]
palette:
    cargo run --release -p xtask -- palette

# ---------------------------------------------------------------- git

# Point git at the tracked hooks. Run once per clone.
hooks:
    git config core.hooksPath .githooks
    @echo '  hooks installed — .githooks/pre-push runs: just pre-push (CI runs: just ci)'

# ---------------------------------------------------------------- housekeeping

clean:
    cargo clean
    rm -rf dist
