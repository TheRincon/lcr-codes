# Robustness ideas — what JABCode does, and what we could take from it

*Written 2026-10-03. A read of the vendored `jabcode/` reference implementation
(Fraunhofer SIT's `libjabcode`), compared against `lc-decode` as it stands after D20. No code
changed. This is a list of ideas to evaluate, not decisions.*

Each idea says where it comes from, which of **our measured** failure modes it targets, what
it would cost, and how we would know it worked. The measured failure modes it is ranked
against:

| Evidence | Source |
|---|---|
| Face-on capture decodes **5%**, tilted to obliquity 1.4–1.6 decodes **84%** — the face-on data *is* the glare result | PROGRESS.md, "First measured envelope" |
| **57–64%** of sweep frames located no symbol at all | PROGRESS.md, caveats |
| `C3` needs ~14 px/module against `C1`'s 6–7; chroma is bandwidth-limited | D10, D11, sweep |
| The ring detector assumes one ink region = one symbol, untested on real scenes | D17 limits |
| Finder-less geometry comes from **four corner points only**; corner error under blur already costs a step of the defocus envelope | D17, D18 |
| `FrameAccumulator` keeps stale geometry until reset; `receive.html` resets on a 24-frame heuristic | `lc-decode/src/lib.rs`, `web/receive.html` |
| The scanner sits at **89%** of the raw size budget | D20 |

---

## 1. How JABCode works

### 1.1 Colours

- **Palette at the RGB cube corners.** The default 8-colour palette is black, blue, green,
  cyan, red, magenta, yellow, white (`encoder.h:20`). Every channel is either fully on or off,
  so a colour can be classified one channel at a time. Up to 256 colours are defined; above
  64, only 64 are embedded and the rest are interpolated (`decoder.c:62`).
- **The symbol carries its own palette, four times.** `COLOR_PALETTE_NUMBER = 4`. The first
  two entries of each copy live inside the four finder patterns; the rest sit in metadata
  modules near each finder (`decoder.c:202`). Each module is classified against the copy
  **nearest to it** (`getNearestPalette`, `decoder.c:370`). That is a crude, piecewise-constant
  spatial colour model, with one palette per quadrant.
- **Classification is brightness-normalised nearest-neighbour** (`decodeModuleHD`,
  `decoder.c:409`):
  1. Black is tested first, against per-channel thresholds derived from the measured palette
     (`getPaletteThreshold`, `decoder.c:561`).
  2. The sample is divided by its **max channel**, which makes it invariant to brightness and
     shading, and compared against the palette normalised the same way.
  3. Normalisation makes black and white identical, so if the winner is black or white, a
     second test on the RGB sum separates them.
- **Bootstrap without a palette.** Metadata Part I (the colour count) is written using only
  black, cyan and yellow, which `decodeModuleNc` (`decoder.c:519`) reads with fixed rules and
  no palette. If Part I fails, the decoder **falls back to default metadata** and tries anyway
  (`loadDefaultMasterMetadata`, `decoder.c:1130`).
- **Hard decisions throughout.** The LDPC decoder that actually runs is hard-decision bit
  flipping (`decodeLDPChd`, `ldpc.c:653`). A soft belief-propagation decoder exists
  (`decodeLDPC`, `ldpc.c:1172`), but nothing calls it.

### 1.2 Pre-processing and binarisation

- `balanceRGB` (`binarizer.c:502`): a **global per-channel histogram stretch** to the
  min/max values that more than 20 pixels reach.
- `binarizerRGB` (`binarizer.c:604`): produces **three binary images, one per channel**. A
  pixel is black if all three channels fall below the block average (the image is cut into
  roughly 2×2 blocks). It is white if its normalised RGB standard deviation is below 0.08.
  Otherwise the max channel is set to 1, the min to 0, and the middle channel is decided by a
  ratio test. A 5-tap majority filter runs horizontally, then vertically (`filterBinary`).
- `binarizer` (`binarizer.c:408`), a single-channel local threshold: 32 px blocks, a black
  point averaged over a 5×5 neighbourhood, and a global histogram-valley fallback for small
  images. It is the zxing approach, the same family as ours before D6.

### 1.3 Detection

- **Four finder patterns, each one distinct.** FP0–FP3 are concentric coloured squares with
  different core colours. The R, G or B binary image in which the 1:1:1 bullseye shows up also
  identifies **which corner** a finder is. One finder therefore carries its own orientation,
  and two symbols in one frame cannot be cross-wired.
- **Scan, then cross-check across channels and directions** (`findMasterSymbol`,
  `detector.c:1572`):
  1. Row scan in the green channel.
  2. Cross-check horizontally in blue or red.
  3. Colour check in the third channel.
  4. Vertical, horizontal **and both diagonal** cross-checks (`crossCheckPatternDiagonal`).
     The diagonal check also yields a rotation hint.
- **Strict candidate selection.** A candidate needs `found_count ≥ 3`. Any finder confirmed
  fewer than half as often as the best one is discarded. Outliers by module size are removed.
  A vertical scan runs **only** when the row scan found just the top pair or just the bottom
  pair (`detector.c:1709`).
- **One missing finder is estimated, then re-searched locally** (`detector.c:1745`,
  `seekMissingFinderPattern` at `detector.c:1338`). The missing one is placed by
  parallelogram completion from the other three. A window of five module radii around that
  point is re-quantised using **that window's own average** as threshold, and searched again.
- **Two or more missing: re-binarise and retry** (`detectMaster`, `detector.c:3337`). The
  average colour around the finders that *were* found becomes the threshold for a second
  `binarizerRGB` pass over the whole image.
- **Version from geometry, confirmed by structure.** Side size comes from finder spacing and
  module size, snapped to valid sizes with a reliability flag (`getSideSize`). In default
  mode it is confirmed by locating the first alignment pattern and checking that its position
  matches that version (`confirmSymbolSize`, `detector.c:2881`).

### 1.4 Sampling — two attempts

1. One homography from the four finder centres. Each module takes a **3×3-pixel average** at
   its mapped centre (`sampleSymbol`, `sample.c:31`). The kernel is fixed in pixels and does
   not scale with module size.
2. **If that decode fails**, fall back to sampling from **alignment patterns**
   (`sampleSymbolByAlignmentPattern`, `detector.c:2944`):
   - Each alignment pattern is predicted from its already-found neighbours by parallelogram
     completion.
   - It is searched outward from the prediction with a doubling radius, and accepted after
     two agreeing hits (`findAlignmentPattern`, `detector.c:2269`).
   - Every cell of the alignment-pattern grid gets **its own local homography**, a
     piecewise / mesh warp.
   - A missing pattern is replaced by its estimate. The smallest rectangle of found patterns
     covering each cell is used.

### 1.5 Multi-symbol, masking, interleaving

- **Docked slave symbols.** Up to 61 symbols tile around a master, each located from its
  host's geometry. The host's data carries the slaves' metadata. `COMPATIBLE_DECODE` returns
  partial data when some slaves fail.
- **Masking.** Eight XOR-on-colour-index masks, scored by penalties for finder look-alikes in
  the data, same-colour 2×2 blocks and runs of five or more (`mask.c`). This is close to ours.
- **Interleaving** is a seeded LCG shuffle (`interleave.c`).

---

## 2. Side by side

| Stage | JABCode | LC Code today |
|---|---|---|
| Palette design | RGB cube corners, per-channel binary | Lab-derived, iso-luminant hue states, luma ⟂ chroma (D1/D2) |
| Colour reference | 4 embedded palette copies, nearest-quadrant lookup | Fixed references + ring cells → spatial gain/flare surface + global affine + robust refit (D3) |
| Classification | Max-normalised RGB nearest-neighbour, hard decision | Lab LLRs scaled by measured σ, reliability-weighted (`demod.rs`) |
| FEC | Hard-decision bit flipping | Soft min-sum layered LDPC, luma first |
| Binarisation | Global stretch, per-channel RGB binaries | Luma only; local Ridler–Calvard (D6); ring uses its own percentile ink map (D17) |
| Locator | 4 distinct colour finders | Ring + de Bruijn sequence (finder-less), or 3 identical luma finders |
| Missing locator | Estimate from the others, local re-search | Ring: hull must have 4 corners. Finder path: needs all 3 |
| Detection retry | Re-binarise from found-finder colours | Pyramid on no-candidate (D16); walk 3 candidates on CRC (D13) |
| Geometry | One homography, then piecewise per-AP mesh on failure | One homography (+1 alignment marker on the finder path; 4 hull corners on the ring path) |
| Sampling kernel | 3×3 px fixed | 3×3 bilinear taps scaled to module; wider for chroma (§12.6) |
| Mono capture | Cannot detect (colour finders) | Luma path survives; finder-less format needs colour (D17) |
| Multi-frame | None | LLR accumulation (`FrameAccumulator`) |

Our pipeline is ahead on the signal-processing core: soft decisions, a physically motivated
illumination model, luma/chroma separation, and multi-frame combining. Where JABCode is ahead
is **recovery behaviour**. It assumes the first attempt will fail and has a second, more
expensive attempt for each stage. The ideas below mostly port that attitude.

---

## 3. Ideas — Tier 1: decoder-only, aimed at measured failures

None of these touch the wire format.

### 3.1 An escalation ladder for "found, not decoded"

**From:** JABCode's two-stage sampling (`detectMaster`, `detector.c:3410`) and its
Part I → default-metadata fallback.

**Why:** the sweep separates *not found* from *found, not decoded*, and the second bucket is
where extra effort pays. The symbol is in range, we hold valid geometry and format, and only
the payload failed. Right now `decode` makes one attempt per candidate and stops. A ladder
that only runs after the CRC fails costs nothing on frames that already decode, and nothing
on empty frames.

Rungs, cheapest first. Each one re-runs LDPC + CRC, and the CRC stays the only judge:

1. Geometry refinement (§3.2).
2. Decision-directed illumination refit (§3.3).
3. Glare-robust chroma likelihoods (§3.4).
4. A sub-module phase search: re-sample with the grid nudged ±0.15 module in x/y, keep the
   offset that maximises agreement with known-colour references.

**Cost:** latency on failing-but-found frames only; a small amount of code. Gate it behind a
time budget so a live preview stays responsive.

**Measure:** `just envelope` blur and glare m50, and the *found, not decoded* share in
`just sweep`.

### 3.2 Use the ring for geometry, not only its corners

**From:** JABCode's piecewise per-alignment-pattern homographies (`detector.c:2944`).

**Why:** a finder-less symbol's whole transform rests on four hull corners. D18 already
records that 2–3 px of corner error under blur costs a step of the defocus envelope. At
v32–v64, nothing in the interior pins the grid, so a phone's wide-angle **lens distortion**
or a slightly curved panel goes uncorrected. Every third ring position, however, is a
known-colour reference, and so are the scattered §6.3 references. Those are hundreds of
anchors whose expected colour we already know.

Options, in increasing ambition:

- **Edge-fit refinement.** Fit all four ring edges with a line or a low-order curve through
  every reference cell's measured centroid, not just the edge middles (`refine_quad` today).
  Intersect the edges for the corners.
- **Homography + residual warp.** After the homography, measure the offset of each reference
  cell (local template match or luma-edge centroid) and fit a smooth 2-D residual. A
  bilinear/biquadratic surface matches what `illum.rs` already does for colour.
- **Piecewise.** As in JABCode, split the symbol into quadrants and fit each quadrant's
  homography from its own ring segment and nearby scattered references.

**Cost:** moderate code, mostly reusing `least_squares`. It runs only after detection
succeeds. As a §3.1 rung, it runs only after the first decode fails.

**Measure:** perspective grid and defocus envelope in the harness. Add a **barrel-distortion
impairment** to §15, which we do not have, plus large-version captures in the sweep.

### 3.3 Decision-directed refit: decoded bits become references

**From:** the idea behind JABCode's embedded palettes (more known colours, better
classification), taken further than JABCode does.

