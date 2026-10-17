# LCR Code — implementation progress

Reference implementation of [`lcr-code-spec-v0.4.md`](lcr-code-spec-v0.4.md).

**State:** encode and decode are both complete and round-trip end to end through a
rendered image — and, as of 2026-09-04, **through a real iPhone camera pointed at a real
laptop screen** (see "First real-world decode") — detection, illumination fit, soft
demodulation, LDPC, framing, CRC. The §15 degradation harness is built and the operating
envelope is mapped as the success *rate* §15 asks for (`just envelope`). `lcr-wasm` ships
both paths as a **WebAssembly component** with a WIT interface (D8), in two builds:
decode-only and full.

**The capture sweep has been run** (2026-09-14, `sweep1.json` + `sweep2.json` in `docs/data/capture-sweeps.tar.zst`,
4,207 trials). Its headline is that **holding the phone face-on, not distance, was the
dominant failure mode** — see "First measured envelope" below. Every *other* envelope figure
in this file still comes from the §15 harness, which is code.

**Provisional, pending review:** the palette (D1/D2) and illumination-model (D3) changes in
[`docs/decisions.md`](decisions.md) are being carried forward so work can continue,
not signed off. Both are cheap to unwind — see "Unwinding the provisional decisions" below.

**All tests passing, zero warnings.** `just test` (plus `just test-debug`, unit tests
unoptimised — see below). Three tests are `#[ignore]`d on purpose: the ring detector's
diagnostic probes in `tests/ring_diag.rs`, which print rather than assert.

---

## Picking this up again

*Rewritten 2026-09-02. The previous version of this section was stale: it warned that
nothing was committed, but the tree is committed and clean.*

### Nothing is half-finished

The palette gamut-clipping fix below is complete — constants, docs, guards, regenerated
gallery. `just test` and `just test-debug` are both green. Nothing needs unwinding.

### Verifying the world after a restart

```bash
just component     # rebuild dist/ — gitignored, so it will be missing
just test          # the whole suite; depends on `component`
just test-debug    # unit tests unoptimised, exercising debug_assert!
just c-host        # the C trial, needs the Go module cache (see below)
```

### Things a restart may take with it

- **`dist/`** — build output, not tracked. `just component` regenerates it in ~30s.
- **`~/go/pkg/mod/.../wasmtime-go/v34@v34.0.0`** — `just c-host` borrows the
  component-capable `libwasmtime.a` bundled there (D9). If it is gone:
  `go get github.com/bytecodealliance/wasmtime-go/v34`, or point `WASMTIME_C_API` at an
  unpacked wasmtime-c-api release.

### What to pick up next

**Run a capture sweep on finder-less symbols (D17).** Screen symbols no longer have finders,
and the ring detector that replaces them has only been tested on the §15 harness. It treats
each connected region of ink as one symbol, which a real scene may not honour, and its cost
per frame is untimed. The sweep is the measurement.

**Earlier: run the capture sweep again, with the rig's two faults fixed.** The first run
(2026-09-14) found that face-on capture, not distance, was the dominant failure mode — and
also found two defects in the rig itself: it asked the camera for 1280px video when the
phone can do 4K, and it offered the operator no guidance. Both are fixed. A second run
should therefore measure a materially different envelope, and this time wants the session
panel filled in and each protocol run separately labelled.

See "First measured envelope" for what came back, and
[`docs/capture-sweep.md`](capture-sweep.md) for the procedure.

---

**§8.2 ECC rate tuning is done, and the rates did not move.** The full argument is D10 in
[`docs/decisions.md`](decisions.md); the short version is that the table was right and
its stated reason was not.

- `just envelope` reports success *rate* per cell (§15's metric) with a clean control row.
- `just thresholds` writes [`docs/ldpc.md`](ldpc.md) — each rate step buys ~1 dB, and
  §8.2's two-step offset grants chroma 1.8–2.4 dB.
- `Diagnostics::chroma_snr_db` measures the chroma channel on the same scale as the luma
  one. Chroma reads **7.6 dB better** than luma on a clean capture, not worse — it is the
  palette's amplitude ratio, `20·log10(55/22.9)`. So §8.2's "chroma is measured over a
  noisier channel" is not what the numbers say.
- But narrowing the offset to one step, tested directly, buys 8.6% capacity on `C1` and
  drops blur-radius-4 success from **81% to 3%**. The protection is earning its keep.
- Both are true because chroma's handicap is **spatial bandwidth, not noise** (§1.4, not
  §8.2). A per-module noise sigma is blind to defocus, which is the impairment that
  actually binds chroma.

Remaining, and smaller than it was: the result is measured on `C1` at one payload size.
**Done, including the per-profile check.** `just tune C3` and `just profiles` closed the
gap D11 opened: making chroma 60% of `C3`'s payload meant the rate table had only ever
been validated where chroma was the *minority* layer. It survives both — narrowing the
offset costs ~20% of the blur envelope on `C1` and `C3` alike. The measurement also
sharpened D10: whether chroma is "the noisier channel" turns out to be a property of the
palette's hue spacing, not of chroma, and it is false for `C1` while true for `C3`.

Second, smaller: `just ci` runs locally via the pre-push hook but nothing runs it on a
fork, so that gate wants to be automatic rather than trusted to a hook someone can
`--no-verify` past. The scanner's size is no longer the pressure it was — see below.

### The size budget, 2026-09-14

**§14.2's "200 KB" was being read 2.5× too strictly, and the scanner had 20 KB of free
savings in it.** Both fixed; D15 has the argument.

| | before | after | budget | |
|---|---:|---:|---:|---:|
| scanner raw | 199,453 | **178,720** | 204,800 | 87% |
| scanner gzip | 79,341 | **74,641** | 102,400 | 72% |

- **`wasm-tools strip --all` saves 20,733 bytes, free.** Plain `strip` saves 1,753 — it
  leaves the 19 KB `name` section, 10% of the artifact, that no shipped decoder reads. Now
  part of every component build; the core module keeps its names for debugging.
- **The gate measured raw only**, at 97%, while the gzipped figure sat at 38% unmentioned.
  §14.2 says "gzipped"; D8 made the artifact a disk-loaded component. Both readings are
  defensible and **both are now gated**. The compressed limit is a proposal, not a
  measurement — #16 still wants it justified against a real embedder.

**`opt-level = "z"` measured and declined.** The `wasm-min` profile had been defined and
never exercised since the project started. It gives another 17 KB raw but costs **11%
median and 22% p90 decode time**, and at ~80 ms for one 492×492 frame on a laptop, frame
rate is closer to binding than size is at 87% (§1.1 goal 5).

