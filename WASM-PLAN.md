# LC Code in a WASM-only host

*Written 2026-09-03, for integrating LC Code into a WebAssembly-only browser as the
transport for offline passkey transfer.*

**Correction to earlier advice in this repo.** A previous note claimed "zero JS is not
reachable in a browser". That is true of *stock* browsers and false for the case here. If
you are building the browser, you own the host, and there is no JavaScript anywhere in the
picture. LC Code is unusually well suited to that — better than it is to the conventional
web — and this document explains why, then lists what integration actually needs.

---

## 1. Why WASM-only fails in a stock browser

Worth stating precisely, because the reason is not a missing feature that might land next
year.

WebAssembly has **no ambient access to anything**. A module can only touch its own linear
memory plus whatever functions its host imports into it. In a stock browser the host is
the JavaScript engine, and every capability a scanner needs — `navigator.mediaDevices`,
`<canvas>`, `getImageData`, the DOM to draw a result — exists only as a JS object graph.
There is no wasm-facing binding for any of them. So a page must:

1. Run JS to instantiate the module.
2. Run JS to open the camera and pull a frame.
3. Run JS to copy those pixels into wasm memory.
4. Run JS to read the result back and put it on screen.

Steps 2 and 4 are irreducible in that environment: the capability is only expressed in
JS, so removing JS removes the capability. WasmGC and direct-DOM proposals shorten step 3
and 4's glue but never step 2. That is the whole of the argument, and it evaporates the
moment the host is not a JS engine.

**Your host is not a JS engine.** You supply the camera and the display, so you import
frames into the component directly and read the decode result directly. The JS tier does
not exist to be removed.

---

## 2. What LC Code already gives you

Three properties that were designed for a non-browser host and happen to be exactly what a
WASM-only runtime wants.

**It imports nothing.** The scanner component instantiates against a linker containing
*literally nothing* — no WASI, no clock, no randomness, no filesystem. That is asserted, not
assumed: `crates/lc-wasm/tests/component.rs::instantiates_with_an_empty_linker` fails the
build if the component ever acquires a host dependency. For a minimal runtime this is the
difference between "embed a wasm module" and "implement a syscall surface first".

**It is already a Component Model component**, not a browser bundle. `wit/lc.wit` is the
whole contract: typed records, a `result` type for errors, and a resource for multi-frame
state. No hand-written ABI, no struct layout to agree on out of band.

**The pixel interface is host-shaped.** `image` takes `pixels`, `width`, `height`,
`stride` and a `pixel-format` of `gray8` / `rgb8` / `rgba8`. Stride is explicit because
camera planes are padded, and `gray8` decodes the luma layer alone — useful if your capture
path can hand over a Y plane without a colour conversion.

| artifact | raw | zstd | exports |
|---|---:|---:|---|
| `dist/lc-code-scanner.wasm` | 193 KB | ~69 KB | `decoder` only |
| `dist/lc-code.wasm` | 283 KB | ~104 KB | `decoder` + `encoder` |

Two artifacts because §14.1 keeps the encoder out of a scanner deliberately. **A transfer
needs both ends**: the sending device encodes and displays, the receiving device scans. If
one device does both, it links the full component; if the sender is a separate device with
a smaller job, it can link only what it needs.

---

## 3. Two integration surfaces

**Both are wanted.** Option A is the primary path and exists today. Option B is a fallback
for a runtime that cannot host a component, and is specified here so it can be built when
needed — or discarded without anything else changing. They are not exclusive: the same
`lc-decode` crate backs both, so a host can start on A and fall back to B without the
decoder itself moving.

| | A — Component Model | B — flat core ABI |
|---|---|---|
| Status | **ships today** (`dist/lc-code-scanner.wasm`) | **design only** — `lc-ffi` is a stub |
| Runtime needed | Component Model | any core wasm |
| Module size | 193 KB | ~180 KB estimated |
| Host code | bindings generated | ~40 lines, hand-written |
| Memory | marshalled for you | you own the protocol |
| Type safety | typed, versioned | offsets in a struct |

### Option A — Component Model runtime *(primary)*

Instantiate `lc-code-scanner.wasm` directly. Typed values cross the boundary and lists are
marshalled for you; `wit/lc.wit` is the entire contract.

* **Cost:** the runtime must implement the Component Model, not just core wasm. Wasmtime
  does. `wasmi`, `wasm3`, `wazero` and WAMR are core-wasm-only or partial — see D9, where a
  Go host was abandoned for exactly this reason.
* **Benefit:** no ABI to design, no memory management to hand-roll, and the interface is
  versioned (`lc:code@0.4.0`) so a mismatch is a load error rather than corrupt pixels.

Start here. 193 KB against ~180 KB is not a reason to own a memory protocol.

### Option B — flat core ABI *(fallback, not yet built)*

A core `wasm32-unknown-unknown` module exporting a C-style surface, callable from any
core-wasm runtime. **This is `lc-ffi`, currently a one-line placeholder**, already on the
roadmap as "C ABI" — targeting it at wasm as well as native serves both at once.

