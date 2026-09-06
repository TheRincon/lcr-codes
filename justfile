# LC Code — project automation.
#
# Two layers, split by whether a task needs the project's own types:
#
#   * Recipes here orchestrate shells — builds, size gates, artifact generation.
#   * `xtask` holds the tasks that must call into the crates. `capacity` recomputes the
#     §6.1 table from `lc_core::capacity` rather than restating it, and `ldpc` generates
#     the §8.2 base graphs, which the spec forbids building at runtime. No command runner
#     can do either.
#
# The recipes below wrap xtask so `just --list` is the single front door.

set shell := ["bash", "-uc"]

# §14.2's budget for the decode-only component: 200 KB.
#
# Measured **raw**, not compressed. The spec says "gzipped" because it assumed a browser
# bundle fetched over a network (D8); a component is loaded from disk by a host, so the
# uncompressed size is the one that has to fit. The difference is not cosmetic — the
# scanner is 93% of budget raw against 33% zstd-compressed, so the compressed figure would
# report comfort that does not exist.
scanner_budget := "204800"

wasm_target := "wasm32-unknown-unknown"
wasm_out := "target/wasm32-unknown-unknown/release/lc_wasm.wasm"

_default:
    @just --list --unsorted

# ---------------------------------------------------------------- check

# Everything CI should run.
ci: fmt-check test test-debug wasm-size

# Depends on `component` because `lc-wasm/tests/component.rs` loads the built artifact
# from dist/ and runs it under wasmtime. Without that ordering the component tests would
# either fail confusingly or, worse, pass against a stale build.

# The whole test suite, including the component under wasmtime.
test: component
    cargo test --workspace --release

# Exercise `debug_assert!`, which the release suite above compiles out.
#
# This is not redundant with `test`. Release builds strip every `debug_assert!`, so a
# release-only suite cannot see them — and one of them was failing unnoticed: the §4.2
# reachability check in `lc-core/src/palette.rs` had been red since the palettes were
# pinned, because `just test` is the only command anyone runs and it passes `--release`.
#
# `--lib` rather than the whole suite: the assertions sit in crate internals that the
# unit tests reach directly, while the integration suites are dominated by the §15
# degradation sweep — ~5 minutes unoptimised, for no additional assertion coverage.
[doc("Run the unit tests unoptimised, so debug_assert! is live.")]
test-debug:
    cargo test --workspace --lib

# Just the native tests, skipping the component build. Faster inner loop.
test-fast:
    cargo test --workspace --release --exclude lc-wasm
    cargo test -p lc-wasm --release --test surface

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

# Decode + encode.
_component-full: _dist
    cargo build -p lc-wasm --release --target {{wasm_target}}
    wasm-tools component new {{wasm_out}} -o dist/lc-code.wasm
    wasm-tools validate --features=component-model dist/lc-code.wasm

# Decode only. §14.1 keeps the encoder out so a scanner cannot acquire it by accident.
_component-scanner: _dist
    cargo build -p lc-wasm --release --target {{wasm_target}} \
        --no-default-features --features decode
    wasm-tools component new {{wasm_out}} -o dist/lc-code-scanner.wasm
    wasm-tools validate --features=component-model dist/lc-code-scanner.wasm

# Print the WIT interface each component actually exports.
component-wit: component
    @echo "=== full"
    @wasm-tools component wit dist/lc-code.wasm | grep -E '^\s*(import|export)'
    @echo "=== scanner"
    @wasm-tools component wit dist/lc-code-scanner.wasm | grep -E '^\s*(import|export)'

# Gate the scanner component against §14.2's size budget (open decision #16).
#
# zstd rather than gzip for the transfer figure: ~10% smaller on these artifacts, and the
# format anyone shipping a component today would reach for.
[doc("Gate the scanner component against §14.2's size budget.")]
wasm-size: component
    #!/usr/bin/env bash
    set -euo pipefail
    for f in dist/lc-code.wasm dist/lc-code-scanner.wasm; do
        raw=$(wc -c < "$f")
        zs=$(zstd -19 -c "$f" 2>/dev/null | wc -c)
        printf "  %-28s raw %7d  zstd %7d\n" "$(basename $f)" "$raw" "$zs"
    done
    raw=$(wc -c < dist/lc-code-scanner.wasm)
    pct=$(( raw * 100 / {{scanner_budget}} ))
    echo "  scanner is ${pct}% of the §14.2 budget ({{scanner_budget}} bytes, raw)"
    if [ "$raw" -gt "{{scanner_budget}}" ]; then
        echo "  OVER BUDGET" >&2
        exit 1
    fi