Worth saying plainly: `probe` (D12) and the sequence table (D14) were never the problem.
The headroom was.

---

`ci` is `fmt-check clippy test test-debug wasm-size check-compare`. **Clippy was added on 2026-09-08 and
was not there before**, which is how six lints accumulated in untouched code while this
file claimed zero warnings — `is_multiple_of` and friends arrived with a newer toolchain
and nothing was looking.

**The toolchain is pinned to 1.98.0** in `rust-toolchain.toml`, added the same day and for
the same reason. Under `-D warnings` a floating toolchain makes a green tree go red on
somebody's *upgrade* rather than on somebody's *change*; pinning makes an upgrade a
deliberate commit that fixes the new lints in the same breath. The file also declares the
`wasm32-unknown-unknown` target and the rustfmt/clippy components, so a fresh clone
acquires them automatically instead of discovering the gap one failing recipe at a time.

Two things that pin does *not* do. It has no `>=` form — it selects exactly 1.98.0, and
the supported floor for consumers stays `rust-version = "1.94"` in `Cargo.toml`. **Nothing
verifies that floor**, because CI now only ever builds with the pin; if the MSRV claim
matters, it wants its own job.

---

## Where things stand

| Crate | State | Tests | Notes |
|---|---|---|---|
| `lcr-core` | ✅ Complete | 140 | Geometry, palette, ECC, container, masking, modulation |
| `lcr-encode` | ✅ Complete | 11 | Full §8 pipeline; auto version, auto compression, mask search |
| `lcr-render` | ✅ Complete | 16 | SVG, RGB raster, PNG, ANSI preview; §17.2 dual-render behind `qr` |
| `lcr-decode` | ✅ Complete | 88 | §12 pipeline end to end, plus the §15 harness |
| `lcr-verify` | ⬜ Stub | — | §10 signing, entirely untouched |
| `lcr-wasm` | ✅ Complete | 24 | Component Model surface over `wit/lcr.wit` (D8) |
| `lcr-ffi` | ⬜ Stub | — | |
| `lcr-cli` | ⬜ Stub | — | |
| `xtask` | ✅ Working | — | `ldpc`, `capacity`, `palette`, `thresholds`, `vectors`, `sweep` |

`lcr-decode`'s 88 are 62 unit, 14 round-trip, 12 degradation. `lcr-wasm`'s 24 are 5 component
and 19 surface.

Automation is split: `just --list` is the front door, and `xtask` holds the two tasks
that must call into the crates (`ldpc`, `capacity`). See the note atop the justfile.

### What works right now

```
just component       # both wasm components into dist/, validated
just test-debug      # unit tests unoptimised, so debug_assert! is live
just palette         # search the §4 palette constants against the sRGB gamut
just envelope 64     # success rate vs degradation magnitude, 64 trials/cell
just tune C3         # where each layer fails, per profile, for the §8.2 rate split
just profiles        # C1 / C2 / C3 head to head at one symbol size
just thresholds      # regenerate docs/ldpc.md (~4 min)
just wasm-size       # gate the scanner against §14.2's budget
just gallery         # regenerate examples/
just payload-sizes   # JSON payloads, end to end
just envelope        # degradation sweep
just diagnose        # detection internals
just sweep log.json  # reduce a real-camera capture log to §15's metric
```

The full path — `encode → render → decode` — round-trips across every profile, every
version, every mask, every ECC level, and every compression setting. Dropping the chroma
layer entirely still recovers a complete prefix of the packet, per §8.1.

[`examples/`](../examples/) holds a rendered symbol for each payload below — PNG, SVG and the
source JSON — every one decoded before it was written.

**JSON payloads**, measured by the `json` example rather than estimated:

| payload | JSON | CBOR | saved | framed | ver | side |
|---|---|---|---|---|---|---|
| contact (§6.1) | 34 | 26 | 24% | 30 | 2 | 25 |
| location fix | 54 | 39 | 28% | 43 | 2 | 25 |
| allergen profile (App. A) | 66 | 53 | 20% | 54 | 3 | 29 |
| device pairing | 67 | 52 | 22% | 56 | 3 | 29 |
| membership | 96 | 68 | 29% | 72 | 3 | 29 |
| device config | 144 | 114 | 21% | 118 | 4 | 33 |
| event admission | 144 | 96 | 33% | 100 | 4 | 33 |

Note **§9.4's "30–50% smaller" is optimistic** for these shapes — measured 20–33%, because
short string *values* do not shrink, only the structural punctuation does. §9.3 holds up
better: the encoder picks no compression on six of the seven.

### Read this before reviewing

**[`docs/decisions.md`](decisions.md)** — every `[TBD]` pinned, and **nine places the
implementation departs from v0.4 or the ecosystem forced a choice**. Two want your
sign-off:

- **The §4.3 palette fails three of §4.2's four bounds** (D1), and its hue states are not
  the same hue at both luma levels — which breaks the layer independence §4.1 exists for.
- **§12.3's residual surface has to be multiplicative, not additive** (D3). An additive
  surface cannot cancel a shading gradient, which is the impairment the step exists for.

Also, all settled rather than open: §6.1's capacity table was 15–20% optimistic at the
large end (D5); §5's bits/module figure is unreachable for `C2` (D4); §13.1's ≥ 8
px/module floor did not detect (D6); `no_std` does not build and will not be made to (D7);
and the wasm target is a Component Model component rather than the browser bundle §14.2
describes (D8).

---

## What is left

### 1. Host integration

The component exists and is measured; what is missing is the other side of the boundary.

- [x] **Component-level integration tests.** `lcr-wasm/tests/component.rs` loads both
      artifacts into wasmtime and runs them: encode → decode through the canonical ABI,
      the `accumulator` resource across the boundary, errors arriving as `result::err`
      rather than traps, and the scanner proven to export no encoder. The empty-linker
      test turns "imports nothing from its host" from an inference into an assertion.
- [x] **A pre-push gate.** `.githooks/pre-push` runs `just ci`; install with `just hooks`.
- [x] **A C host trial** — `examples/c-host/main.c`, run by `just c-host`. Encode and
      decode in one process against the wasmtime C API, from a host sharing none of the
      guest's idioms. Two languages now, no Rust-specific assumptions.
- [ ] **Go is blocked upstream** (D9): `wasmtime-go` does not bind the Component Model and
      `wazero` is core-wasm only. Nothing to do here until that changes.
- [ ] `just ci` in actual CI. The hook covers local pushes, nothing covers a fork.

### 2. Tune the ECC rates — done