Reach for it if your runtime is core-wasm only, if you are writing the interpreter
yourself, or if the Component Model's generated glue is unwelcome in a constrained target.

Specified below so it is buildable rather than merely mentioned. Nothing here exists yet;
treat it as the design to build against, and expect the details to move once it does.

**Exports**

```
lc_abi_version() -> u32                     // bump on any layout change; check it first
lc_alloc(len: u32) -> u32                   // returns an offset into guest memory, 0 on failure
lc_dealloc(ptr: u32, len: u32)

lc_decode(ptr: u32, len: u32, width: u32, height: u32,
          stride: u32, format: u32) -> u32  // returns an offset to the result block
lc_result_free(res: u32)

lc_acc_new() -> u32                         // handle, 0 on failure
lc_acc_push(acc: u32, ptr: u32, len: u32, width: u32, height: u32,
            stride: u32, format: u32) -> u32   // 0 = ok
lc_acc_frames(acc: u32) -> u32
lc_acc_decode(acc: u32) -> u32              // result block, as lc_decode
lc_acc_reset(acc: u32)
lc_acc_free(acc: u32)
```

`format` is `0 = gray8, 1 = rgb8, 2 = rgba8`, matching `pixel-format` in the WIT.

**Result block** — 17 little-endian 4-byte fields, 68 bytes total. Every field is `u32`
except the four marked `f32`, and on `wasm32` a pointer is 4 bytes, so the whole block is a
flat array of words:

| offset | field | notes |
|---:|---|---|
| 0 | `status` | 0 = decoded, non-zero = failed |
| 4 | `packet` | 0 unsigned, 1 verified, 2 unverified, 3 partial |
| 8 | `ctype` | §9.2 content type |
| 12 | `body_ptr` | offset of the body bytes |
| 16 | `body_len` | |
| 20 | `version` | 1..=32 |
| 24 | `profile` | 0 M, 1 C1, 2 C2, 3 C3 |
| 28 | `luma_iterations` | |
| 32 | `chroma_iterations` | |
| 36 | `frames_combined` | |
| 40 | `references_rejected` | |
| 44 | `chroma_recovered` | 0 or 1 |
| 48 | `alignment_found` | 0 or 1 |
| 52 | `snr_db` | `f32` |
| 56 | `chroma_snr_db` | `f32` |
| 60 | `illum_residual_rms` | `f32` |
| 64 | `format_margin` | `f32` |

**On failure `status` is non-zero and `body_ptr`/`body_len` carry a UTF-8 message** — one
mechanism for both outcomes, so a host never has a second error channel to forget.

**Call sequence**

1. `lc_alloc(len)` → offset; write pixels there through the runtime's memory accessor.
2. `lc_decode(...)` → result offset.
3. Read the 68-byte block; if `status == 0`, read `body_len` bytes at `body_ptr`.
4. `lc_result_free(res)`, then `lc_dealloc(ptr, len)`.

Two properties worth preserving if the design changes. **The guest owns every allocation** —
the host never frees guest memory itself, only asks the guest to. And **the result block is
fixed-size and position-independent**, so a host reads it with constant offsets and no
parser, which is the whole point of choosing B over A.

**What B gives up.** The §10.8 guarantee that `unverified` carries no body is enforced by
the *type* in A — a variant with no payload. In B it is a convention: `packet == 2` with
`body_len == 0`. A careless host could ignore `packet` and read the body anyway. If you
take B and later add signing, that is the line to be careful about.

---

## 4. Task list

### Runtime integration

- [ ] **Start on A.** It exists, and everything below is identical either way — the capture
      path and the payload questions do not care which surface carries the pixels.
- [ ] **Instantiate the scanner component** and get one decode from a static test frame
      before touching the camera. `cargo run -p lc-decode --release --example frame` emits a
      raw RGBA capture with a `u32` width / `u32` height header — a fixture that removes
      capture from the equation while the plumbing is proven.
- [ ] **Confirm the empty linker holds in your runtime.** If instantiation demands an
      import, the module is not the one this repo builds.
- [ ] **Build B only when A is ruled out**, and say why in the issue. It is ~200 lines of
      Rust in `lc-ffi` plus the host side, and it is dead weight if the runtime can host a
      component. The one signal that settles it: does your runtime load
      `dist/lc-code-scanner.wasm` at all?
- [ ] **If B is built, cover the same ground A's tests do.** `lc-wasm/tests/surface.rs` and
      `component.rs` are the model — a round trip, the accumulator, malformed input
      returning an error rather than trapping, and `unverified`/`partial` exposing no body.
      That last one is a type guarantee in A and only a convention in B, so it needs a test
      that A gets for free.

### Capture path

- [ ] **Get frames from the sensor into a supported layout.** This is the task most likely
      to surprise: camera stacks usually deliver **NV12, YUY2 or another YUV format**, not
      RGBA. Today the host must convert.