# Transpile the scanner component for the browser and serve the demo.
#
# `jco` turns the component into a core module plus JS bindings for the Component Model
# ABI. Served from localhost because a camera needs a secure context — a plain LAN address
# loads the page but the browser refuses `getUserMedia`.
#
# Node comes from Homebrew explicitly: nvm shadows it with a v14 that cannot run jco.
web port="8000":
    #!/usr/bin/env bash
    set -euo pipefail
    export PATH=/opt/homebrew/bin:$PATH
    just component
    # The full component, not the scanner: `display.html` needs `encoder`, and
    # `index.html` needs `codec.cbor-to-json` to show a CBOR body as JSON. A shipping
    # scanner would take `lc-code-scanner.wasm` and render host-side instead.
    npx --yes @bytecodealliance/jco@latest transpile dist/lc-code.wasm \
        -o web/lc --no-typescript
    cargo run -p lc-decode --release --example frame -- web/frame.rgba
    echo
    echo "  display:  http://localhost:{{port}}/display.html   (laptop)"
    echo "  scanner:  http://localhost:{{port}}/               (phone — needs HTTPS)"
    echo "  ^C to stop"
    cd web && python3 -m http.server {{port}}

# Check the WIT interface parses and round-trips.
wit-check:
    wasm-tools component wit crates/lc-wasm/wit/lc.wit > /dev/null && echo "  wit ok"

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
        "$root/$plat/libwasmtime.a" $libs -o target/c-host/lc-host
    ./target/c-host/lc-host

# ---------------------------------------------------------------- artifacts

# Regenerate examples/ — a symbol per payload, each decoded before it is written.
gallery:
    cargo run -p lc-render --release --example gallery -- examples

# Regenerate docs/img/ — the figures docs/explainer.html reads.
#
# Every figure is rendered by the encoder and damaged by the §15 harness at the same
# settings the explainer's quoted success rates were measured on, so a figure and its
# caption cannot drift apart. The page links these rather than embedding them, so it
# stays a 46 KB diff instead of a 2.7 MB one.
[doc("Regenerate the figures used by docs/explainer.html.")]
figures:
    cargo run -p lc-decode --release --example explain

# What JSON-shaped payloads cost, end to end.
payload-sizes:
    cargo run -p lc-decode --release --example json

# Sweep each degradation to find where decoding stops working.
#
# Reports success *rate* per cell, which is what §15 asks for and what comparing two
# builds requires. Optional argument is the trial count (default 16); the standard error
# is 0.5/sqrt(n), so pass 64 or more when a difference matters.
[doc("Sweep each degradation for success rate vs magnitude (§15).")]
envelope trials="":
    cargo run -p lc-decode --release --example envelope -- {{trials}}

# Measure where each layer fails, to calibrate the §8.2 rate split (open decision #5).
#
# Second argument is the profile — `just tune 12 C3`. Worth running per profile: chroma is
# 11% of C2's payload and 60% of C3's, so the same rate table lands very differently.
[doc("Measure where each layer fails, per profile, for the §8.2 rate split.")]
tune trials="" profile="":
    cargo run -p lc-decode --release --example tune -- {{trials}} {{profile}}

# One symbol size, each profile loaded to its own capacity — the comparison that says
# which profile to reach for. This is what established D11.
[doc("Compare the colour profiles head to head at one symbol size.")]
profiles trials="":
    cargo run -p lc-decode --release --example profiles -- {{trials}}

# Print detection internals for a degraded capture.
diagnose:
    cargo run -p lc-decode --release --example diagnose

# ---------------------------------------------------------------- codegen (xtask)

# Regenerate the §8.2 LDPC base graphs. §8.2 forbids generating these at runtime.
ldpc:
    cargo run --release -p xtask -- ldpc > crates/lc-core/src/ecc/tables.rs
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

# Regenerate the wire-format freeze, vectors/golden-v1.json (§15).
#
# Only run this when a wire-format change is *intended*. `golden_vectors_still_match`
# fails when the encoder's output moves, and the point is to notice; regenerating to make
# a red test go green throws away the signal. The diff is the record of what changed.
[doc("Regenerate the golden vectors. Only when a wire-format change is intended.")]
vectors:
    cargo run --release -p xtask -- vectors > vectors/golden-v1.json
    @echo "  wrote vectors/golden-v1.json — review the diff"

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
    @echo '  hooks installed — .githooks/pre-push runs: just ci'

# ---------------------------------------------------------------- housekeeping

clean:
    cargo clean
    rm -rf dist