**Why:** the illumination fit uses roughly 1.6% of the symbol's area as references (§6.3).
Once the **luma** layer has converged, which §12.5 does first, the luma level of *every*
payload module is known exactly. That makes hundreds or thousands of extra references with
known `L*`. Refit the gain surface and the luma centroids from them, re-demodulate chroma,
and run chroma LDPC again. If chroma partially converges, its high-|LLR| bits can seed one
more round (a "turbo" loop between demodulation and decoding).

This targets `C3`, the profile whose payload is mostly chroma and whose chroma is fragile.
It also targets glare: a highlight that ate several references is still covered by the
payload modules around it.

**Cost:** one extra fit plus one chroma demod/decode, only when chroma fails after luma
succeeds. That case is exactly what `Partial` exists for today.

**Measure:** `just profiles` for `C3` noise and glare m50; the multi-frame glare test.

### 3.4 Glare-robust chroma: trust hue angle when saturation collapses

**From:** JABCode's max-channel normalisation (`decoder.c:434`). It throws away brightness
on purpose, so that shading and exposure cannot move a decision.

**Why:** specular glare adds white. That lifts `L*` and **pulls `C*` toward zero while
roughly preserving hue angle**. Today's chroma likelihood is a Euclidean distance in `a*b*`
(`hue_log_likelihoods`), which penalises the lost chroma radius as if it were a hue error.
`reliability` then attenuates the module to near-erasure. That is safe, but it throws away
the hue angle, which is still informative.

