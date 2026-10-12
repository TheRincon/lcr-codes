# C4 — a 16-colour profile

> **Status: shelved (2026-09-30).** `C4a` was built, then removed. `C3` is the densest
> profile. Two reasons, the second of which matters for any future attempt:
>
> 1. **Too greedy for the field hardware.** It did not decode on a real phone at any setting.
> 2. **It destabilised `C3`.** With `C4` present, `C3` stopped decoding at ECC 1 on the same
>    phone where it had decoded before. The only decode-path change was that the ring's
>    format reader tried a third palette family and accepted profile code 4. The reader
>    keeps whichever family's BCH decode has the best margin, so a new family is a new wrong
>    answer competing with the right one. On a clean render the right answer wins by a mile,
>    which is why every simulated test passed. On a marginal real capture it need not — and
>    because each family fits illumination to *its own* reference colours, an affine fit can
>    partly absorb the palette difference and make the misread systematic rather than
>    random. That mechanism is plausible, not proven: the failure did not reproduce in
>    simulation.
>
> **Before any future profile touches format detection:** measure the false-positive rate of
> the new family against *real* captures of every existing profile, and gate acceptance on
> something stronger than a margin comparison — for example, the illumination fit's
> residual, which a wrong palette should inflate. Adding a family is not free for the
> profiles that were already working.
>
> The measurements below are kept because they are still correct; §5.5 records what the
> build showed before it came out.

Spec proposal for a fourth colour profile: **8 hues × 2 luma levels = 16 colours**,
4 bits/module, a 33% raw density gain over `C3`.

Everything below the first section is measurement, not estimate. The gamut numbers come
from `cargo run -p xtask -- palette`, which tabulates the sRGB boundary at 0.5 L\* × 0.5°
and is the same search that derived the shipped `C1`/`C3` palettes.

---

## 1. Verdict first

**8 hues is geometrically possible and practically doubtful.** The honest summary:

- The search finds **no solution at all** for k = 8 under §4.2 as written, at any luma gap
  from 40 to 54. The binding constraint is `C* ≥ 35`, not hue separation.
- Relaxing that floor, eight evenly-spaced hues reach a common **C\* = 27.5**, which puts
  the minimum pairwise chroma distance at **21.0** against the shipped `C3`'s **52.0** —
  a loss of **7.85 dB** on the channel that is already the weak one.
- The *entire* chroma ECC ladder, rate 0.65 down to 0.35, buys **2.64 dB**. It cannot
  close a 7.85 dB gap. Not at any level, not with any interleaver change.

And the empirical anchor, which matters more than the arithmetic: **`C3` currently decodes
only at ECC 1 on real hardware** — that is roughly one step of margin at 4 hues. C4 asks
for eight more dB than that setup has.

This document specifies C4 properly anyway, because the numbers are worth having written
down — including §4's measurement of the four-luma alternative, which turned out not to
escape the chroma penalty after all.

---

## 2. What the gamut actually allows

The constraint that governs everything is §4.1's: hue states are **iso-luminant and shared
across both luma levels**. One hue angle serves both levels, so a single common `C*` has to
be reachable at all `2k` corners — and it is limited by whichever corner the sRGB gamut
pinches hardest.

Best common `C*` for **evenly spaced** hues, searched over both `L*` levels and the
starting offset:

| k | colours | bits/module | best C\* | L\* levels | meets §4.2's C\* ≥ 35 |
|---:|---:|---:|---:|:---|:---|
| 2 | 4 (`C1`/`C2`) | 2 | 57.3 | 33.5 / 73.5 | yes |
| 4 | 8 (`C3`) | 3 | 32.8 | 39.0 / 79.0 | **no** |
| 8 | 16 (`C4`) | 4 | 27.5 | 41.5 / 81.5 | **no** |
| 16 | 32 | 5 | 26.4 | 42.0 / 82.0 | **no** |

Two things to read off this.

**The curve flattens, which is the encouraging part.** Going 2 → 4 hues costs 43% of `C*`;
4 → 8 costs only 16%, and 8 → 16 costs 4%. By k = 4 the search is already jammed against
the gamut's tightest directions, so extra hues cost comparatively little *in chroma radius*.

