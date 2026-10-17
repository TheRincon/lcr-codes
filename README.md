# LCR Code

**L**uma / **C**hroma / **R**ing code — a colour 2D matrix symbology for **portable offline data
transmission**. Structured, JSON-shaped payloads at higher density than a monochrome
matrix code, read from a phone screen or from print, with no network involved at any
point.

Reference implementation of [`lcr-code-spec-v0.4.md`](docs/lcr-code-spec-v0.4.md), in Rust,
shipped as a **WebAssembly component**.

**Zero warnings.** `just test` runs the whole suite.

---

## What it looks like

<img src="examples/profile-c3.png" width="360" alt="An LCR Code symbol">

The geometry of the symbol is in flux. There is a version with 3 finders and an alignment "bullseye" and one without. 

### "Old Style"
Three finder bullseyes and an alignment marker mark the geometry, using the 2 luma levels of a single hue. This means detection works before any colour processing exists. On
a colour profile that means one hue's dark/light pair rather than black and white: a
four-colour palette has no neutral pair to use, and borrowing black and white would put
the fixed patterns outside the palette, giving the detector luma centroids that do not
match the payload's. What detection actually needs is the 1:1:3:1:1 run ratio in luma,
which holds regardless of hue.

The colour carries the payload: a luma layer at full module pitch and a chroma layer at
equal or coarser pitch, with independent code rates. The outer ring is the optional §6.4 reference border, a
calibration target for the illumination fit.

[`examples/`](examples/) has a symbol for each of seven representative payloads, plus one
per profile, each as PNG, SVG and the source JSON. Every one is decoded before it is
written, so anything there reads back to the JSON beside it.

Two integration guides, for opposite hosts:

- **[`WASM-PLAN.md`](docs/WASM-PLAN.md)** — a host you control. No JavaScript, no WASI, no host
  imports at all, because the component asks for none.
- **[`FIREFOX.md`](docs/FIREFOX.md)** — a stock browser, with camera capture. `just web`
  transpiles the component and serves it; the bundle is 47% of §14.2's gzipped budget.

**[`docs/explainer.html`](docs/explainer.html)** is the illustrated walkthrough — what the
luma and chroma layers actually are, what a decibel means here, how the profiles compare,
and what each kind of damage does. Open it in a browser; every figure in it is rendered by
the encoder and damaged by the §15 harness rather than drawn by hand. `just figures`
regenerates them.

## Why not just use a QR code

Density, and graceful failure. LCR Code targets **1.5–2.0 bits/module** against monochrome's
1.0 — measured at **1.73×** on this implementation — so payloads that would need an
unreasonably large QR code fit in a scannable symbol.

**You can now check this claim yourself** run `just web` and open
`http://localhost:8000/display.html`, which draws a QR of the same payload beside the LCR symbol at the same module pitch and reports the ratio. QR generation is the `qrcodegen` crate and the test
suite reads the result back with `rqrr`, an independent decoder, so the thing being
compared against is a real QR symbol.

Be careful what the ratio means, because it moves with the pairing. On a 50-byte CBOR
payload, against `C1` at ECC level 1:

| | QR-L | QR-M | QR-Q | QR-H |
|---|---:|---:|---:|---:|
| QR grid | 29×29 | 33×33 | 37×37 | 41×41 |
| area vs LCR Code's 29×29 | **1.00×** | 1.29× | 1.63× | 2.00× |

So LCR Code is *level with* QR at its weakest error correction and pulls ahead as QR's
strengthens. The bits/module figure above and the area figure here are different
measurements and only the second is what a user sees. Note also that LCR Code is carrying
far more redundancy than QR-L in that tie — ECC level 1 is a luma rate of 0.75 — so the
honest summary is that the two symbologies are close at small payloads and the gap opens
with size and with error correction, not that LCR Code is uniformly 1.73× anything.

The trade is deliberate. JAB Code reaches ~3 bits/module; LCR Code spends the difference on
operating margin, because the impairments that actually break colour codes on a phone
screen are severe: specular glare, panel refresh beating against a rolling shutter, moiré,
and display colour modes that shift the whole palette. Two design choices follow:

- **The luma layer alone is a complete monochrome code.** Lose chroma entirely — bad light,
  a monochrome sensor — and you still get a result rather than a hard failure.
- **Colour errors are correlated across space, not independent.** A shadow or a glare
  highlight moves a whole region coherently, so ~2% of the symbol area spent on scattered
  known-colour references buys a fitted correction surface that cancels most of it.

## Quick start