**Measured, and left unchanged** — see D10. The two-step luma/chroma offset is necessary
and maximal; §8.2's stated reason for it is wrong and D10 records the correct one. Still
now validated per profile as well — see the D10 addendum. `C2` is the one place the chroma
rate is nearly free: chroma is 11% of its payload, so its blur envelope does not move when
the rate does, and that rate could be chosen on capacity grounds alone.

### 3. Signing (§10) — optional, lower priority

`lcr-verify`: COSE_Sign1, CWT claims, ES256 + Ed25519, `KeyResolver`. §1.2 is explicit that
this is a feature and not the point, so it sits behind the decode path. `lcr-encode/sign.rs`
is a placeholder.

Worth doing carefully when we get there: test against externally generated vectors, not
just our own encoder — self-consistent crypto bugs are the dangerous kind.

### 4. Everything else

- [ ] `lcr-cli` — encode/decode/inspect dev tool.
- [ ] `lcr-ffi` — C ABI. Note edition 2024 needs `#[unsafe(no_mangle)]`.
- [x] **`fuzz/` — four targets, run clean.** `just fuzz <target> <secs>`; needs nightly and
      `cargo-fuzz`. Corpus is seeded from the golden vectors and a real capture, so the
      fuzzer starts from valid symbols rather than noise.

      | target | what it guards | runs |
      |---|---|---:|
      | `decode_rgb` | the camera-facing entry point, geometry included | 5,047 |
      | `cbor` | §9.2's typical body, post-ECC and pre-trust | 556,250 |
      | `json` | §9.4 authoring input, and the scanner's display path | 13,661,761 |
      | `frame` | §9.1 container, §9.5 multipart, §9.3 decompressors | 1,909,902 |

      **`decode_rgb`'s 5,047 runs is the number to be honest about.** A full decode is
      milliseconds, so it explores thousands of inputs where the parsers explore millions;
      it found nothing, but that is much weaker evidence than the others. It wants an
      overnight run, and it is the target that matters most.

- [x] **Golden vector file** — `vectors/golden-v1.json`, 34 vectors, regenerated by
      `just vectors` and enforced by `golden_vectors_still_match`. Covers every profile,
      every mask, every ECC level, both ends of the version range, all three input kinds,
      forced DEFLATE, and the degenerate payloads that once defeated detection. Verified
      to work in both directions: perturbing mask 2 reddens 9 of 34 vectors and names the
      first differing module, while changing a palette *colour* correctly changes nothing,
      because the canonical form is palette **indices**. That boundary is deliberate —
      recalibrating against measured panels is a rendering change, not a wire-format one.
- [ ] Print palette (§4.5) — `C2` currently borrows the screen palette.
- [ ] Brotli preset dictionary (§9.3 `cflag 11`) — **not built, and the reason changed on
      2026-09-05.** Still rejected rather than silently mishandled.

      Two blockers, one of them new. §9.3 says build the dictionary "from a real corpus; do
      not hand-write it", and no corpus exists — seven example payloads is not one.

      The new one is decisive: **brotli does not fit the scanner.** Building the scanner
      component with `lcr-decode/brotli` takes it from 192,876 to 392,293 bytes. The codec
      alone is 199,417 bytes — 97% of §14.2's entire budget — putting the scanner at 192%
      of it. So `cflag 10` *already* emits symbols a conforming minimal decoder rejects,
      and `cflag 11` would add a dictionary on top of a codec that cannot be there.

      Do not start on the dictionary. Settle what `cflag 10` is for first: a full build
      that already carries the encoder can afford brotli, a scanner cannot, and two of the
      four `cflag` values currently point at a codec half the decoders will refuse. That is
      a wire-format question, not a compression one, and it wants answering before the
      format is offered to third parties.

      The API side was scouted and is workable if it ever proceeds: `brotli` 7.0.0 exposes
      custom dictionaries only through `BrotliCompressCustomIoCustomDict` and
      `BrotliDecompressCustomIoCustomDict`, the heavily-generic `CustomIo` variants — which
      suit `lcr-core`'s `no_std + alloc` shape, but need a `BrotliAlloc` and custom IO
      wrappers rather than the one-line `BrotliCompress` the standard path uses. Note also
      that brotli ships no trainer; `zstd --train` is the usual way to *discover* the
      substrings, which are then used as brotli dictionary bytes.
- [ ] Remaining §15 screen impairments: PWM banding, subpixel geometry, a moiré sweep.
      The moiré sweep is also what closes open #14, and note the capture sweep will *not*
      close it — a walk backwards varies pitch, not the fractional device-pixel ratio.
- [ ] **Run the capture sweep.** The rig is built; see below. This is the outstanding
      item with the best ratio of value to effort, and the only one that produces a number
      the §15 harness did not.

---

## Robustness round — 2026-10-03 to 2026-10-07 (D22–D26)

Prompted by a read of the JABCode reference implementation (`docs/robustness-ideas.md`).
Every change below was measured with the §15 harness before and after; the full figures
and the ideas that measured worse are in [`docs/decisions.md`](decisions.md).

- **D22, glare.** Luma is read a second time against its own neighbourhood: an additive
  plane plus a level step, checked against the palette after de-glaring so an occluder is
  not mistaken for a level. Specular-radius m50 went from 0.34–0.37 to beyond the swept
  range on every profile.
- **D23, ring geometry.** Edges fitted by consensus instead of trusting hull corners, and a
  one-coefficient lens model estimated from how the edges bow. A patch or highlight on one
  ring corner went from 0% to 100%; v32 went from failing at 2% lens distortion to decoding
  at 10%.
- **D24, blur.** After the CRC rejects a frame, neighbour bleed is measured and subtracted
  using the decoded luma levels. Blur radius 5–7 went from 0% to 100% on `C1` and `C3`.
- **D25, blur further.** `C2` included, turbo rounds (equalise against the decoder's own
  posterior), equalisation for `FrameAccumulator` (which had decoded 0% under any blur),
  a two-rung kernel fit, and a defocus fallback for ring acceptance. At blur radius 9, 48
  trials: `C1` 88%, `C2` 98%, `C3` 79%, from 0%.
- **D26, display.** `transfer.html` gained an integer-scaled full-screen presentation mode.
  A v40 `C3` transfer that failed once moved to its own page now decodes; the cause was
  the page scaling the symbol by a non-integer factor.

**Scanner budget raised** from 200 KB to 250 KB raw (256,000), 125 KB gzipped, for D23–D25.
The scanner is at 222,107 bytes, 86%.

**Still unmeasured on real captures**: D22's glare gain face-on, D23's lens model, and
D24/D25 against real defocus. The harness's blur is a box and its glare is neutral white.

---