Two complementary changes:

- **An angular likelihood.** In modules whose corrected `C*` is well below the palette's
  `C*`, use a von Mises likelihood on hue angle alone, with a concentration that shrinks as
  `C*` shrinks. On `C1`, with two hues 170° apart, the angle survives a lot of desaturation.
- **Explicit raw-clipping erasures.** If any tap in the sampling kernel has a channel ≥ ~250
  in sRGB, the corrected value is not just noisy, it is unrecoverable. Zero that module's
  LLRs outright instead of relying on the Lab distance to notice. Partial clipping (one
  channel) can land a sample *near* a palette entry, where `reliability` would not catch it.
- **Kernel variance as reliability.** The 3×3 taps already exist. High spread inside one
  module's kernel means an edge, blur bleed, moiré or a glare boundary. Fold it into the
  weight.

**Cost:** small: a few lines in `demod.rs` and `sample.rs`.

**Measure:** face-on sweep bins, where the 5% lives; harness glare m50.

### 3.5 Recover a ring with a corner missing

**From:** JABCode's estimate-then-local-search for one missing finder (`detector.c:1745`,
`detector.c:1338`).

**Why:** glare is the dominant impairment, and it is localised. A highlight over one corner
blows out the hull, so `quad` either fails or returns a corner on the highlight's edge. The
detector does not need that corner:

- Three good edges are fitted lines. The fourth corner is the intersection of the two edges
  adjacent to it, extended from their unaffected parts.
- When an edge is missing entirely, complete the parallelogram from the other three corners
  (JABCode's formula), then re-run the sequence correlation. The correlation is robust to a
  missing arc by design (`docs/border-sequence.md`: ~20 positions survive two misreads).
- As in JABCode, re-threshold a local window around the estimated corner with its own
  levels, and search there.

**Measure:** a new harness case, glare blob centred on a ring corner; sweep run 4.

### 3.6 Second-chance ink map

**From:** JABCode's re-binarisation seeded from the finders it *did* find
(`detector.c:3337`), and its use of chroma in detection.

**Why:** 57–64% of sweep frames found nothing. Some of those were truly out of range, but
the ink map has known weak spots: light-hue ring cells against white paper (D17 fault 1),
and a contrast floor of 80 levels. Two ideas:

- **Ink = dark *or* saturated.** A light ring cell at `L*` 78 is close to paper in luma but
  far from it in chroma. A per-cell saturation term in `InkMap` would make light hues solid
  ink without lowering the luma threshold that keeps noise out. This breaks §12.1's
  "luma-only" rule for detection, so make it a *second* attempt and leave the luma map as the
  first.
- **A seeded retry.** If the hull or correlation fails but some region scored partially, for
  instance agreement 0.6–0.85, re-derive the dark/light levels from inside that region's
  hull only, and run the ink map and correlation once more.

**Cost:** a second ink-map pass on near-miss frames only.

**Measure:** the *not found* share in the sweep; `ring_diag` cases.

### 3.7 A spatially varying colour transform

**From:** JABCode's four quadrant palettes. They are crude, but they allow **cross-channel**
colour behaviour to differ across the symbol, which our model does not.

**Why:** our spatial surface is per-channel and scalar. The cross-channel 3×4 affine is
global. But we now *tell users to tilt* to obliquity 1.3–1.5, and both LCD and OLED panels
shift colour (not only brightness) with viewing angle. Under tilt that shift **varies
across the symbol**: the near edge and the far edge are seen at different angles.

Option: fit a separate affine per quadrant (or a bilinear blend of four corner affines) when
references are plentiful, with the current global affine as the fallback. JABCode's
discontinuity at quadrant borders is the thing to avoid; blend instead.

**Measure first:** before building anything, log the per-quadrant residual of the current fit
on tilted sweep captures. If residuals don't differ by quadrant, drop this.

---

## 4. Ideas — Tier 2: multi-frame and file transfer (CFT)

This is where "encoding entire files" lives, and JABCode has nothing to offer here beyond
docked multi-symbol layouts. These matter more for us than any single-frame gain.

### 4.1 Make `FrameAccumulator` safe for animated sequences

**Why:** a file transfer cycles through symbols. The accumulator adds LLRs from every frame
against geometry fixed on the first one. If the display advances to the next chunk,
**accumulation mixes two different symbols**, which is worse than not accumulating at all.
`receive.html`'s fallback (reset after 24 frames) only papers over this.

- **Key accumulation on symbol identity.** The accumulator needs to know that frame *n* shows
  the same symbol as frame *n−1* *before* decoding. Candidates: the format word (already
  decoded per frame), the ring sequence phase, or a cheap per-frame hash of the
  hard-decision luma layer compared against the running sum's sign pattern. On a mismatch,
  start a new accumulator. Keep a small LRU of them, one per chunk seen recently.
- **Track geometry instead of freezing it.** JABCode's predicted-position, doubling-radius
  local search (`findAlignmentPattern`) is the model: re-locate the four ring corners near
  last frame's positions instead of re-detecting from scratch. That is cheap, and it removes
  the "phone moved, geometry stale" failure.
- **Weight frames by quality.** Scale each frame's LLRs by its measured SNR or illumination
  residual before adding, so one glare-ruined frame doesn't dilute ten good ones.

### 4.2 Detect display transitions and rolling-shutter tearing

**Why:** a camera at 30–60 fps reading a display that switches symbols will capture frames
that are half old symbol, half new: rolling shutter, plus panel refresh. Those frames
demodulate to confident garbage.

- **Display side:** hold each symbol for at least two camera frame periods, and alternate a
  1-bit "phase" (for example two complementary ring sequences, or a flipped reference
  pattern) between consecutive chunks.
- **Receiver side:** if the phase reads differently at the top and bottom of the ring, the
  frame is torn. Drop it, or accumulate only the half that matches.

### 4.3 Fountain / erasure coding above CFT

**Why:** CFT §10 recovers missing chunks by "rescan only those". With a looping display,
every missed chunk costs up to one full cycle. A rateless code (LT / Raptor-style), or
simply systematic XOR/Reed–Solomon parity chunks across a window, means **any**
`K·(1+ε)` received chunks reconstruct the file. The receiver stops caring *which* chunks it
missed, and a receiver that joins mid-loop finishes in about one cycle.

**Fit with CFT:** CFT.md §21 already lists "forward error correction above the transport" as
a future extension. It belongs in `lc-cft`, transport-independent. Signing and the
end-to-end hash are unaffected: the hash is still checked after reconstruction.

**Cost:** encoder and decoder code in `lc-cft`, which is not the scanner budget unless the
scanner build links CFT.

### 4.4 Multi-symbol frames (JABCode's docking)

JABCode tiles slave symbols around a master so one image carries more data, with geometry
propagated from the host. The spatial equivalent for us is several ringed symbols on one
screen, which D17 says the ring detector already handles as separate regions. Combined with
4.3, a large screen could show 2–4 chunks per frame. Measure against the cost of a smaller
module pitch per symbol before committing.

---

## 5. Ideas — Tier 3: format changes, need sign-off

### 5.1 A palette-independent bootstrap for the finder-less format

**From:** JABCode's Part I, written in three colours readable without a palette, plus its
fallback to defaults.

**Why:** D17's trade is that a finder-less symbol's format rides on hue, so a mono or
heavily colour-cast capture cannot read it. Two ways out:

- **Decoder-only blind trial.** When the ring correlates (so the version is known) but the
  format BCH fails, try the most common `(profile, ECC, mask)` combinations and let LDPC
  convergence + CRC decide. **Caution:** compact frames carry CRC-16, so *n* trials raise the
  false-accept rate to roughly *n*·2⁻¹⁶ before LDPC convergence filters anything. D13's
  argument (more candidates are fine *only* where the CRC is the gate) applies, but the
  budget of trials has to be small and justified. Require LDPC convergence on **both**
  layers as an additional gate.
- **Wire change.** Carry the format's most critical bits (profile, plus enough to locate the
  rest) redundantly in the ring's **luma** sequence phase or in the outer layer's unused hue
  slots (D19 notes 2–4% of capacity there).