**But `C*` is not the metric that matters.** The demodulator discriminates on the distance
between adjacent hue centroids, `d = 2·C*·sin(Δθ/2)`, and `Δθ` halves every time `k`
doubles. That term dominates:

| configuration | C\* | min separation | **d** | vs shipped `C3` |
|---|---:|---:|---:|---:|
| `C1`/`C2`, k = 2 | 57.3 | 180° | 114.6 | +6.87 dB |
| `C3` even, k = 4 | 32.8 | 90° | 46.4 | −0.99 dB |
| **`C3` shipped, uneven** | 42.7 | 75° | **52.0** | 0.00 dB |
| **`C4`, k = 8 even** | 27.5 | 45° | **21.0** | **−7.85 dB** |
| k = 16 | 26.4 | 22.5° | 10.3 | −14.06 dB |

`C3` ships *uneven* — 71° / 146.5° / 280° / 355.5° — precisely because trading angular
slack for chroma radius won at k = 4: it gave up 90° → 75° separation to gain C\* 32.8 →
42.7, netting +1 dB. **That trade does not repeat at k = 8.** §4.2 leaves only 60° of total
slack (360 − 8 × 37.5), the same slack `C3` had, but spread across twice as many gaps — and
the search reports the derived k = 8 palette *violating* the separation floor rather than
finding a useful placement.

---

## 3. Why ECC cannot pay for it

§8.2 already gives chroma two ECC steps more than luma because it is the noisier channel.
The question is whether a third, fourth or fifth step can absorb 7.85 dB. From
`docs/ldpc.md` (mid layer, FER 5%):

| chroma rate | SNR for 5% FER | cumulative gain |
|---:|---:|---:|
| 0.65 | 3.61 dB | — |
| 0.55 | 2.50 dB | 1.11 dB |
| 0.45 | 1.89 dB | 1.72 dB |
| 0.35 | 0.97 dB | **2.64 dB** |

The whole ladder is worth 2.64 dB. It is a third of what C4 needs.

And spending it is self-defeating, because what the payload actually receives is
`raw chroma bits × rate`:

| profile | raw chroma bits | rate | chroma bits/module delivered |
|---|---:|---:|---:|
| `C3` at chroma ECC 0 | 2 | 0.65 | **1.30** |
| `C4` at chroma ECC 0 | 3 | 0.65 | 1.95 |
| `C4` at chroma ECC 3 | 3 | 0.35 | **1.05** |
| `C3` at chroma ECC 3 | 2 | 0.35 | 0.70 |

So `C4` only beats `C3` while it can hold a rate near 0.65 — the setting with the *least*
margin. Pushed to ECC 3 to survive, it delivers 1.05 bits/module, which is **less than
`C3` delivers at ECC 0**. The profile wins only in the regime where it is least likely to
decode, which is the shape of a bad trade.

---

## 4. The alternative axis — measured, and not the free lunch it looked like

> **Correction.** An earlier draft of this section claimed C4b leaves chroma "exactly as
> `C3` has it — no hue loss at all" and costs "about 6.2 dB on luma". The first claim was
> wrong and the second omitted the other half of the bill. The sweep below replaced both.

The counter-proposal is to spend the sixteenth state on **luma**: 4 luma levels × 4 hues.

**As §4.2 is written this is impossible.** Four levels need three inter-level gaps at ≥ 40
plus four intra-level bands at ≤ 12; even with zero spread that is 120 L\* units inside a
100-unit range. But §4.2's `≥ 40` exists for one stated reason — monochrome fallback — and
that only needs the gap where a greyscale reader binarises. A **hierarchical** luma axis
keeps it there:

```
L0 --g-- L1 ======= MSB gap >= 40 ======= L2 --g-- L3
```

The MSB is as robust as `C3`'s luma bit (a 40-unit gap against `C3`'s 40.77) and survives a
greyscale capture. The LSB rides `g` apart. And the gamut table spans L\* 12–96, so
`2g + 40 ≤ 84` caps **g at 22** before chroma is even considered.

**Chroma is not spared.** Hue states are shared across all four levels, and the outer two
sit toward black and white, where the sRGB gamut has almost no chroma to give. The common
`C*` is therefore set by the extremes, and it falls as `g` grows. Swept against the gamut
(`xtask palette`, "C4b" section):