## First real-world decode — 2026-09-04

**A laptop screen displayed a symbol and an iPhone decoded it**, through `web/display.html`
and `web/index.html` over a `cloudflared` tunnel. Until this point nothing in the project
had ever seen a photograph: every figure — the operating envelope, the profile comparison,
D10's rate validation, D11's `C3` redesign — came from the §15 harness, which is code, and
which had two of its own bugs fixed the same week (the quiet-zone clipping and the
point-sampled resampler). The whole edifice rested on a simulator that had been wrong twice.

It holds. Real optics, a real panel, real glare and a real rolling shutter decoded on the
first serious attempt, which does not prove the envelope is accurate but does rule out the
failure mode that mattered: that the simulation was wrong in some way that made all of it
meaningless.

**What this does not yet establish.** One successful scan is not an envelope. The capture
conditions were not recorded, so there is no measured distance, angle, or profile behind
it.

**The rig to fix that was built on 2026-09-08 and first run on 2026-09-14** — see "First
measured envelope".

---

## The ring carries payload — 2026-09-16 (D19, D20)

**A data position's inner ring cell is now an ordinary payload module.** The §6.4 ring was
two modules deep with both layers painted the same colour; that depth is kept only where it
earns its place — the every-third reference positions and the four corners — and the rest is
payload. Worth **+15–18% at v1–v2** and ~2% at v32. In `examples/`, `membership` went v3 → v1
and `event-admission` v3 → v2. D19 has the full account.

**The detector cost was probed before the feature was built**, by repainting those cells with
unrelated colours and reading data positions from the outer layer only: clean, blur and noise
cases still correlate at 1.000, and the realistic combination falls to 0.941 against an accept
threshold of 0.85. Cushion, not function — but cushion on the hardest case, and still
synthetic.

**The wire format changed**: the grid array no longer describes a whole symbol, so the golden
vectors are schema 2 and carry a `ring` array.

**`wasm-opt` is now part of the component build** (D20), which took the scanner from 101% of
its size budget to 89% and made decoding 14% *faster*. It is optional: without binaryen the
build prints a note and carries on.

---

## Finder-less symbols — 2026-09-15 (D17)

**The finders are gone from screen symbols.** A colour symbol that carries the §6.4 ring has
no finders, separators, anchors, alignment marker, timing or format region; the ring locates
it, names its version and orientation, and carries its format. The full account is D17 in
[`docs/decisions.md`](decisions.md).

Every ringed colour symbol in `examples/` shrank one or two versions — `contact` v2 → v1,
`allergen-profile` v3 → v1, `event-admission` v4 → v3 — and each border-less twin sits beside
its ringed one for comparison. Profile `M` and border-less symbols keep the classic layout.

**The trade** is that a greyscale capture cannot read a finder-less symbol, because its
format rides on hue.

**The gap between ring and grid is now one module, not two** (D18, 2026-09-16). It buys area
— 11.8% of a v1 symbol, 10.5% of a v2 — or, at a fixed size on screen, ~6% more pixels per
module, which is range. It costs ~10% of the defocus envelope: blur radius ~0.30 modules
where it was ~0.33, and the reason is geometric, not optical (D18 has the table). Every
version's ring got shorter, so all 32 position sequences were regenerated.

**The scanner is at 99% of its raw budget** (203,741 / 204,800; 82% compressed), after
cutting 34 KB back out of a detector that first landed at 116%. That leaves 1,059 bytes, so
the next feature needs a saving first. The best lead is about 11 KB of float formatting that
some runtime-reachable `Debug` impl pulls in — see D17.

**What to check first on a real capture.** The ring detector treats each connected region of
ink as one symbol. On a clean display stage that holds; in a real scene it is untested.
`cargo test -p lcr-decode --release --test ring_diag -- --ignored --nocapture` traces every
detection stage and is the place to start when a capture fails.

---

## Reclaiming the finder area — groundwork, 2026-09-13

*Superseded 2026-09-15 by D17 and the section above: the removal this section describes as
still needed is done.*

The three 7×7 finders plus separators cost a **fixed 192 modules**, and the palette anchors
another 12, at every version. Measured from the real layout:

| v | side | finders+sep | anchors | payload | fixed detection cost |
|---|---:|---:|---:|---:|---:|
| 1 | 21 | 192 | 12 | 167 | **46.3%** |
| 3 | 29 | 192 | 12 | 526 | 24.3% |
| 5 | 37 | 192 | 12 | 1038 | 14.9% |
| 12 | 65 | 192 | 12 | 3838 | 4.8% |
| 32 | 145 | 192 | 12 | 19718 | **1.0%** |

At v1, 62% of the symbol is fixed overhead once the 60-module format region is counted.
By v12 it is noise. **So this is a small-symbol problem**, and any redesign should be
judged on v1–v5.

**The §6.4 border can carry detection, and the objection I expected doesn't hold.** §12.1
requires detection from luma alone, and §4.1 makes hue states iso-luminant by design — so
a palette-cycling ring ought to be invisible in luma. It isn't, because P12 puts white and
black in the cycle and the two luma levels are far apart: `C1` binarises to `DDLLLD` with
full 0–255 contrast, `C3` to `DDDDLLLLLD`.

`just debruijn` writes [`docs/border-sequence.md`](border-sequence.md): a cyclic
sequence per version in which **every window of `k` cells is unique**, so the same cells
become an absolute position reference — orientation, which edge, how far along — for no
extra modules. Verified by brute force over the extracted cyclic windows rather than
trusted to the construction. Max run capped at 4 so the ring still carries module pitch;
balance ~50%; colour rides on top with all six border colours still evenly present
(`[37, 36, 36, 36, 35, 36]` at v1).

**Why a distributed sequence rather than a gap or a marked corner.** §3 makes specular
glare the dominant screen impairment and §12.3 already discards border references a
highlight has ruined. A localised asymmetry is a single point of failure at exactly the
wrong scale; a gap is worse still, because occlusion *produces* gaps — it signals with the
same alphabet the channel uses for damage.

**Minimum window Hamming distance is 1, and that is not the number to judge it on.**
Separating `p` windows by distance 3 is sphere-packing-bound at `p·(1+k) ≤ 2^k`, needing a
12-cell window at v1 and 15 at v32. But a decoder correlates whatever arc is unobscured
against all rotations rather than looking up exactly `k` cells. Measured that way:

| v | ring cells | clean | 1 misread | 2 misreads | worst case as % of ring |
|---|---:|---:|---:|---:|---:|
| 1 | 216 | 9 | 17 | 21 | 9.7% |
| 5 | 344 | 10 | 20 | 21 | 6.1% |
| 12 | 568 | 11 | 19 | 23 | 4.0% |
| 32 | 1208 | 12 | 20 | 25 | 2.1% |