### 5.2 Identifiable finders for the finder path (profile `M`, print)

**From:** JABCode's four distinct finders, which carry orientation and rule out
cross-wiring.

**Why:** D13 found that identical finders let a triple straddle two symbols. `M` and print
must stay luma-only, but finders could still differ *in luma*: a different core size, or a
notch in one ring layer, chosen so a run-ratio scan still matches. Only worth doing if print
or `M` becomes a priority, since screen symbols are finder-less now.

### 5.3 Mask penalty against ring look-alikes

JABCode's rule 1 penalises finder look-alikes in the data. If real scenes show the ring
detector latching onto a symbol's own interior (a dark run along a payload row that extends
the hull), add an equivalent penalty term. Speculative; only act on sweep evidence.

---

## 6. Ideas — Tier 4: detection details and cost

- **Stricter finder confirmation** (`found_count ≥ 3`, drop finders confirmed fewer than half
  as often as the best). D16 found the larger live-preview cost is D13's candidate walk on
  **spurious triples in noisy empty frames**: 41 ms per frame. Fewer false finders means
  fewer triples to walk. Cheap and directly measurable with `decode_cost` timings.
- **A diagonal cross-check** for finders (`crossCheckPatternDiagonal`). It rejects blobs that
  pass both axis checks but aren't concentric squares, and helps at 45° rotation.