| LSB g | best C\* | L\* levels | chroma d | new LSB bit vs `C3` | chroma vs `C3` |
|---:|---:|:---|---:|---:|---:|
| 8 | 33.5 | 22/30/70/78 | 40.8 | −14.1 dB | −2.1 dB |
| 10 | 30.9 | 20/30/70/80 | 37.6 | −12.2 dB | −2.8 dB |
| 12 | 27.9 | 18/30/70/82 | 33.9 | −10.6 dB | −3.7 dB |
| 14 | 25.2 | 16/30/70/84 | 30.7 | −9.3 dB | −4.6 dB |
| 16 | 22.1 | 14/30/70/86 | 27.0 | −8.1 dB | −5.7 dB |
| **18** | **18.5** | 12/30/70/88 | 22.5 | **−7.1 dB** | **−7.3 dB** |
| 20 | 13.9 | 12/32/72/92 | 16.9 | −6.2 dB | −9.8 dB |
| 22 | 8.6 | 12/34/74/96 | 10.5 | −5.4 dB | −13.9 dB |

There is no row where both columns are small. Widening the LSB step buys luma and spends
chroma almost one-for-one, and the balance point is around **g ≈ 18: roughly 7 dB lost on
the new bit and 7 dB lost on both existing chroma bits.**

Seen bit by bit, the two routes land in almost the same place:

| | strong bits | weak bits |
|---|---|---|
| `C3` | luma @ 40.8 | 2 chroma @ d 52.0 |
| `C4a` — 8 hues × 2 luma | luma @ 40.8 | 3 chroma @ d 21.0 (−7.85 dB) |
| `C4b` — 4 hues × 4 luma, g 18 | MSB luma @ 40 | LSB @ 18 (−7.1 dB), 2 chroma @ d 22.5 (−7.3 dB) |

Both add one bit per module and pay for it with three bits running about 7–8 dB below
where `C3` runs them. `C4b` is marginally better in raw dB and has one real advantage — the
MSB keeps the monochrome fallback — but it is several times the implementation cost:

| | `C4a` | `C4b` |
|---|---|---|
| cost | chroma −7.85 dB | LSB −7.1 dB **and** chroma −7.3 dB |
| monochrome fallback | intact | MSB only |
| decoder work | none — 3 chroma bits | four-level luma demodulation, §12.3 illumination fit, binarisation |
| §4.2 changes | drop `C* ≥ 35` | replace `≥ 40` with an MSB-gap rule, and drop `C* ≥ 35` |

One caveat keeps this from being a flat tie: the luma and chroma channels do not start from
the same noise floor, and `Diagnostics::estimated_snr_db` reports luma only, so there is no
field measurement yet of how much headroom each channel actually has. If luma turns out to
have far more margin than chroma on real panels, `C4b`'s LSB loss matters less than its
chroma loss suggests. That is a measurement, not something the gamut can answer.

---

## 5. Specification, if C4a proceeds

### 5.1 Palette

Two luma levels × eight hue states, iso-luminant and shared across levels, at a single
common `C*`. The search's best evenly-spaced configuration:

```
L* 41.5 / 81.5      C* 27.5      hues 12° + n·45°,  n ∈ 0..8
index(l, h) = l·8 + h
```

**[TBD]** These are gamut-derived, not panel-measured. §4.4 already marks `C3`'s
eight-colour palette TBD for the same reason, and C4 should not be pinned ahead of it —
building a 16-colour palette on an unmeasured 8-colour one compounds the uncertainty.

### 5.2 Structure

| field | value |
|---|---|
| colours | 16 |
| luma levels | 2 |
| hue states | 8 |
| chroma bits | 3 |
| bits/module | 4 |
| chroma cell | 1×1 (as `C1`/`C3`) |

### 5.3 Signalling — C4 is ring-only

This falls out of the existing format words and is not a choice:

- **Classic §7 word**: profile is **2 bits** (`format.rs:43`, read back at `(w >> 8) & 3`).
  All four values are taken by M / C1 / C2 / C3. **There is no encoding for C4.**
- **D21 ring word**: profile is **4 bits** (`(w >> 11) & 0xF`), of which 12 values are
  spare.

So C4 is available only to finder-less symbols — the same restriction that applies to
versions above 32, and for the same structural reason. A finder-bearing C4 symbol would
need the §7 word widened, which is a wire-format break.

### 5.4 ECC pairing