So even with two misread cells, **2–10% of the ring surviving is enough to localise**.

**The sequence is now live, and the border is mandatory on screen** (D14). `cell_color`
reads the committed table, so renderer and decoder inherit it from one definition;
`just gallery` shows the irregular ring in `examples/`. Golden vectors were unaffected,
confirming the border sits outside the canonical wire form. Scanner cost: 197,663 →
199,453 bytes, **97% of budget**, leaving 5,347.

**The finders are still there.** This is the groundwork, not the removal — detection is
still the 1:1:3:1:1 run-ratio scan, so nothing can localise from the ring yet and a
finder-less symbol would decode nowhere. What is still needed: a ring detector (initial
*location* wants quad detection rather than the 1-D scan, which is where Data Matrix is
reckoned weaker than QR at low resolution), and a decision on whether to keep one finder as
a cheap locator — which reclaims 128 of 192 modules without replacing the detection
primitive at all.

**Why the ring might beat QR at low resolution, and it is a real argument.** QR's finder is
a ~7-module local signature; the ring is a 112–608 position *known* sequence. Matched
filtering integrates over all of it, which is 12–19 dB more integration than a bullseye
offers. That is the mechanism by which a sequenced border should out-detect a finder as
resolution falls — you are correlating against far more known structure. The cost is search
space (position × scale × rotation), and the mitigation is a cheap 1-D pre-filter: a scan
line running *along* a border band produces a long stretch of runs in 1..4 at consistent
pitch, which almost nothing else in a natural scene does. **Untested, and stated as a
hypothesis** — QR's advantage is also twenty years of tuned implementations.

**Sequenced after the capture sweep, deliberately.** Changing detection before the sweep
runs would leave the new detector comparable only against simulator numbers.

---

## Dual-render, and a detector bug it uncovered — 2026-09-10

**§17.2 dual-render is implemented**, in `lcr-render` behind a default-off `qr` feature:
`render_dual_rgb` / `render_dual_png` compose an LCR symbol and a QR carrying a fallback URL
at **one shared module pitch**, so the fallback does not stop working before the thing it
backs up. Off by default because `lcr-render` is in the wasm full build (~23 KB), and
because §17.2 itself calls this "a bridge, not an answer" — it trades the offline property
for reach, which a deployment should choose rather than inherit. `lcr-wasm/compare` is now
the same code behind a second flag, so the density comparison and a rendered dual share one
encoder and one set of round-trip tests. Full argument in D13.

**Writing it found a real decoder bug.** §6.2 adopts QR's 1:1:3:1:1 finder deliberately, and
a finder carries no identity — so a frame with two codes offers six finder-like patterns,
and at a shared module pitch a triple *straddling* them can outscore either real triple.
Detection committed to the single best-scoring triple with no way back, so a dual-render —
or two LCR symbols on one poster — returned `FormatUndecodable` on a frame containing two
good symbols.

`Decoder::decode` now walks candidates and **the CRC decides**, per §9.1. The first attempt
accepted on the format region instead, which looked reasonable and immediately broke
noise rejection: BCH(30,15) corrects three errors, so a random 30-bit word decodes ~14% of
the time and six candidates turn "found nothing" into "found something". `probe` and
`FrameAccumulator` therefore still use the best candidate only — neither has a CRC to check
a guess against. Six candidates also doubled the §15 suite (33.5 s → 67.2 s) on failing
frames; three recover all of it and still fix the case.

Known limitation, recorded rather than fixed: `FrameAccumulator` can still lock onto a
straddling triple in a two-code frame until reset.

---

## QR comparison — added 2026-09-09

`display.html` now draws a QR of the same payload beside the LCR symbol, at the same module
pitch, through the same rasteriser. Generation is the **`qrcodegen`** crate; the test suite
reads the output back with **`rqrr`**, an independent decoder, at every ECC level and
across a multi-block symbol — so the comparison rests on a verified QR rather than on a
hand-rolled one. `codec.json-to-cbor` was added alongside it so QR can be handed the exact
bytes LCR Code puts on the wire, which keeps §9.4's container win out of a carrier
comparison.

**Neither shipped artifact carries QR.** It is a third build,
`dist/lcr-code-compare.wasm`, produced only by `just web`, and
`the_shipped_components_export_no_qr` asserts the scanner and the full build stayed clean —
so "off by default" is a fact rather than a comment.

**The result is more equivocal than the README claimed, and the README now says so.** On a
50-byte CBOR payload against `C1` at ECC level 1, the area ratio runs 1.00× at QR-L, 1.29×
at QR-M, 1.63× at Q and 2.00× at H. LCR Code is level with QR at its weakest error
correction and pulls ahead as QR's strengthens. The 1.73× bits/module figure is a different
measurement and was never the area figure a user sees.

---

## The decoder had an upper pitch limit — found 2026-09-14, fixed 2026-09-15 (D16)

**Captured module pitch above ~22 does not decode at all**, and this is the ceiling on
working range. Found by raising the sweep rig's capture width to 4K, which tripled the
captured pitch and broke decoding completely at every distance.

Reproducible on clean renders, identical at v2/v4/v8 (so it is pitch, not version):

```
  px   8 10 12 14 16 18 20 22 24 26 28 30 32 34 36 38 40
       +  +  +  +  .  +  +  +  .  .  .  .  .  .  .  .  .
```

Shifting the image by a single pixel — same pitch, different alignment between the 8 px
binarisation grid and the module grid — separates it into **two distinct faults**:

```
pitch | pixel offset 0..7
   16 |  .  +  +  +  +  +  +  +     <- alignment only
   24 |  .  +  +  +  +  +  +  +     <- alignment only
   30 |  .  .  .  .  .  .  .  .     <- hard wall
```

1. **Grid alignment, at exact multiples of `BLOCK`.** 16 = 2×8 and 24 = 3×8 fail only when
   the grids coincide, and a one-pixel shift fixes them. **This is D6's mechanism
   recurring**: D6 diagnosed and fixed it at 1×`BLOCK`, and 2× and 3× were never tested.
2. **A hard wall above ~24**, which no offset rescues. At pitch 30, `find_finders` locates
   two of the three finders and reports a module estimate of 26 against a true 30. Cause
   not yet established; the binariser's dark fraction stays healthy at ~34%, so this is
   detection rather than thresholding.

**Why it matters more than the 2-foot range figure.** Range is set by how many sensor
pixels span a module, so more capture resolution is the obvious lever — and it is currently
unusable, because raising it walks straight through this ceiling. Fixing it is what turns
resolution back into a range control.