- **A conditional second-axis scan.** JABCode scans columns only when the row scan found an
  incomplete set. We always scan both. Making the column scan conditional would roughly halve
  finder-scan cost on frames where rows suffice. Check that coverage stays the same on the
  perspective grid before adopting it.
- **Outward-from-prediction local search** (`findAlignmentPattern`: start at the predicted
  point, alternate left and right, double the radius, stop after two agreeing hits). This is
  a better shape for `locate_alignment` and for tracking (§4.1) than a fixed window.

---

## 7. What *not* to copy

Worth writing down so nobody reads JABCode as a reference for these.

| JABCode choice | Why it would be a regression for us |
|---|---|
| Hard-decision LDPC (`decodeLDPChd`) | Gives up the ~2 dB of soft decoding §12.4 cites; we already run soft min-sum |
| RGB-cube palette, per-channel binarisation | Not iso-luminant. Glare or a cast flips individual channels, so one error becomes two or three bits. D1's design exists to prevent exactly this |
| Global `balanceRGB` stretch | One specular highlight sets the max for the whole frame; our local percentile levels are the fix |
| Nearest-of-four palettes | Piecewise-constant colour model, discontinuous at quadrant boundaries; our smooth surface dominates it (see §3.7 for the part worth keeping) |
| Fixed 3×3-pixel sampling | Doesn't scale with pitch; at 20+ px/module it samples a tiny fraction of the module |
| `rand()` inside the LDPC decoder for short codes (`ldpc.c`, `length < 36` branch) | Non-deterministic decode; breaks reproducible test vectors |
| Colour-only finders | No monochrome fallback; our luma-first ordering (§12.1) is deliberate |

Aside: `decodeSlave` computes palette thresholds at `symbol->palette + i*3` (`decoder.c:1420`)
where `decodeMaster` uses `(color_number*3)*i`. That looks like an upstream bug: slave symbols
read thresholds from the wrong palette copy. It doesn't affect us, but it's a reminder that
this codebase is a reference for *ideas*, not for behaviour.

---

## 8. Suggested order

Ordered by expected gain against measured failures, per unit of effort and budget:

1. **§3.4 glare-robust chroma and clipping erasures.** Smallest change, aimed directly at the
   5% face-on bin.
2. **§4.1 identity-keyed accumulation and tracking.** A correctness issue for file transfer,
   not just a robustness one.
3. **§3.1 + §3.3 escalation ladder with decision-directed refit.** Biggest expected gain for
   `C3` and partial glare.
4. **§3.5 corner recovery and §3.2 ring-based geometry refinement.** Glare-on-corner and
   lens distortion at large versions. Add a distortion impairment to §15 first.
5. **§4.3 fountain coding in CFT.** Transfer time and robustness for whole files.
6. **§6 stricter finder confirmation.** Preview frame rate.
7. Everything in Tier 3, after a sweep says it's needed.

Each Tier 1 item adds decode-path code. With the scanner at 89% of the raw budget, measure
`just wasm-size` after each one, as D12–D20 did.