§8.2's `CHROMA_RATE_INDEX = [2,3,4,5]` gives chroma two steps more than luma. C4 would
need more, and there is nowhere to go: `RATES` has six entries and index 5 (rate 0.35) is
already the chroma floor. Extending `RATES` below 0.35 is possible but the returns are
visibly flattening — 0.45 → 0.35 buys 0.92 dB.

---

## 5.5 Built — what the experiment shows

`C4a` is implemented as `Profile::C4`, experimental and ring-only. Palette `C16_COLORS`:
L\* 40 / 82 (gap 42), hues every 45° from 12°, common C\* 26 — one unit under the 27 the
gamut reaches at all sixteen corners.

**Capacity** — against `C3` at the same version, ECC 1/1:

| version | `C3` | `C4` | gain |
|---:|---:|---:|---:|
| v5 | 325 B | 426 B | +31% |
| v10 | 765 B | 996 B | +30% |
| v20 | 2,196 B | 2,851 B | +30% |
| v32 | 4,868 B | 6,321 B | +30% |
| v48 | 10,065 B | 13,061 B | +30% |

+30%, not the raw +33%, because the extra bit is a chroma bit and chroma runs at a lower
code rate than luma.

The practical consequence is bigger than the percentage: **the same file needs a smaller
symbol**. An 8,000-byte slice of a real PDF needs v42 (185×185) at `C3` but v36 (161×161)
at `C4`, both ECC 1. A smaller symbol is easier to show at §13.1's integer device-pixel
density — the constraint that stopped large symbols decoding in the field.

**Robustness** — the §15 impairments, v10, same payload length for both profiles, 10 seeds:

| impairment | `C3` / `C4`, ECC 1 | `C3` / `C4`, ECC 3 |
|---|---|---|
| clean; noise σ ≤ 0.06; warm casts; 4:2:0 subsampling | 10 / 10 | 10 / 10 |
| sensor noise σ = 0.08 | 10 / **2** | 10 / **6** |
| defocus r = 3 | 8 / **0** | 10 / 10 |

This is **better than §2's 7.85 dB suggested**, and worth being precise about why. The
simulated channel has margin in the middle of the envelope, so a 7.85 dB loss there costs
nothing visible; it shows up only at the edge, where `C4` falls over first. Colour casts
cost neither profile anything, because §12.3's affine illumination fit removes them before
hues are classified.

What the simulation cannot answer is where a real phone sits on that curve. The field report
— `C3` decodes only at ECC 1 — says real captures are nearer the edge than σ = 0.06, which is
exactly where `C4` gives way. So the phone test is the result that counts, and the table says
what to try first: **`C4` at ECC 3**, with `C3` at ECC 3 as the control.

Reproduce with:

```
cargo test -p lcr-decode --release --test c4_profile -- --ignored --nocapture
```

---

## 6. What I would do instead

Three options, in the order I would take them:

1. **Measure `C3`'s palette first.** §4.4 is `[TBD]` — the shipped 8-colour palette has
   never been verified on real panels, and the field report is that it decodes only at
   ECC 1. That is the measurement that tells you whether the chroma channel has *any*
   headroom, and C4a is unarguable without it. If C3 turns out to have been leaving
   margin on the table, C4a's −7.85 dB looks different.

2. **Use the capacity that already exists.** The version ceiling now reaches v64 and a
   single symbol carries 16,383 bytes — a 2.9× gain, already verified end to end. C4a
   offers +33% on an unproven channel; bigger symbols offer more than that on a proven
   one. If the goal is "a file in one symbol", widening §9.1's 14-bit length field is the
   cheaper lever (see the analysis in the previous session — it is one file plus a spec
   table).

3. **If a fourth profile is still wanted, prototype `C4a` first.** §4 shows the two
   routes cost about the same in dB; `C4a` is a palette plus a third chroma bit, `C4b` is
   a rework of every luma path in the decoder. Build the cheap one, put it in front of a
   camera, and only reach for `C4b` if the field shows chroma is the binding channel and
   luma has headroom to spare.

---

## 7. Reproducing everything here

```
cargo run -p xtask -- palette
```

The k = 8 row and the evenly-spaced-hue ceiling table were added to that report for this
proposal; the `C1`/`C3` rows are the same search that derived the shipped palettes.