**Fixed by retrying the finder search on a halved image** — two levels, each built from the
last. Only the finder search sees the reduced image; the alignment search, transform and
sampling all stay at full resolution. On clean renders, 11 pitches × 8 grid phases, **80 of
88 cells decode** across 8–80 px/module, where every pitch from ~24 up used to decode at none.

Two residuals, recorded rather than hidden: pitch 30 decodes at only 4 of 8 phases, and at
pitch ≥ 32 *with the grids exactly in phase* detection now succeeds but the format region
fails its BCH decode — the same alignment fault one stage later. The regression test says
exactly which cells it covers.

**Cost, timed interleaved at 1280×720:** nothing on a successful capture (it returns at
full resolution) and nothing on a noisy empty frame (spurious finders end the scan early);
**+8.5 ms (+38%) on a clean empty frame**, where every level has to fail. Scanner +1,550
bytes, now 88% raw / 73% gzip.

**Resolution is a range lever again.** `sweep.html` re-enables 1920/2560/3840 with their
decode cost on the label. **2560 is the one to try** — ~2× range for ~1.8× decode time —
but it is unmeasured on a real capture, so run it as its own labelled run.

**Found on the way, and larger than the pyramid:** on a noisy frame with no symbol,
`Decoder::decode` spends 41 ms beyond the scan walking D13's candidates through format
reads on a spurious triple. That is the better target for live-preview speed. Not
addressed.

---

## First measured envelope — 2026-09-14

`sweep1.json` and `sweep2.json` (in `docs/data/capture-sweeps.tar.zst`), 4,207 trials, single-frame, `C1` and `C3`, one
iPhone against one laptop panel. **The first numbers in this project that did not come from
the §15 harness.** Read with `just sweep sweep2.json`.


### The tilt target is obliquity 1.30–1.50, and an angle is the wrong way to say it

Pooled over **1,549 `C1` trials** from sweeps 1–4 (`docs/sweep{1,2,3,4}.json`):

| obliquity | 1.00–1.05 | 1.05–1.10 | 1.10–1.20 | 1.20–1.25 | 1.25–1.40 | **1.40–1.60** | 2.00+ |
|---|---:|---:|---:|---:|---:|---:|---:|
| n | 451 | 437 | 230 | 67 | 225 | 116 | 12 |
| decoded | 5% | 37% | ~40% | 58% | ~71% | **84%** | 0% |

The shape survives restricting to a single pitch band (7–9 px/module: 2% → 78%), so it is
tilt and not distance. Past 2.00 it collapses — foreshortening eventually costs more than
the reflection did.

**Sweep 4 is the evidence that an angle is the wrong instruction.** The operator held a
careful 10° as §13.2 recommends, landed at obliquity 1.05–1.18, and decoded *worse* than
sweep 3 — no `C1` bin reached 50% with enough trials to count. A fixed angle produces
different obliquity at different distances, so the number to aim at is the one the page
already measures. `sweep.html` now guides to 1.30–1.50 and warns past 1.90.

### Face-on capture was the dominant failure mode, not distance

At **matched pitch**, so this is not distance in disguise (`C1`, sweep2):

| px/module | face-on (<1.15 obliquity) | tilted (≥1.35) | difference |
|---|---:|---:|---:|
| 6–7 | 5% (42) | 88% (17) | **+83 pts** |
| 7–8 | 4% (74) | 93% (14) | **+89 pts** |
| 8–9 | 22% (142) | 80% (35) | **+58 pts** |
| 9–10 | 37% (103) | 86% (36) | **+49 pts** |
| 10–11 | 29% (49) | 100% (9) | **+71 pts** |

Checked for a time confound: in sweep2's final quarter — same minutes, same conditions —
tilted decoded **92.7%** against face-on's **15.4%**.

**This is §13.2 being right.** It already recommends 10–20° off-axis so the phone's own
reflection lands away from the symbol; held flat at a glossy laptop panel, the phone
photographs itself. The operator's impression that "glare did not work at all" and the
face-on data are the same finding: **the face-on dataset _is_ the glare result.**

Tilted, `C1` decodes at **6–7 px/module** — below §13.1's floor of 8. The reported ~2-foot
limit was mostly a glare limit, not a resolution one.

### `C3` is materially worse, exactly as predicted

`C3` needs ~14 px/module against `C1`'s 6–7. **D10 and D11 jointly predicted this** —
chroma's handicap is spatial bandwidth, and D11 put 60% of `C3`'s payload there. Real optics
agree with the simulator on this one, which is the first independent support either decision
has had.

### What was wrong with the rig, and is now fixed

- **The page asked the camera for 1280px-wide video.** Captured pitch scales linearly with
  sensor pixels spanning the symbol, so that alone cost ~3× of working distance before
  anything else. Capture width is now selectable to 3840 and defaults to 1920; every trial
  records `frameW`/`frameH` so a log says what it actually measured.
- **The scanner gave no guidance.** It now uses `probe`'s obliquity to say *"tilt the phone
  10–20° off-axis — your own reflection is on the symbol"*, which is what §14.3 keeps
  diagnostics for and, on this evidence, the most valuable thing on the page.
- **`just sweep` now prints the face-on/tilted split** at matched pitch, so the effect
  cannot hide behind a distance average again.

### Caveats, which are not small

- **Session metadata was left blank in sweeps 1–3** — sweep 4 records it properly (iPhone 17
  main camera, MacBook Air M1 at maximum brightness, warm indoor light at night).
- **Sweep 3 captured 720 px across, not 1280.** The control said 1280 and the camera
  delivered `720×1280` — the portrait form of that mode, with the symbol spanning the
  *short* axis. Held upright, a phone throws away 1.78× of the captured pitch and nothing
  on screen said so. `frameW`/`frameH` are now recorded and shown live, with a warning that
  fires on any portrait frame; sweep 4 was 491/563 landscape as a result.
- **The landscape gain is not visible in the logs, and cannot be.** Sweeps 3 and 4 have
  near-identical pitch distributions (median 8.2 both) because the operator naturally
  stands where it works — the gain shows up as *more distance at the same pitch*, and
  distance is not recorded. Confirming it needs a fixed-position A/B: stand still, rotate
  the phone between portrait and landscape, and read the pitch off the live display. The
  ratio should be ~1.78.
- **All four protocol runs were recorded under one label**, so glare and angle were never
  isolated. The geometry still separates them, but by luck rather than design.
- 57–64% of frames located no symbol at all, and those carry no geometry, so they cannot be
  binned. The `not found` share is a floor on how much was simply out of range.

