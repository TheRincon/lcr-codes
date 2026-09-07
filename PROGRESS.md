# LC Code — implementation progress

Reference implementation of [`lc-code-spec-v0.4.md`](lc-code-spec-v0.4.md).

**State:** encode and decode are both complete and round-trip end to end through a
rendered image — and, as of 2026-09-04, **through a real iPhone camera pointed at a real
laptop screen** (see "First real-world decode") — detection, illumination fit, soft demodulation, LDPC, framing, CRC. The
§15 degradation harness is built and the operating envelope is mapped — as a pass/fail
grid, not yet as the success *rate* §15 asks for. `lc-wasm` ships
both paths as a **WebAssembly component** with a WIT interface (D8), in two builds:
decode-only and full.

**Provisional, pending review:** the palette (D1/D2) and illumination-model (D3) changes in
[`docs/decisions.md`](docs/decisions.md) are being carried forward so work can continue,
not signed off. Both are cheap to unwind — see "Unwinding the provisional decisions" below.

**265 tests, all passing, zero warnings.** `just test` (plus `just test-debug`, 217 unit
tests unoptimised — see below).

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
just test          # 265 tests; depends on `component`
just test-debug    # 217 unit tests unoptimised, exercising debug_assert!
just c-host        # the C trial, needs the Go module cache (see below)
```

### Things a restart may take with it

- **`dist/`** — build output, not tracked. `just component` regenerates it in ~30s.
- **`~/go/pkg/mod/.../wasmtime-go/v34@v34.0.0`** — `just c-host` borrows the
  component-capable `libwasmtime.a` bundled there (D9). If it is gone:
  `go get github.com/bytecodealliance/wasmtime-go/v34`, or point `WASMTIME_C_API` at an
  unpacked wasmtime-c-api release.

### What to pick up next

**§8.2 ECC rate tuning is done, and the rates did not move.** The full argument is D10 in
[`docs/decisions.md`](docs/decisions.md); the short version is that the table was right and
its stated reason was not.

- `just envelope` reports success *rate* per cell (§15's metric) with a clean control row.
- `just thresholds` writes [`docs/ldpc.md`](docs/ldpc.md) — each rate step buys ~1 dB, and
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
fork. The scanner sits at **93% of its size budget** measured raw (D8), so that gate wants
to be automatic rather than trusted to a hook someone can `--no-verify` past.

---

## Where things stand

| Crate | State | Tests | Notes |
|---|---|---|---|
| `lc-core` | ✅ Complete | 139 | Geometry, palette, ECC, container, masking, modulation |
| `lc-encode` | ✅ Complete | 11 | Full §8 pipeline; auto version, auto compression, mask search |
| `lc-render` | ✅ Complete | 12 | SVG, RGB raster, PNG, ANSI terminal preview |
| `lc-decode` | ✅ Complete | 79 | §12 pipeline end to end, plus the §15 harness |
| `lc-verify` | ⬜ Stub | — | §10 signing, entirely untouched |
| `lc-wasm` | ✅ Complete | 22 | Component Model surface over `wit/lc.wit` (D8) |
| `lc-ffi` | ⬜ Stub | — | |
| `lc-cli` | ⬜ Stub | — | |
| `xtask` | ✅ Working | — | `ldpc` regenerates base graphs, `capacity` recomputes §6.1 |

`lc-decode`'s 79 are 55 unit, 12 round-trip, 12 degradation.

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
```

The full path — `encode → render → decode` — round-trips across every profile, every
version, every mask, every ECC level, and every compression setting. Dropping the chroma
layer entirely still recovers a complete prefix of the packet, per §8.1.

[`examples/`](examples/) holds a rendered symbol for each payload below — PNG, SVG and the
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

**[`docs/decisions.md`](docs/decisions.md)** — every `[TBD]` pinned, and **nine places the
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

- [x] **Component-level integration tests.** `lc-wasm/tests/component.rs` loads both
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

`lc-verify`: COSE_Sign1, CWT claims, ES256 + Ed25519, `KeyResolver`. §1.2 is explicit that
this is a feature and not the point, so it sits behind the decode path. `lc-encode/sign.rs`
is a placeholder.

Worth doing carefully when we get there: test against externally generated vectors, not
just our own encoder — self-consistent crypto bugs are the dangerous kind.

### 4. Everything else

- [ ] `lc-cli` — encode/decode/inspect dev tool.
- [ ] `lc-ffi` — C ABI. Note edition 2024 needs `#[unsafe(no_mangle)]`.
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
      component with `lc-decode/brotli` takes it from 192,876 to 392,293 bytes. The codec
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
      suit `lc-core`'s `no_std + alloc` shape, but need a `BrotliAlloc` and custom IO
      wrappers rather than the one-line `BrotliCompress` the standard path uses. Note also
      that brotli ships no trainer; `zstd --train` is the usual way to *discover* the
      substrings, which are then used as brotli dictionary bytes.
- [ ] Remaining §15 screen impairments: PWM banding, subpixel geometry, a moiré sweep.

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
it. The obvious next step is cheap now the rig exists: sweep distance and angle by hand,
try `C1` against `C3`, and write down where it stops working. That is the first data in
this project that is not self-referential, and it is worth having before the wire format
freezes.

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

- **Edition 2024** throughout, `rust-version = 1.94`.
- `lc-core`, `lc-decode`, `lc-verify` are `no_std + alloc`; `lc-encode`, `lc-render`,
  `lc-cli` are `std`. This is the §14.1 split, and it is what keeps the scanner bundle
  from accidentally acquiring the encoder.
- No C dependencies anywhere. `miniz_oxide` for DEFLATE, and the PNG encoder is ~40 lines
  rather than a dependency, because §14.2 gates `image` out of the wasm path.
- `panic = "abort"` in release; a `wasm-min` profile is defined but not yet exercised.
- LDPC base graphs are **generated offline and committed** (§8.2: "do not generate at
  runtime"). `cargo run -p xtask -- ldpc > crates/lc-core/src/ecc/tables.rs`.
- The degradation harness lives in `crates/lc-decode/tests/support/ops.rs` and is shared by
  the `degradation` test and the `envelope`/`diagnose` examples via `include!`, so the
  sweep that establishes the envelope and the suite that guards it cannot disagree.