- [ ] **Consider adding `pixel-format::nv12`** (and possibly `yuy2`) to `wit/lc.wit`. LC
      Code already separates luma from chroma internally, and NV12 *is* a separated Y plane
      plus interleaved chroma — so this is a natural fit rather than a bolt-on, and it
      removes a full-frame colour conversion from the hot path on every frame.
- [ ] **Honour §13.1 on the display side.** ≥ 8 device pixels per module, integer scaling,
      maximum brightness, colour-adaptive modes off. These are the sending device's
      obligations and they dominate the error budget.
- [ ] **Use the accumulator.** `decoder.accumulator` combines log-likelihoods across
      frames; §12.4 calls it "the single best use of a live camera preview" because glare and
      banding move between frames while the symbol does not. Push frames until `try-decode`
      succeeds. It is a *resource* — release it, or you leak guest memory across scans.
- [ ] **Drive user guidance from diagnostics.** `snr-db`, `chroma-recovered`,
      `references-rejected` and `alignment-found` exist so a UI can say "move closer" or
      "tilt away from the glare" rather than "failed".

### Payload — passkey specifics

- [ ] **Encrypt before encoding. LC Code provides no confidentiality** (§10.8) — it is a
      public channel by construction, readable by anything with a camera in line of sight,
      and a displayed symbol is trivially photographed. This is not a gap to be fixed at the
      symbology layer. The right shape is what FIDO's credential-exchange work already
      does: encrypt the credential to the receiver's key, then use LC Code purely as the
      transport for the resulting ciphertext.
- [ ] **Size the symbol from the ciphertext.** Encrypted blobs do not compress, so budget
      the wire size directly:

      | payload | smallest version, C1 | smallest version, C3 |
      |---:|---|---|
      | 256 B | v7 (45×45) | v6 (41×41) |
      | 512 B | v11 (61×61) | v9 (53×53) |
      | 1 KB | v17 (85×85) | v14 (73×73) |
      | 2 KB | v26 (121×121) | v21 (101×101) |
      | 3 KB | v32 (145×145) | v26 (121×121) |
      | 4 KB | — *exceeds v32* | v31 (141×141) |
      | 4475 B | — *exceeds v32* | v32 (145×145) — the ceiling |

      `C3` is the density profile — 8 colours, 3.00 bits/module — and it tops out at
      **4475 B**, the format's ceiling. Above that you need multiple symbols, which LC Code
      does not currently define a docking scheme for. If passkey blobs routinely exceed
      ~4 KB, that is a format-level gap to raise early.
- [ ] **Weigh `C3` against capture quality.** It carries 1.42× `C1` but gives up ~25% of
      its noise tolerance and ~15% of its glare tolerance (D11). Device-to-device transfer at
      close range on a bright screen is close to the best case a colour code ever gets, so
      `C3` is defensible here in a way it is not in general — but measure it on your own
      hardware with `just profiles` before committing.
- [ ] **Decide whether the transfer needs a signature.** If the receiver must know *who*
      issued the blob rather than merely that it decoded, that is §10 COSE_Sign1 — and
      **`lc-verify` is a stub**. Scope it as real work: COSE/CWT parsing, ES256 and Ed25519,
      a `KeyResolver`, and expiry handling. Note §10.5's hard rule — a public key travelling
      inside the symbol is not authentication, and a verifier must never accept one.
      Often the encryption layer already covers the trust question and no signature is
      needed; decide deliberately rather than by default.

### Before depending on the wire format

- [ ] **Freeze and generate golden vectors.** The format is still moving — the palette
      changed on 2026-09-02 (D1) and `C3`'s geometry on 2026-09-03 (D11). A symbol encoded
      today is not guaranteed to decode against next month's build. If two independently
      built devices must interoperate, a frozen vector file is the thing that makes that
      testable, and it does not exist yet.
- [ ] **Pin the interface version.** `lc:code@0.4.0` is in the WIT package name; a host
      that checks it gets a clean failure instead of a subtle one.

---

## 5. What is *not* needed

Worth saying explicitly, because these are the usual costs of embedding a decoder and none
of them apply.

* **No JavaScript**, no `jco`, no `wasm-bindgen`. Those exist in this repo only to serve
  stock browsers — see `web/`, `just web` and [`FIREFOX.md`](FIREFOX.md), which are a
  demonstration of the *other* case, not the path here. They are worth a look anyway: the
  laptop-displays / phone-scans demo is the shortest way to see the format work end to end
  before committing to an integration.
* **No WASI.** No clock, no randomness, no filesystem, no environment. Verification would
  need a clock if you add signing, but the decode path never does — and §14.4 makes time an
  explicit parameter rather than an ambient read, precisely so a host without one still
  works.
* **No threads.** Single-threaded by design (§14.2).
* **No C toolchain and no native dependencies** anywhere in the decode path.
* **No network, ever.** The point of the format.