---

## The capture sweep — rig built 2026-09-08, first run 2026-09-14

Everything needed to turn the single scan above into an envelope exists, and **it has now
been run once** — see "First measured envelope" for what came back. What follows describes
the rig itself.

[`docs/capture-sweep.md`](capture-sweep.md) is the procedure: four runs — `C1` by
distance, `C3` by distance, `C1` by angle, `C1` under glare — with the display and capture
settings §13.1 and §13.2 require.

**The rig measures itself, which is what makes it cheap to run.** Distance is recorded as
*captured module pitch*, sensor pixels per module, read off the fitted homography; angle as
*obliquity*, the ratio of coarsest to finest pitch across the symbol. Neither needs a tape
measure, a protractor, or the camera intrinsics a browser will not give you, and both are
the units the decoder actually experiences — so the figures transfer between phones and
panels in a way centimetres do not.

- `web/sweep.html` — served by `just web`. Records one trial per frame with its geometry,
  bins them live so the operator can see which bins are still thin, and exports JSON.
  Frame combining is **off** by default: it would make success a function of how long you
  lingered rather than of where you stood.
- `just sweep <log.json>` — reduces a log to §15's metric: success rate against captured
  pitch and against obliquity, per run and per profile, with a stated working range.
- `decoder.probe` (D12) — the piece that makes failures analysable. A failed decode
  returns a string, so a failed trial could not be binned by the conditions it failed
  under; `probe` runs §12.1–§12.3 and stops, so a frame that was *found but not decoded*
  still reports its geometry. "Not found" and "found, marginal" are different limits and
  the report keeps them apart.

**What it should settle.** The captured-pitch floor (open #14 asks for it, and only the
*display*-side figure has ever been measured); whether `C3` gives up before `C1` as pitch
falls, which is what D10 and D11 jointly predict; whether tilt tolerance really is
azimuth-dependent through real optics as it is in the harness; and what the §6.4 reference
border is worth against real glare, which nothing has measured.

Where the result disagrees with `just envelope`, the harness is what is wrong — it is the
model, and this is the measurement.

**Cost paid up front:** `probe` took the scanner from 93% to **95%** of §14.2's size
budget (192,876 → 196,104 bytes raw). D12 argues that is the right call and names it as
the first candidate to move if the budget later binds.

---

## Fixed

**`ImageView` overflowed on adversarial dimensions, and the two builds failed differently.**
Found while writing the fuzz target rather than by running it — reading the arithmetic was
enough. `width`, `height` and `stride` are all caller-supplied `u32`, and in the wasm
component they cross the ABI from whatever the host chose, so every product in the
size check is attacker-controlled.

`decode_rgba(px, u32::MAX, u32::MAX, u32::MAX)` **panicked in debug** with "attempt to add
with overflow" — which in a component is a trap that poisons the instance, and which §14.2
forbids in the hot path outright. Release did not panic, and returned the correct error, but
only because `stride < min_stride` short-circuits *ahead of* the wrapped value it would
otherwise have consulted. Reordering that condition would have turned a silently wrapped
bound into an out-of-range view.

`width · channels` is the sharper edge: it exceeds `u32` past a width of ~1.07e9, and the
scanner is **wasm32**, where `usize` is 32 bits. On the build that matters most this
overflows at merely large values rather than absurd ones. `ImageView::packed` had the same
flaw in plain `u32`, where it wraps on every target.

Both are checked now, and `adversarial_dimensions_are_rejected_not_overflowed` and
`packed_rejects_a_width_whose_stride_would_wrap` pin them — alongside
`a_padded_plane_is_still_accepted`, because the risk in a fix like this is rejecting the
legitimate padded camera planes the zero-copy path exists to serve.

**Fuzzing falsified a documented claim about `json::to_json`.** The doc said "Round-tripping
JSON → CBOR → JSON is exact". It is not, in two ways the fuzzer found in about four minutes:
`-0` has no fraction or exponent so it takes the *integer* path and comes back `0`, and
`1e400` becomes an infinity that JSON cannot write, so it renders as `null`.

Neither is a defect — the text `-0` is genuinely ambiguous and every implementation picks a
side — but the comment was wrong, and it matters because the scanner now renders payloads
through this function (`codec.cbor-to-json`), so these are what a user sees. The doc is
corrected with the actual table and `the_json_projection_is_lossy_but_converges` pins it.
The real invariant is **convergence**: one pass may lose information, and from the second
pass on nothing changes.

One earlier crash was the harness's own fault and worth recording as a caution: the CBOR
target asserted value equality across a round trip, and a payload carrying a `NaN` float
compares unequal to itself. The fuzzer duly reported a failure whose two sides printed
identically. Both parser targets now compare re-encoded *bytes*, which is the invariant
that actually holds.


**The harness's tilt sweep was measuring its own resampler.** The `perspective tilt` row
was not monotone — 59% at magnitude 0.1 against 78% at 0.3 — and a fixed-azimuth magnitude
sweep read `+ . . + + .`. A mild tilt scales the image by a ratio very near 1, and
`perspective` took a single bilinear tap per output pixel, so the sample grid beat against
the 12 px module pitch. That is genuine moiré, which §15 wants a *separate* sweep of, but
here it was confounded with the quantity being swept.

`perspective` now supersamples 4×4 across each output pixel's footprint — the box filter a
sensor pixel applies, and the right model given a projective map compresses the source by a
different factor at every row. The fixed-azimuth sweep now reads `+ + + + + .`, and the
envelope row reads 66 / 72 / 69 / 34 / 9 / 16 / 3: monotone within its ±9-point error bar
where it previously was not.

**The residual spread across azimuth is real, and worth knowing.** Doubling to 8×8 changes
essentially nothing, which rules out aliasing. Failures come back as `BadFrame` /
`CrcMismatch` with a *perfect* format-region margin — so the symbol is found and the format
reads, and only the payload sampling is off. The cause is that the fixed patterns are
asymmetric: finders at three corners, only a 5×5 alignment marker at the fourth, so
tolerance genuinely depends on which corner the projection foreshortens.