```bash
just              # list every recipe
just test         # the whole suite
just component    # build both wasm components into dist/
just gallery      # regenerate examples/
just deny         # dependency licences and RustSec advisories (needs cargo-deny)
just web          # run the illustrative example, needs a concurrent "cloudflared tunnel --url http://localhost:8000" for full example.
```

Requires a Rust toolchain, plus [`just`](https://github.com/casey/just),
[`wasm-tools`](https://github.com/bytecodealliance/wasm-tools), `zstd`, and
[`cbindgen`](https://github.com/mozilla/cbindgen) (`cargo install --locked cbindgen`) for
the C header check. On x86 and x86_64 the CLI's AVIF support also needs `nasm`.

`just hooks` points git at `.githooks`, whose `pre-push` runs `just ci`. Pre-**push**
rather than pre-commit on purpose: the gate builds two components and runs the suite in
release mode, which is too slow to sit between `git commit` and your prompt.

### From the command line

```bash
cargo install --path crates/lcr-cli          # installs `lcr`

lcr encode -d '{"name":"Fred"}' -o fred.png  # JSON goes on the wire as CBOR
lcr encode -d '{"name":"Fred"}' -o fred.gdf  # or .svg; .gdf is a one-page GDF document
lcr encode notes.txt                         # no -o: a preview in the terminal
lcr decode photo.jpg                         # prints {"name":"Fred"}
lcr decode --combine f1.png f2.png f3.png    # multi-frame (§12.4)
lcr decode --json photo.jpg                  # payload plus diagnostics, as JSON
lcr capacity                                 # bytes per version and profile

lcr send report.pdf -o symbols/              # a file as a sequence of symbols
lcr receive symbols/*.png                    # any order; names what is missing
```

`lcr decode` reads PNG, JPEG, WebP, JPEG XL and AVIF, chosen by the file's content rather
than its name, all in pure Rust. Each format is a cargo feature, so a smaller build can
drop what it does not need. HEIC (the iPhone camera's default), GIF, BMP, TIFF and camera
RAW, JPEG 2000, PSD and PDF are recognised but not read: `lcr` names the format and prints
the command that converts that file to PNG — `sips` on macOS, ImageMagick elsewhere.

### From C

`crates/lcr-ffi` builds `liblcr_ffi` (static and shared) with the header
[`include/lcr.h`](crates/lcr-ffi/include/lcr.h): the same surface as the WebAssembly
component, minus QR comparison. Every call returns a status, `lcr_last_error()` explains a
failure, and everything the library returns has a matching `_free`.
[`tests/smoke.c`](crates/lcr-ffi/tests/smoke.c) is the shortest complete example, and
`just ffi-test` builds and runs it.

### From Rust

```rust
use lcr_encode::EncodeOptions;
use lcr_render::{RenderOptions, render_rgb};
use lcr_decode::Decoder;

// Author in JSON, transmit CBOR (§9.4).
let sym = lcr_encode::encode_json(
    r#"{"name":"Fred","loc":"45.00,55.0"}"#,
    &EncodeOptions::screen(),
)?;

let img = render_rgb(&sym, &RenderOptions::screen());

let out = Decoder::new().decode_rgb(&img.data, img.width, img.height, img.width * 3)?;
if let lcr_core::Packet::Unsigned { body, .. } = out.packet {
    let value = lcr_core::cbor::Value::from_slice(&body)?;
    println!("{}", lcr_core::json::to_json(&value));
}
```

### From any WebAssembly host

The deliverable is a **component**, not a browser bundle. The interface is declared in
[`crates/lcr-wasm/wit/lcr.wit`](crates/lcr-wasm/wit/lcr.wit) and host bindings are generated
from it, so a host in Rust, Go, C or C++ adds support with no native dependency and no
hand-written ABI.

```
just component

dist/lcr-code.wasm          decode + encode   283,338 bytes   (103,693 zstd)
dist/lcr-code-scanner.wasm  decode only       192,876 bytes    (68,845 zstd)
```

```wit
export lcr:code/decoder@0.4.0;   // decode, plus a multi-frame accumulator resource
export lcr:code/encoder@0.4.0;   // encode-json / -text / -bytes / -json-svg
```

**Neither component imports anything from its host.** The single `import lcr:code/types`
is a types-only interface — zero functions, nothing to provide. No WASI, no clock, no
randomness, no filesystem. It instantiates anywhere a Component Model runtime exists, and
that is asserted rather than assumed: `instantiates_with_an_empty_linker` loads both
components into wasmtime with a linker containing literally nothing.

Two builds because §14.1 keeps the encoder out of a scanner, so one cannot be acquired by
accident.

`just c-host` builds and runs a **C host** (`examples/c-host/main.c`) against the wasmtime
C API — encode and decode in one process, from a host sharing none of the guest's idioms.
Note that **Go cannot host a component yet**: `wasmtime-go` ships a component-capable C
library its Go API does not expose, and `wazero` is core-wasm only. See D9.

`just wasm-size` gates the scanner at **93% of §14.2's 200 KB budget**, measured **raw**.
The spec says "gzipped" because it assumed a browser bundle fetched over a network; a
component is loaded from disk, so uncompressed size is what has to fit. That distinction
is worth 60 percentage points here — the same artifact is 33% of budget compressed — so
the compressed figure would report headroom that does not exist.

## Layout

| Crate | State | What it is |
|---|---|---|
| `lcr-core` | ✅ | Geometry, palette, ECC, container, masking, modulation |
| `lcr-decode` | ✅ | §12 pipeline: detect, illuminate, demodulate, decode |
| `lcr-encode` | ✅ | §8 pipeline; auto version, auto compression, mask search; §10 issuing |
| `lcr-render` | ✅ | SVG, RGB raster, PNG, ANSI terminal preview |
| `lcr-wasm` | ✅ | The WIT component surface, plus tests run under wasmtime |
| `lcr-verify` | ✅ | §10 offline verification: COSE_Sign1, CWT claims, ES256 + Ed25519. Optional |
| `lcr-cft` | ✅ | CBOR File Transfer ([`docs/CFT.md`](docs/CFT.md)): chunked, hash-verified file transfer over any carrier, LCR Code symbols included |
| `lcr-ffi` | ✅ | C ABI: the component's surface as a static or shared library, with a generated header |
| `lcr-cli` | ✅ | The `lcr` command: encode, decode, probe, capacity, and file transfer from images |
| `xtask` | ✅ | Table generation that must call into the crates |

Signing is **opt-in and not the point** (§1.2). Most payloads are expected to be unsigned
structured data — a config blob, a pairing token, a contact record — and an unsigned CBOR
payload pays ~4 bytes of framing and nothing else.

## Automation

Two layers, split by whether a task needs the project's own types:

- **`justfile`** — shell-shaped orchestration: builds, size gates, artifact generation.
  `just --list` is the front door.
- **`xtask`** — tasks that must call into the crates. `capacity` recomputes the §6.1 table
  from `lcr_core::capacity` rather than restating it; `ldpc` generates the §8.2 base graphs,
  which the spec forbids building at runtime. No command runner can do either.

The justfile wraps xtask, so `just ldpc` and `just capacity` work and xtask stays an
implementation detail.

## Read before contributing

- **[`docs/decisions.md`](docs/decisions.md)** — every `[TBD]` pinned, and **nine places
  the implementation departs from the spec or the ecosystem forced a choice**, each with
  its reason and the test that locks it in. Two are provisional and want sign-off: the
  palette (D1/D2) and the illumination model (D3).
- **[`PROGRESS.md`](docs/PROGRESS.md)** — what is built, what is left, and what was fixed.

The spec is a **draft**, not a frozen format. Sections marked `[NORMATIVE]` define the wire
format and must be settled before v1.0; the §8.2 ECC rates in particular are still the
spec's guesses and are the last thing blocking a freeze.

## Status

Encode and decode are complete and round-trip end to end through a rendered image. The §15
degradation harness is built, and the operating envelope is measured rather than asserted —
perspective, shading, specular glare, display colour modes, sensor noise, defocus, 4:2:0
subsampling, and multi-frame combining all have tests that name the claim they check.

## Licence

The code is available under your choice of [MIT](LICENSE-MIT),
[Apache-2.0](LICENSE-APACHE) or the [Unlicense](UNLICENSE). Apache-2.0 is the option that
carries an explicit patent grant.

Unless you state otherwise, any contribution you submit for inclusion is offered under the
same three licences, with no additional terms.

The specification, [`lcr-code-spec-v0.4.md`](docs/lcr-code-spec-v0.4.md), is published under the
[Open Web Foundation Final Specification Agreement 1.0](https://www.openwebfoundation.org/the-agreements/the-owf-1-0-agreements-granted-claims/owfa-1-0),
which covers the text and gives implementers a royalty-free patent commitment. The author
holds no patents on the format and pledges never to assert any against a conforming
implementation.

See [Appendix B](docs/lcr-code-spec-v0.4.md#appendix-b--prior-art-and-licensing) of the spec for
prior art — JAB Code, HCCB, and the EU DCC's COSE/CWT container — and read it before
publishing anything commercial.