Note this was measured at **v4**, and the pinned §6.2 rule gives v4 exactly one alignment
marker — interior markers only start appearing around v14, where the same rule yields six.
So the open question is not the placement rule (that is settled, open #9) but whether tilt
tolerance improves with version once those markers exist. Untested, and cheap to test:
`just profiles` and the tilt rows of `just envelope` both currently pin v4.

Cost: `just envelope 32` went from ~1 min to ~5, essentially all of it in the two tilt
rows. It is an on-demand tool and not part of `just ci`, so this was judged worth paying.
Two earlier fixes in the same function: the canvas sizing used to clip the quiet zone at
low tilt, and the original sweep tested four azimuths out of 360.

**Both palettes shipped with a gamut-clipped colour, and the assertion that would have
caught it was compiled out.** D1 derives the palettes on three rules — iso-luminant hue
states shared across levels, a single common `C*`, and a `C*` the sRGB gamut reaches at
every corner. The third was never checked against the values pinned for the first two.
Neither held: at the original 316° magenta the gamut stops at `C* = 47.9` for `L* = 78`,
against a requested 55, and at the original 149° it stops at 39.4 for `L* = 30` against a
requested 40. C4 therefore shipped as 55.1 / 55.0 / 55.3 / **48.1**.

That is the exact failure the common-`C*` rule exists to prevent. §12.4 demodulates chroma
against one hue centroid per state shared across both luma levels; a clipped corner pulls
its state's centroid toward the neutral axis, so the states stop being equidistant from it
and the per-bit chroma LLRs carry a bias no ECC can see.

`derive` did check this, as a `debug_assert!` — and it had been failing since the palettes
were pinned. `just test` passes `--release`, which strips `debug_assert!`, and that is the
only command anyone runs, so a red assertion sat invisible behind a green suite.

The fix turned out to be one number. `C* = 55` is not too ambitious; **316° is the wrong
angle.** At 326° the gamut clears 55 at both levels with `L*`, `C*` and the green state
untouched — the inter-level gap stays at 45.8 and all four colours land at `C* ≈ 55`. C8's
hues moved the same way, to 71°/146.5°/280°/355.5°.

Three things came out of it:

- `xtask palette` (`just palette`) — the search D1 and the module docs already cited as
  the source of these numbers, which did not exist. It maximises common achievable `C*`
  over hue angles and `L*` levels subject to §4.2, and reports two trade-offs §4.2 leaves
  open: what luma gap costs in chroma (~0.85 of `C*` per unit of gap at k=2), and what
  each hue orientation is worth, so D1's green-against-magenta choice can be priced
  rather than assumed. Green sits at 136° across the entire trade curve, which is a point
  in that choice's favour.
- `no_shipped_colour_is_gamut_clipped` — the same check as a **release** test, confirmed
  red against the old 316°.
- `just test-debug` — `cargo test --workspace --lib` unoptimised, so the remaining
  `debug_assert!`s are exercised at all. Wired into `just ci`. `--lib` because the
  assertions live in crate internals the unit tests reach directly, while the integration
  suites are dominated by the §15 sweep — ~5 minutes unoptimised for no extra coverage.

A full unoptimised run confirmed this was the only `debug_assert!` failing.

**Degenerate payloads broke detection at v14 and above.** LDPC is linear and systematic, so
an all-zero body encodes to an all-zero codeword and the rendered luma layer is just the
mask pattern, unmodulated by any data. For mask 0 that is a perfect module-pitch
checkerboard — which scores *best* on the §8.5 penalty function (no runs, no 2×2 blocks, an
exact 50/50 split), so the search reliably picked it.

That made it the detector's problem, and the detector failed: `matches_finder` normalised
by `total / 7`, which let a wrong-length centre run drag the module estimate toward itself.
Five equal runs of `r` give `module = 5r/7`, and the centre check `|r − 3·(5r/7)| = 8r/7`
clears its limit for any tolerance above 8/15 ≈ 0.533. At `TOLERANCE = 0.55` the margin was
3%, so a checkerboard read as a finder on every scan line — 5841 spurious candidates at
v32, wrong version estimate, no decode.

The module estimate now comes from the four outer runs, with the centre tested against it.
Guarded by `run_ratio_rejects_a_uniform_checkerboard_at_every_scale` and
`degenerate_payloads_roundtrip_across_versions_and_profiles`; both were confirmed to fail
against the old matcher.

Worth noting the existing `run_ratio_rejects_non_finders` asserted `[1,1,1,1,1]` was
rejected and passed throughout — that case is caught by the `total < 7` size guard before
the ratio test runs, so it never tested the logic it appeared to.

**§13.1's ≥ 8 px/module floor did not detect (D6).** `BLOCK` is also 8, so at that pitch
the binarisation grid coincided with the module grid: every block landed wholly inside one
module, none saw an edge, and each then inherited a threshold from an already-computed
neighbour. That is a recurrence, and a monotone one — a single dark block dragged
everything below-right of it down with no way back up. A v32 symbol that is 47.7% dark
binarised to 3.2% dark and the finder scan returned nothing.

`binarize` now classifies blocks by two levels of evidence (saw an edge / near one) and
takes the threshold from the nearest level available, so no block inherits another block's
result. The statistic is one step of Ridler–Calvard — split the block means at their own
mean, average each side, take the midpoint — which is what the second level needs, since
those blocks sit wholly inside one module and a plain mean of them would track the local
dark/light balance instead of the black point.

Envelope cost: none measurable. 28/40 cells of the perspective grid decode against 27/40
before, with marginal cells flipping both ways. `survives_perspective` was pinning named
cells in that marginal band — which measures luck — and now asserts the structural cases
individually and the rest as an aggregate, per §15. Guarded by
`decodes_at_the_normative_minimum_module_pitch`, confirmed red against the old binarizer.

---

## Notes on the build

- **Edition 2024** throughout. `rust-version = "1.94"` is the MSRV — the floor claimed for
  consumers — while `rust-toolchain.toml` pins the toolchain the repo itself builds and
  gates with to **1.98.0**. Two different things: the first is a promise, the second is
  what CI actually runs, and only the second is enforced.
- `lcr-core`, `lcr-decode`, `lcr-verify` are `no_std + alloc`; `lcr-encode`, `lcr-render`,
  `lcr-cli` are `std`. This is the §14.1 split, and it is what keeps the scanner bundle
  from accidentally acquiring the encoder.
- No C dependencies anywhere. `miniz_oxide` for DEFLATE, and the PNG encoder is ~40 lines
  rather than a dependency, because §14.2 gates `image` out of the wasm path.
- `panic = "abort"` in release; a `wasm-min` profile is defined but not yet exercised.
- LDPC base graphs are **generated offline and committed** (§8.2: "do not generate at
  runtime"). `cargo run -p xtask -- ldpc > crates/lcr-core/src/ecc/tables.rs`.
- The degradation harness lives in `crates/lcr-decode/tests/support/ops.rs` and is shared by
  the `degradation` test and the `envelope`/`diagnose` examples via `include!`, so the
  sweep that establishes the envelope and the suite that guards it cannot disagree.
