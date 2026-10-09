# Pinned decisions and deviations from spec v0.4

A reference implementation cannot leave the wire format open. This is the register of
every `[TBD]` the spec leaves and every place the implementation departs from what v0.4
says — with the reason, and where the test that locks it in lives.

Anything marked **Deviation** changes the wire format or contradicts the draft, and needs
sign-off before v1.0 freeze. Anything marked **Pinned** just fills a hole the draft left
open.

**D1, D2 and D3 are provisional.** They are carried forward so the decoder can be built
against something concrete, not because they are settled. See
[Unwinding the provisional decisions](#unwinding-the-provisional-decisions) for what it
costs to revert each one.

---

## Deviations — these contradict v0.4 and need a decision

### D1. The §4.3 palette does not satisfy §4.2

**Status:** replaced. `crates/lc-core/src/palette.rs`

Measured against §4.2's own bounds, the four colours printed in §4.3 fail three of four:

| Bound | Requirement | §4.3 palette |
|---|---|---|
| Inter-level luma gap | ≥ 40 | 42.1 ✅ |
| Intra-level luma spread | ≤ 12 | **13.6** ❌ |
| Hue separation (k=2) | ≥ 150° | **94.9° / 122.5°** ❌ |
| Chroma `C*` | ≥ 35 | 36.5 ✅ (marginal) |

There is also a structural problem the scalar bounds do not catch: **the hue states are
not the same hue at the two luma levels.** H0 is 301.7° at L0 but 214.4° at L1; H1 is
36.5° at L0 but 92.0° at L1. §12.4 demodulates chroma against a hue centroid shared by
both levels, so a palette whose hue states move 80–90° between levels forces the decoder
to know the luma bit before it can read the hue bit — exactly the layer coupling §4.1
exists to prevent.

Both colour palettes are now *derived* from the §4.2 constraints rather than transcribed:
iso-luminant hue states shared across levels, a single common `C*`, and `C*` set to a
value the sRGB gamut reaches at every corner.

| | L0 | L1 | Hues | `C*` | Gap | Spread | Min hue sep |
|---|---|---|---|---|---|---|---|
| C1 / C2 | 32 | 78 | 136°, 326° | 55 | 45.8 | 0.3 | 169.8° |
| C3 | 30 | 74 | 71°, 146.5°, 280°, 355.5° | 40 | 43.8 | 0.2 | 75.2° |

The four-colour result is **green against magenta**, which has a property worth keeping
through any recalibration: the discriminating signal is G against R+B, the axis on which a
Bayer sensor has *twice* the photosite density. §1.4 argues chroma is the low-bandwidth
channel; this spends what bandwidth there is where the sensor actually has it.

`SPEC_4_3_COLORS` is retained, unused, with a test asserting it still fails — so the
deviation cannot be quietly reverted by someone transcribing the spec.

**Corrected 2026-09-02 — the first pinning did not satisfy its own third constraint.**
`C*` was set to 55 for C4 and 40 for C8 without anything checking those were reachable,
and neither was: at the original 316° magenta the gamut stops at 47.9 for `L* = 78`, and
at the original 149° it stops at 39.4 for `L* = 30`. Both palettes therefore shipped with
one corner silently gamut-clipped — C4 measured 55.1 / 55.0 / 55.3 / **48.1**.

That is the specific failure the "single common `C*`" rule exists to prevent. §12.4
demodulates against one hue centroid per state shared across both luma levels; a clipped
corner pulls its state's centroid toward the neutral axis, so the two states stop being
equidistant from it and the per-bit chroma LLRs acquire a bias the ECC cannot see.

The fix was smaller than the diagnosis. `C* = 55` is not too ambitious — **316° was the
wrong angle.** Moving the magenta state to 326° clears 55 at both levels with `L*`, `C*`
and the green state untouched, so the inter-level gap stays at 45.8 and all four colours
land at `C* ≈ 55`. C8's hues moved for the same reason, to 71°/146.5°/280°/355.5°.

Two guards, because one was not enough. The reachability check inside `derive` was already
there as a `debug_assert!` and had been failing since the palettes were pinned — invisible
because `just test` passes `--release`, which strips it. It is now also
`no_shipped_colour_is_gamut_clipped`, a release-mode test, and `just test-debug` runs the
unit tests unoptimised so the remaining `debug_assert!`s are exercised at all.

**Needs your call:** these are derived, not measured. §4.3 and §4.4 both want panel
measurement. The structure is right; the four numbers per palette are placeholders for
calibration data. `just palette` is the search that produces them — it reports the best
configuration per profile, the rate at which luma gap trades against chroma (about 0.85
of `C*` per unit of gap at k=2), and what each hue orientation is worth, so recalibration
against measured panels is a matter of re-running it rather than hand-fitting.

### D2. §4.4's "four hue states ~90° apart" is not achievable in sRGB

**Status:** hue spacing made uneven. `crates/lc-core/src/palette.rs`

Four iso-luminant hues at exactly 90° spacing, with a 40-unit inter-level gap, cap out at
`C* = 32.4` — below §4.2's `C* ≥ 35` floor. §4.2 only requires `≥ 300°/k = 75°`, so
spending the 60° of slack on the directions where sRGB is roomiest reaches `C* = 40`. The
pinned C3 hues are consequently uneven: 71°/146.5°/280°/355.5°. (These were
72°/149°/280°/356° until the gamut-clipping correction recorded in D1.)

### D3. §12.3's residual surface must be multiplicative, not additive

**Status:** model restructured. `crates/lc-decode/src/illum.rs`

§12.3 step 2 says to fit the correction surface "over the residuals", which reads as
additive. **An additive surface cannot cancel a shading gradient**, and shading is the
dominant spatial impairment the step exists for. A gradient scales every channel by
`s(x, y)`, so the residual it leaves is `(1 − s) · nominal` — proportional to the colour
of the module, and therefore different for every palette entry at the same position. One
additive surface fitted against a mixture of palette colours produces their average and
cancels none of them.

Measured on a 50% linear falloff: additive-only leaves 0.23 of linear-light residual,
larger than the gap between palette entries.

Two further arrangement details turned out to matter as much as the model:

- **Fit forward, then invert.** A linear attenuation *is* a bilinear surface, so a forward
  fit recovers it exactly. Its inverse is rational and a bilinear surface can only
  approximate it — 0.05 residual on the same falloff.
- **Spatial before global.** The affine mixes channels; the per-channel surface cannot.
  Fit the affine first and it absorbs the average shading, leaving the surface a target
  that depends on all three nominal channels — still 0.05 residual.

Final model, which reaches < 0.02 on shading, on flare, and on both together:

```text
fit:      observed_c  ≈  gain_c(x, y) · nominal_c  +  flare_c(x, y)
correct:  nominal     =  affine · [ (observed_c − flare_c(x, y)) / gain_c(x, y) ]
```

### D4. §5's bits-per-module figure is not reachable for `C2`

**Status:** measured values pinned. `crates/lc-core/src/layout.rs`

| Profile | §5 | Measured at v32 |
|---|---|---|
| M | 1.00 | 1.000 |
| C1 | 2.00 | 2.000 |
| C2 | 1.25 | **1.227** |
| C3 | 1.50 | **3.000** (see D11 — the chroma cell changed) |

`C2`'s shortfall is the partial-chroma-cell rule (see P6) and decomposes cleanly: ~6.3%
from scattered references disqualifying their 2×2 cell, ~2.7% because `side` is always odd
so a 2×2 grid cannot tile it, and the rest from cells straddling fixed patterns. §5's
figure is the geometric ideal for an infinite field with no references.

Only profiles with a 2×2 cell can suffer this. A 1×1 cell cannot straddle anything, which
is why `C1` — and now `C3` — land exactly on their ideal.

### D5. §6.1's capacity table was optimistic

**Status:** recomputed. `cargo run -p xtask -- capacity`

| Version | §6.1 estimate (C1, ECC 1) | Computed |
|---|---|---|
| 1 | ~18 B | **20 B** |
| 2 | ~42 B | 42 B |
| 3 | ~76 B | 77 B |
| 5 | ~165 B | 161 B |
| 8 | ~330 B | 318 B |
| 12 | ~640 B | 607 B |
| 20 | ~1.6 KB | 1377 B |
| 32 | ~3.9 KB | 3143 B |

Close at the small end, 15–20% low at the large end. The estimates did not account for
LDPC shortening (a layer length is set by geometry and never lands on `Z·n_b`, so the
effective rate sits below the table value), the partial-chroma-cell rule, or the fixed
overheads at low versions.

Density against the monochrome fallback is **1.73×**, inside §1.1 goal 2's 1.5–2× target.

### D6. §13.1's ≥ 8 device-pixels-per-module floor did not detect

**Status:** fixed, no spec change needed. `crates/lc-decode/src/image.rs`, guarded by
`decodes_at_the_normative_minimum_module_pitch`.

§13.1 point 3 makes "≥ 8 device pixels per module" normative for a conforming display
application. At exactly 8, `detect::find_finders` returned **zero** candidates on a clean,
undegraded render, at every version. At 9 it detected normally. Nothing caught it because
every round-trip and degradation test renders at 10 or 12 — the §13.1 *preferred* range —
so the floor the spec actually mandates was never exercised.

**Cause.** Not the detector. `BLOCK` is also 8, so at that pitch the binarisation grid
coincided with the module grid: every block landed wholly inside one module and not one
block in the image saw an edge. Each then took its threshold from an already-computed
neighbour — `avg = min(neighbour, own_min)` — which is a recurrence, pass one both reading
and writing `block_avg`, and a monotone one. A single dark block dragged everything
below-right of it downward with no way back up. A v32 symbol that is 47.7% dark binarised
to 3.2% dark:

| Version | Genuinely dark | Binarised dark, before | after |
|---|---|---|---|
| 1 | 35.2% | 10.6% | 35.2% |
| 5 | 41.3% | 9.2% | 41.3% |
| 14 | 46.6% | 5.8% | 46.6% |
| 32 | 47.7% | 3.2% | 47.7% |

**Fix.** Two changes, both in `binarize`.

1. The recurrence is gone. Blocks are classified into two levels of evidence — *saw an
   edge itself*, and *within one block of one* — and the threshold is computed from
   whichever level is available, nearest first. No block inherits a value from another
   block's result, so nothing can ratchet.
2. The threshold statistic is one step of Ridler–Calvard: split the block means in the
   window at their own mean, average each side, take the midpoint. A plain mean cannot be
   used at the second evidence level, because those blocks each sit wholly inside one
   module and their means are *level* values, so averaging them tracks the local dark/light
   balance instead of the black point. Splitting first is indifferent to the mix. Block
   means rather than raw pixel extremes, because a raw midpoint chases outliers and on a
   glossy screen the outlier is the impairment (§3).

**Cost to the operating envelope: none measurable.** Over the 8 tilts × 5 rotations of the
perspective grid, 28/40 cells decode against 27/40 before. Individual marginal cells flip
in both directions — `(0.5, 45°)` was lost, `(0.5, 0°)`, `(0.6, 0°)` and `(0.6, 15°)`
gained. `survives_perspective` was asserting named cells in that marginal band, which
measures luck; it now asserts the structural cases individually and holds the rest as an
aggregate count, per §15's "a pass/fail corpus will not tell you whether a change helped".

Binarisation also got *more* accurate everywhere, not just at pitch 8 — the dark fraction
now matches the true fraction to within 0.1% at every pitch, where before it drifted by up
to 0.5%. No other test needed adjusting; in particular `multi_frame_beats_single_frame`,
which asserts a single frame *fails* at σ = 0.1 so that multi-frame combining has
something to beat, still holds unchanged.

### D7. `no_std` does not build, and will not be made to

**Status:** accepted deviation, decided. `crates/lc-wasm/Cargo.toml`

§14.1 describes `lc-core`, `lc-decode` and `lc-verify` as "`no_std` + alloc". **They do not
build that way.** `cargo build -p lc-core --no-default-features` fails with 31 errors, all
of them float maths: §4's colour conversions use `sqrt`, `powf`, `atan2`, `cbrt`, `sin`,
`cos` and `to_radians`, which are `std` methods and have no `core` equivalent.

This was never noticed because the default features include `std`, so no build ever
exercised the `no_std` configuration. The crates are *written* `no_std`-style — `#![no_std]`
with `alloc` — but the configuration has never compiled.

Making it compile means a `libm` dependency and mechanical replacement at ~84 call sites
(`powi` 30, `sqrt` 23, `to_radians` 7, `cos` 6, `round` 5, `powf` 4, `sin` 3, `atan2` 2,
one each of `log10`/`ln`/`exp`/`cbrt`).

**Declined, deliberately.** The only deployments `no_std` unblocks are bare-metal embedded
targets with no OS, which are not a target group for this project. Everything that is —
browsers, phones, servers, Node, WASI runtimes — provides `std`. Taking a dependency and
touching 84 numerical call sites to serve a hypothetical embedder is not a trade worth
making now.

**What this costs, stated plainly:** `std` is a hard requirement of every LC Code crate,
and §14.1's description is aspirational rather than accurate. §14.2's actual constraints
are unaffected — it bans **C dependencies** and **`getrandom`**, not the standard library,
and the shipped wasm bundle has neither.

Measured cost to the thing §14.2 does care about: the decode-only component is **192,046
bytes, 93% of the 200 KB budget** (open #16), with `std` on. That is tighter than it looks
elsewhere in this document because D8 gates it raw rather than compressed — but the
pressure it creates is an argument for trimming the decode path, not for `no_std`, which
would change the float maths and not the code size.

Revisit if an embedded target becomes real. Until then, either §14.1 should be reworded or
this entry stands in its place.

### D8. The wasm target is a Component Model component, not a browser bundle

**Status:** decided. `crates/lc-wasm/wit/lc.wit`, built by `just component`.

§14.2 specifies a browser artifact throughout: "Accept `Uint8ClampedArray` from canvas
`getImageData` directly", `wasm-bindgen`, a bundle measured **gzipped** because it is
served over a network. §17.3's adoption argument is likewise about "a web app or POS
vendor". **The deployment target is not a browser** — it is a host application embedding a
wasm runtime, with hosts in Rust and Go/C/C++ — so the wasm crate targets the Component
Model and exports a WIT interface instead.

A `wasm-bindgen` build was written first and then replaced. It was the wrong artifact for
a non-JS host in three concrete ways, all measured on the built module:

- It **imported three functions from a JS module**, `./lc_wasm_bg.js`. A non-JS host has
  to hand-stub `__wbindgen_throw`, `Error` and `__wbindgen_init_externref_table` before
  the module will instantiate.
- It **required the reference-types proposal** — `wasm-tools validate
  --features=-reference-types` failed at offset 0x88, because `wasm-bindgen` puts an
  `externref` table in the module. wasmtime and wasmer are fine with that; WAMR and
  several embedded runtimes are not, by default.
- Its exports were **bindgen-mangled** (`lcdecoder_new`, `__wbg_lcdecoder_free`) with an
  ABI assuming JS glue does the marshalling. A host calling those directly is reverse
  engineering a private calling convention.

**Build route.** `wasm32-wasip2` produces a component directly, but links the whole WASI
CLI world — stdin/stdout/stderr, environment, exit, terminal handles, a monotonic clock —
none of which decoding needs. Building for `wasm32-unknown-unknown` and componentising
with `wasm-tools component new` avoids all of it:

| Route | Raw | zstd -19 | Host imports |
|---|---|---|---|
| `wasm32-wasip2` | 334,386 | 131,041 | 15 WASI interfaces |
| `wasm32-unknown-unknown` + `component new` | 282,508 | **103,617** | **none** |

The shipped components import **nothing** a host must provide. The single
`import lc:code/types@0.4.0` is a types-only interface containing zero functions.

Two worlds, per §14.1: `lc-code` exports `decoder` and `encoder`; `scanner` exports only
`decoder`, so a scanner build cannot acquire the encoder by accident.

**The budget is now gated in raw bytes, and that changes the answer.** §14.2 says "200 KB
gzipped" because it assumed a browser bundle fetched over a network. A component is loaded
from disk by its host, so the uncompressed size is the one that has to fit. Measured that
way the scanner is **192,046 bytes, 93% of budget** — against 33% if compressed. The
compressed figure reports headroom that does not exist, so `just wasm-size` gates on raw
and reports zstd alongside for the case where the artifact genuinely is transferred.

Open decision #16 is therefore answered *conditionally*: the scanner fits, but with 12 KB
of margin rather than the comfortable two-thirds the spec's unit implies. Anything that
grows the decode path — the §15 impairments still to be added, a larger LDPC table — will
need re-measuring against this.

**What this costs.** §14.2's browser-specific requirements are now inapplicable rather
than met: there is no `Uint8ClampedArray` path, no JS API, no npm package. If a browser
deployment is ever wanted, `jco` can transpile a component to JS, so this is not a
one-way door — but it would want its own measurement, since the transpiled output is
larger than a purpose-built `wasm-bindgen` bundle.

§17.3's argument survives the move intact and is arguably better served: a typed,
versioned WIT interface generates the host's bindings, which is a lower adoption cost than
a hand-written ABI in any language.


### D9. No Go runtime can host a component today

**Status:** blocked upstream, not on us. Verified 2026-09-01; recheck before planning Go work.

§17.3's adoption argument names non-browser hosts, and Go is an obvious one. **It does not
work yet**, and the reason is worth recording so nobody re-derives it:

| Runtime | Component Model | Detail |
|---|---|---|
| `wasmtime-go` v34.0.0 | ❌ in the Go API | The module **ships a component-capable C library** — `build/include/wasmtime/component/{component,func,instance,linker,val}.h` plus `libwasmtime.a` for five platforms — but no Go source file binds any of it. `grep -ic component *.go` finds two incidental hits, both unrelated. |
| `wazero` v1.12.0 | ❌ | Pure Go, no cgo, implements the Core Specification 1.0/2.0 only. |

So a Go host today means hand-writing cgo bindings against `component.h` — which is
exactly the cost §17.3 claims a WIT interface removes. Doing it would demonstrate the
opposite of the thing we set out to show.

**What was done instead.** A **C host** trial, `examples/c-host/main.c`, run by
`just c-host`. The wasmtime C API *does* expose the Component Model, so a C host works
today with no hand-rolled ABI. It encodes JSON and decodes the result back inside one
process, asserting the packet arrives as `unsigned` carrying CBOR, and — most usefully —
instantiates with a linker containing nothing at all. That is the same claim
`instantiates_with_an_empty_linker` makes on the Rust side, now confirmed from a host
that shares none of the guest's idioms.

Two languages, no Rust-specific assumptions, and the C API is also the path a future
`wasmtime-go` component binding will take, so this de-risks Go rather than deferring it.

**When to revisit:** when `wasmtime-go` binds the component API, or if a Go deployment
becomes concrete. In the latter case the options are cgo against the C API, or finishing
`lc-ffi` and having Go call a native library — which works but abandons the
build-once-run-anywhere property, since you are back to shipping per-platform binaries.

---

### D10. §8.2's rate table is right; its stated reason is not

**Status:** table unchanged, rationale corrected. `crates/lc-decode/src/demod.rs`

§8.2 gives each ECC level a chroma rate two steps stronger than its luma rate and calls
the inequality load-bearing, because "chroma bits are measured over a noisier,
lower-bandwidth channel". Both halves of the table were guesses. Measuring them needed a
per-layer SNR, so `ChannelStats::chroma_snr_db` was added alongside the existing
`snr_db` — both are `20·log10(amplitude / sigma)` against half the distance between the
nearest decision points, which is the same axis `docs/ldpc.md` sweeps, so a layer's
measured SNR can be read straight against the rate thresholds.

**Chroma is not the noisier channel.** On a clean capture it measures **37.3 dB against
luma's 29.7** — 7.6 dB *better*. That is not a surprise once measured: it is almost
exactly the palette's amplitude ratio, `20·log10(55 / 22.9) = 7.6 dB`, because D1 gives
the hue axis 110 units of `a*b*` separation while the luma axis has a 45.8 `L*` gap.
Under noise or glare the two converge to parity (within 0.4 dB) but never cross.

**The extra protection is still doing work.** Narrowing the offset from two steps to one
buys 8.6% capacity on `C1` (v4: 116 → 126 B) and costs the blur envelope almost
everything — success at blur radius 4 falls from **81% to 3%**, far outside the ±9-point
error bar, with every other impairment unchanged. Restoring the offset restores the 81%.

The two findings resolve against each other in §1.4 rather than §8.2. Chroma's handicap is
**spatial bandwidth, not noise** — it is low-passed by the capture pipeline and sampled
over a wider kernel (§12.6), so defocus destroys it first. A per-module noise sigma cannot
see that, which is why the SNR measurement and the blur sweep disagree: the SNR is blind to
the impairment that actually binds chroma.

Two steps is also the most the family allows — level 3's chroma already sits on `RATES[5]`,
the last entry, so the offset cannot be widened without extending the rate list.

**Re-validated across profiles after D11.** The above was measured on `C1`, where chroma
carries 42% of the payload. D11 then made chroma the *majority* of `C3` — 60% — so the
table's central claim had only ever been tested where chroma was the minority layer. Two
things came out of re-running it:

**Whether chroma is "the noisier channel" is a property of the palette, not of chroma.**
Measured luma-minus-chroma SNR, same scale both profiles:

| | hue axis | clean | under stress | §8.2's premise |
|---|---|---:|---:|---|
| `C1` | 2 states, 170° apart, `C* 55` | −7.6 dB | ~0 | **false** |
| `C3` | 4 states, 75° apart, `C* 40` | −0.9 dB | **+5.5 … +8.3 dB** | **true** |

Negative means chroma is the *better* channel. `C1`'s palette hands chroma more than twice
the separation luma gets, so chroma wins; `C3` crowds four hues into the same circle at
lower chroma and loses that advantage entirely, degrading faster than luma under load.
§8.2's sentence is not wrong in general — it is wrong about `C1` specifically, and right
about `C3`. It should say so rather than generalising.

**The offset survives on both.** Narrowing it to one step, measured directly:

| | capacity | blur m50 before | after |
|---|---|---:|---:|
| `C1` | 116 → 126 B (+8.6%) | 4.71 | 3.79 (−20%) |
| `C2` | 75 → 77 B (+2.7%) | 5.75 | 5.75 (no change) |
| `C3` | 165 → 184 B (+11.5%) | 4.94 | 3.94 (−20%) |

Same trade on `C1` and `C3`: roughly a fifth of the blur envelope for a tenth of the
capacity. `C2` is the outlier and the reason is arithmetic — chroma is 11% of its payload,
so the chroma rate barely moves it. `C2`'s robustness is essentially luma-determined, and
its chroma rate could be chosen on capacity grounds alone.

**So the table ships as §8.2 printed it**, now measured rather than guessed. The wording
that should change is §8.2's "noisier": it is the reason given for a decision that is
correct for a different reason, and anyone recalibrating the palette from that sentence
would tune the wrong quantity.

### D11. `C3` becomes 8 colours at a 1×1 chroma cell

**Status:** wire format changed. `crates/lc-core/src/version.rs`

§5 specifies `C3` as eight colours at a **2×2** chroma cell, and calls it "high-quality
screen capture". Those two things contradict each other, and measuring the profiles head
to head is what made it obvious.

**As specified, `C3` was nearly pointless.** Eight colours sounds denser than four but is
not, because the palette is not what sets density — the chroma cell is. Every module
carries its own luma bit either way; the only question is how many modules share a hue
symbol. Doubling the palette adds one bit per cell, while going 1×1 → 2×2 divides the
number of cells by four. So `C3` measured **1.455** bits/module against `C1`'s 2.000.

Measured at v4, each profile loaded to its own capacity (`--example profiles`):

| | capacity | noise m50 | blur m50 | glare m50 |
|---|---:|---:|---:|---:|
| `C1` | 116 B | 0.128 | 4.71 | 0.600 |
| `C2` | 75 B | 0.128 | **5.75** | 0.600 |
| `C3` **old**, 2×2 | 82 B | 0.096 | **> 6.0** | 0.480 |
| `C3` **new**, 1×1 | **165 B** | 0.096 | 4.94 | 0.510 |

The old `C3` was not strictly dominated — the 2×2 cell averages four chroma samples and
made it the most blur-tolerant profile in the set. But that is the one thing it won, `C2`
covers the same ground at 5.75, and it paid for the win with the worst noise and glare
figures and a capacity barely above `C2`'s.

**Moving to a 1×1 cell doubles its capacity for almost nothing.** Noise tolerance is
*identical* (0.096 either way), glare is slightly *better* (0.510 against 0.480), and only
blur regresses — to 4.94, which is still marginally ahead of `C1`. 82 B → 165 B for a
change that is, on three of four impairments, free.

That makes the profile lineup genuinely non-overlapping for the first time: `C2` trades
density for blur tolerance, `C1` balances, `C3` trades margin for density. Against a QR
version 4 in the same 33×33 square, `C3` now carries **2.66×** what QR-M does.

**Why this is legitimate to change.** §5 marks `C3` optional and requires only `M`, `C1`
and `C2`; §4.4's eight-colour palette is still `[TBD]` in the draft; and no wire format has
been frozen. The profile field is two bits with all four values already allocated, so
adding a *fifth* profile would mean restructuring the format region — redefining the one
optional profile costs nothing by comparison.

**The cost, stated plainly.** `C3` is now the least robust profile by design: four hue
states 75° apart at `C* ≈ 40`, against two states 170° apart at `C* ≈ 55`. That is ~25%
less noise tolerance and ~15% less glare tolerance than `C1`. It is documented on the enum
variant itself so the trade is visible at the point of choice, and nothing selects it
automatically — a caller has to ask for it by name.

### D12. `decoder.probe` — geometry without a payload

**Status:** addition beyond v0.4. `crates/lc-decode/src/lib.rs`, `wit/lc.wit`

§14.4's API sketch has one entry point, `decode_rgb`, returning either a `DecodeOutput` or
an error. §12.5 and §14.3 then ask a decoder to drive "user guidance ('move closer',
'reduce glare')" from its diagnostics — but on the path where guidance is *needed*, the
decode failed and there are no diagnostics. A host gets a string. It cannot distinguish

- no symbol in the frame — the user is out of range, tell them to move closer, from
- a symbol found, format read, payload unrecoverable — the user is in range and out of
  margin, tell them to steady the phone or kill the glare,

which are opposite instructions. `probe` runs §12.1–§12.3 and stops, so those two cases
separate. It reports version, profile, format margin, the illumination residual, and the
captured module pitch at both extremes.

**It also makes a capture sweep analysable**, which is what prompted it: a measurement
needs to bin its *failures* by the conditions they failed under, and a failure carrying no
geometry cannot be binned at all. See [`capture-sweep.md`](capture-sweep.md).

**Two new `Diagnostics` fields come with it**, `capture_pitch_min` / `capture_pitch_max`,
both derived from the fitted homography by `Transform::pitch_range`. Their ratio measures
viewing obliquity without needing camera intrinsics, which a browser does not expose.
`probe_measures_the_pitch_the_renderer_drew_at` pins them against a known render, because
every working-range figure measured through the rig scales with them.

**The cost, stated plainly: the scanner went 192,876 → 196,104 bytes, 93% → 95% of
§14.2's budget.** That is 3.2 KB for a function a minimal scanner could arguably live
without, and it comes out of headroom that was already thin — the same headroom the brotli
discussion above is competing for. It is kept in the `decoder` interface rather than
banished to a full-build-only one because §12.5's user guidance is a scanner requirement,
not a development convenience: a scanner that cannot tell "not found" from "found and
marginal" cannot do what §12.5 asks of it. If the budget later binds, this is a candidate
to move — the measurement rig uses the full component and would not notice.

### D13. QR lives in `lc-render` behind `qr`, and detection had to learn to try again

**Status:** addition beyond v0.4. `crates/lc-render/src/qr.rs`, `crates/lc-decode/src/detect.rs`

§17.2 names dual-render — a standard QR alongside carrying a fallback URL — as one of three
answers to the adoption problem, so it is a spec-sanctioned feature rather than a demo.
`lc-render::qr` implements it: `render_dual_rgb` / `render_dual_png` compose an LC symbol
and a QR at **one shared module pitch**, which is the property that makes a fallback a
fallback. Rendering the QR smaller because it has fewer modules would produce a backup that
stops working before the thing it backs up.

**Off by default.** `lc-render` is in the wasm full build, where QR costs ~23 KB, and §17.2
itself calls dual-render "a bridge, not an answer" — it trades the offline property for
reach. A deployment should opt into that rather than acquire it by linking the renderer.
`lc-wasm/compare` is the same code behind a second feature flag, so the density comparison
in `display.html` and a rendered dual share one encoder and one set of round-trip tests.

**Generation is `qrcodegen`; verification is `rqrr`.** Both pure Rust; `rqrr` is
`default-features = false` so it drops `image`, and it is a dev-dependency that reaches no
artifact. A hand-rolled QR encoder was started and abandoned: its masking, interleaving and
format info are unprovable without a decoder, which would have left the density claim
resting on the encoder being right.

**What this exposed, and it is the important part.** §6.2 adopts QR's 1:1:3:1:1 finder
deliberately (Appendix B), and **a finder carries no identity**. So a frame holding two
codes presents six finder-like patterns, and when both sit at one module pitch the
module-spread filter cannot separate them: a triple straddling the two symbols can score
*better* on right-angleness than either real triple. Detection committed to the single
best-scoring triple with no way back, so a dual-render — and equally two LC symbols on one
poster, an ordinary thing to point a camera at — returned `FormatUndecodable` on a frame
containing two perfectly good symbols.

`detect_candidates` now returns the best triples in score order and `Decoder::decode` walks
them. **The acceptance test is the CRC**, per §9.1's "the CRC is the authoritative
accept/reject for decoding", and the first attempt at this got it wrong in an instructive
way: accepting on the *format region* instead looked reasonable and made the noise-rejection
test fail immediately. BCH(30,15) corrects three errors, so a random 30-bit word lands
within distance 3 of a codeword about 14% of the time — trying six candidates against that
filter turns "found nothing" into "found something" on frames containing no symbol at all.
Only the CRC is strong enough to justify a retry.

`Decoder::probe` and `FrameAccumulator` therefore still use the best candidate **only**:
neither has a CRC to check a guess against, so for them retrying is strictly worse.
`prepare`'s doc comment says so, and `probe_finds_nothing_in_a_frame_with_no_symbol` fails
the moment that loop is reintroduced.

**Cost, measured, twice.** Six candidates doubled the §15 degradation suite, 33.5 s →
67.2 s, entirely on frames that fail — which in a live preview is every frame the user is
still aiming. Three candidates recover all of it (33.5 s) and still fix the case, because a
dual-render's two real triples both score well. `MAX_CANDIDATES = 3` is that measurement.

The second cost was size, and `just wasm-size` failed on it. Collecting every scored triple
into a `Vec`, sorting with `sort_by`, and taking three through a `filter_map`/`take`/
`collect` chain cost **10.4 KB** in the scanner — 196,104 → 206,469 bytes, over §14.2's
204,800 budget. Selecting the best three by insertion into a fixed three-entry array needs
no sort, no growth and no iterator machinery, and costs **1.5 KB** instead (197,663, 96%).
Worth recording because the naive version is what anyone would write first, and nothing
about it looks expensive.

**Known limitation.** `FrameAccumulator` can still lock onto a straddling triple in a
two-code frame and keep it until reset. Single-frame `decode` handles that case; multi-frame
accumulation does not yet.

### D14. The §6.4 border is mandatory on screen, and carries a position sequence

**Status:** deviates from v0.4. `crates/lc-core/src/border.rs`, `border_seq.rs`

Two changes, the second the reason for the first.

**The border now carries a de Bruijn-style position sequence.** §6.4's ring cycled the
palette in index order, which made it good illumination references (§12.3) and nothing
else — a periodic sequence cannot tell a decoder *where on the ring* it is looking.
Replacing the period with a sequence whose every `k`-position window is unique turns the
same cells into an absolute position reference: orientation, which edge, and how far along,
for **no extra modules**. Sequences are generated by `just debruijn-table` and committed to
`border_seq.rs`, following §8.2's rule for the LDPC graphs and for the same reason — the
construction is a backtracking graph walk with a budget.

The luma layer is what carries it, which §12.1 requires. The objection I expected to kill
this did not hold: §4.1 makes hue states iso-luminant by design, so a palette-cycling ring
ought to be invisible in luma — but P12 puts white and black in the cycle and the two luma
levels are far apart, so `C1` binarises across the full 0–255. Colour rides on top, chosen
from the entries at the required luma level, so **the sequence changes the order of the
border colours, not their coverage** — `every_reference_colour_appears` still passes
unchanged.

**Why distributed rather than a marked corner or a gap.** §3 makes specular glare the
dominant screen impairment and §12.3 already discards border references a highlight has
ruined. A localised asymmetry is a single point of failure at exactly the wrong scale. A
gap is worse still: occlusion *produces* gaps, so it would signal with the same alphabet the
channel uses for damage.

**Minimum window Hamming distance is 1, and that is not the operative number.** Separating
`p` windows by distance 3 is sphere-packing-bound at `p·(1+k) ≤ 2^k`, which would force a
12-position window at v1. But a decoder correlates whatever arc is unobscured against all
rotations rather than looking up exactly `k` cells. Measured that way — corrupting arcs and
requiring the true position to win outright, ties counting as failures — **~20 positions
survive two misreads at every version**. See [`border-sequence.md`](border-sequence.md).

**The border is therefore mandatory on screen**, where §6.4 made it optional. Once the ring
is what a decoder localises from, a symbol that omits it is a symbol that cannot be found
that way. QR spends the same area on a quiet zone and gets nothing back for it.
`EncodeOptions::print` still turns it off, so this is a screen-profile default rather than a
format-wide mandate — print has no border (§6.4 calls the overhead unacceptable there) and
keeps the finders.

**Format bit 14 is left alone for now**, still signalling the border and now always set on
screen. Freeing it means restructuring the format region, which is also where open #11a's
compact-format-region question lives; better to churn that region once than twice.

**Size cost: the scanner went 197,663 → 199,453 bytes, 96% → 97% of §14.2's budget.** The
table is ~1.7 KB of the 1.8 KB. Third bite this month after D12's `probe` and D13's
multi-candidate detection, leaving 5,347 bytes. **The budget is now the binding constraint
on this project**, and a ring-based detector will want room of its own.

**What this does not do: the finders are still there.** This is the groundwork that makes
removing them possible, not the removal. Detection is still §12.1's 1:1:3:1:1 run-ratio
scan, so nothing can yet localise a symbol from the ring, and a finder-less symbol would
decode nowhere. The measured prize, from the real layout: finders plus separators plus
anchors are a **fixed 204 modules**, which is 46.3% of a v1 symbol and 1.0% of a v32 — so
this is a small-symbol optimisation, and at v2 it is the difference between a 25×25 symbol
and a 21×21 one for the same payload.

Sequenced after the capture sweep deliberately: replacing detection before the sweep runs
would leave the new detector comparable only against simulator numbers.

### D15. Two size budgets, and the 200 KB was being read 2.5× too strictly

**Status:** closes the measurement half of open decision #16. `justfile`

§14.2 says "**< 200 KB gzipped** for decode only" and marks it `[TBD: validate]`. That one
number admits two readings and the difference is not small:

| basis | what it is | scanner, before this |
|---|---|---:|
| raw | what a host loads from disk and holds in memory (D8) | 199,453 — **97%** |
| gzipped | what crosses a network, which is §17.3's actual argument | 79,341 — **38%** |

The gate measured raw only. So a constraint that has been shaping real engineering — D12's
`probe`, D13's bounded candidate search, D14's sequence table — was being enforced on the
stricter basis while the other sat at 38% unmentioned. **Both are now gated and both are
reported.**

The raw limit is §14.2's number taken literally. The compressed limit is **a proposal, not
a measurement**: half the raw budget, because ~75 KB gzipped is already a heavy web
dependency and a gate that never fires is decoration. #16 cannot actually close until that
number is justified against a real embedder rather than chosen for symmetry.

**Savings found: 20,733 bytes, free.** `wasm-tools strip --all` on the built component.
Plain `strip` saves only 1,753 — it leaves the `name` section, which on the scanner is 19 KB
of debug symbols, 10% of the artifact, that no shipped decoder reads. Scanner 199,453 →
178,720 (**97% → 87%** raw, 72% compressed); the full build 304,965 → 276,068. The core
module at `target/…/lc_wasm.wasm` keeps its names, so a trap is still debuggable there.

**`opt-level = "z"` measured and declined.** The `wasm-min` profile has been defined and
unexercised since the project started. It works, and it costs:

| build | raw | gzip | decode median | p90 |
|---|---:|---:|---:|---:|
| release + strip | 178,720 | 74,637 | 79.4 ms | 80.5 ms |
| `wasm-min` + strip | 161,341 | 66,754 | **88.1 ms** | **98.0 ms** |

Eight more points of raw budget for **11% median and 22% p90 decode time**. Declined:
§1.1 goal 5 wants "real-time camera preview decoding on a mid-range phone", one 492×492
frame already costs ~80 ms through jco on a laptop, and a phone is slower. Size is not the
binding constraint at 87%; frame rate plausibly is. Revisit if the budget tightens or if
decode gets materially faster.

Measured under Node + jco, which adds glue overhead a native component host would not pay —
so treat the absolute milliseconds as indicative and the *ratio* as the finding.

### D16. Detection had an upper pitch limit, and now retries on a halved image

**Status:** fixes a defect the capture sweep found. `crates/lc-decode/src/lib.rs` (`scan`,
`PYRAMID_LEVELS`), `detect::candidates_from`, `image::downscale`

**How it was found.** Raising the sweep rig's capture width to 4K tripled the captured
module pitch, and nothing decoded at any distance — including close up, which is what made
it look total. The §15 harness had never rendered above 12 px/module, so nothing had tested
the top of the range.

**The ceiling, measured on clean renders at v2/v4/v8** (identical at all three, so it is
pitch and not version): decode worked at 8–14 and 18–22 captured px/module, failed at 16,
and failed at everything from 24 up. Shifting the image a single pixel — same pitch,
different phase between the binariser's 8-pixel block grid and the module grid — separated
two faults:

* **16 and 24 are `2·BLOCK` and `3·BLOCK`.** They failed only in phase; one pixel of offset
  fixed them. D6's mechanism, recurring at multiples D6 never tested.
* **A hard wall above ~24** that no offset rescued. Binarisation stayed healthy (dark
  fraction ~34% at every pitch); `find_finders` simply stopped locating finders, reporting
  two of three at pitch 30 with a module estimate of 26 against a true 30.

**The fix.** If the full-resolution scan finds no candidate, halve the greyscale image and
retry the finder search, up to two levels, each built from the one before. Halving halves
the pitch, so pitch 40 lands at 10. Finder positions are lifted back to full-resolution
coordinates and everything after the finder search — alignment, transform, sampling — runs
at full resolution, so nothing needing precision is computed from a shrunken image.

**Why two halvings rather than factors 1/2/3/4**, which was the first version: detection
works over pitch 8–22, so a factor of 2 covers 16–44 and a factor of 4 covers 32–88, which
already overlap — factor 3 was redundant. And building each level from the last means only
the first halving touches full resolution. **Coverage-neutral, measured**: 80 of 88 pitch ×
phase cells decode under both.

**Result, clean renders, 8 grid phases each:**

| px/module | 8–24 | 30 | 32 | 40 | 50 | 64 | 80 |
|---|---:|---:|---:|---:|---:|---:|---:|
| decodes | 8/8 | 4/8 | 7/8 | 8/8 | 7/8 | 7/8 | 7/8 |

Every pitch from ~24 up previously decoded at 0 of 8.

**Residuals, stated plainly.** Pitch 30 is a soft spot at 4/8. At pitch ≥ 32 with the grids
exactly in phase, detection now succeeds and the *format region* fails its BCH decode — the
same alignment fault, one stage later. `decodes_across_the_full_module_pitch_range` covers
8–24 at both phases and ≥ 32 at the off-phase only, and its doc comment says so. The real
fix for both is the binariser's fixed block grid, not another retry. Note that real optics
rarely land on an exact integer pitch in exact phase, so this is likelier to bite synthetic
renders than captures — but that is an argument, not a measurement.

**Cost, timed with every stage inside the same loop iteration at 1280×720:**

| frame | full-res scan | pyramid | full decode |
|---|---:|---:|---:|
| clean, no symbol | 22.3 ms | **+8.5 ms (+38%)** | 30.9 ms |
| noisy, no symbol | 31.2 ms | does not run | 72.3 ms |
| symbol found at full res | — | does not run | unchanged |

On a noisy empty frame, noise yields a spurious finder triple at full resolution, so `scan`
returns early and the pyramid never runs. Scanner size 178,720 → 180,270 bytes (+1,550);
88% raw, 73% gzip.

**A measurement mistake worth recording.** The first cost probe reported "+131%" and
blamed the pyramid, and a comment claiming so briefly sat in the source. It had been
comparing a bare scan against a full decode. The mipmap rewrite changed nothing in that
number — which is what exposed it — and an interleaved decomposition showed the 41 ms is
`Decoder::decode` walking D13's candidates through format reads on the spurious triple. So:

**D13's candidate walk is the larger live-preview cost.** 41 ms beyond the scan on every
noisy frame that finds nothing, and a live preview is mostly such frames (57–64% of sweep
frames located no symbol). Not addressed here; it predates D16 and is the better target
for scan-loop speed.

**What it unlocks.** Range is bought with sensor pixels per module, and this ceiling was
what made capture resolution unusable as a lever. `sweep.html` re-enables 1920, 2560 and
3840 with their decode cost on the label. Unmeasured on a real capture.

### D17. Finder-less symbols: the ring replaces finders, alignment, timing and format region

**Status:** implemented, unmeasured on a real capture. `lc-core` (`border::RingMode`,
`Layout::finderless`, `capacity_for_symbol`), `crates/lc-decode/src/ring.rs`, `prepare_ring`
in `crates/lc-decode/src/lib.rs`. Golden vectors regenerated: the 29 ringed colour vectors
changed; `profile-m-border` and the border-less vectors did not.

**What changed.** A symbol is finder-less exactly when it carries the §6.4 ring and its
profile is not `M`. Its layout holds payload and reference modules only — no finders,
separators, palette anchors, alignment marker, timing, or in-grid format region. Those were
204 fixed modules plus the 60-module format region at every version, 62% of a v1.

**Everything they did now comes from the ring:**

* **Where.** Every version's position sequence forces its four corner positions dark
  (`forced_dark` in `xtask/src/debruijn.rs`), so the convex hull of a symbol's ink has the
  ring's outer corners as its corners. Each corner is then refined by intersecting lines
  fitted to the middles of the adjacent edges.
* **Which version, which way up, mirrored or not.** Ring luma is sampled under each version's
  geometry and correlated against that version's sequence at the four corner-aligned shifts,
  both directions. Accepted at ≥ 0.85 agreement with a ≥ 0.08 lead over the runner-up.
* **The format.** Every third ring position is a fixed-colour reference; the rest carry two
  interleaved copies of the 30-bit masked BCH format codeword as Gray-labelled hue choices.
  Illumination is fitted from the references alone — their colours do not depend on the
  format — both palette families are tried, and soft BCH picks the one that decodes to the
  version the correlation named.

Decode tries the ring first and the finder path second. The CRC stays authoritative.

**What it bought — the gallery, same payloads, same 12 px/module:**

| Symbol | JSON | With finders | Finder-less |
|---|---:|---|---|
| `contact` | 34 B | v2 25×25 | **v1 21×21** |
| `location-fix` | 54 B | v2 25×25 | **v1 21×21** |
| `allergen-profile` | 66 B | v3 29×29 | **v1 21×21** |
| `device-pairing` | 67 B | v3 29×29 | **v1 21×21** |
| `membership` | 96 B | v3 29×29 | **v2 25×25** |
| `device-config` | 144 B | v4 33×33 | **v3 29×29** |
| `event-admission` (C1) | 144 B | v4 33×33 | **v3 29×29** |
| `profile-c2` | 144 B | v5 37×37 | **v4 33×33** |
| `profile-c3` | 144 B | v3 29×29 | **v2 25×25** |
| `profile-m` | 144 B | v6 41×41 | v6 41×41 — keeps finders |

As the groundwork section predicted, it is a small-symbol win: one or two versions at the
sizes QR-style payloads actually occur at.

**The trade: a greyscale capture cannot read one.** The format rides on hue. A mono sensor
can still *locate* a finder-less symbol, but cannot read its format, and
`finderless_symbol_through_a_grey_sensor_fails_closed` pins that it fails cleanly rather
than guessing. §1.1 goal 3 now holds for border-less symbols and for profile `M`, which is
why `M` keeps its finders even with a ring: it has no hue to carry the format on.

**The first detector failed on clean renders, and `tests/ring_diag.rs` found three faults.**
It traces every stage on the cases that failed; it is `#[ignore]`d and prints rather than
asserts.

1. **Ring luma was read from the shared adaptive binary.** The ring sits in white quiet
   zone, so a binarisation block that is mostly white thresholds between a light hue (~188)
   and white, and light ring cells binarise *dark*. True-version agreement on clean renders
   was 0.67–0.89, and every clean render at 8, 20, 64 and 80 px/module was rejected. Luma
   now comes from greyscale, thresholded by a two-means split over ±16 ring positions:
   **1.000 at every pitch tested**.
2. **The hull was built from the same binary.** Its 24-level dynamic-range floor is below
   sensor noise in flat white, so at σ = 0.10 the quiet zone binarised as speckle and all
   twelve noisy frames' hulls ran to the frame corners; and one global hull spanned two
   symbols. The detector now builds its own ink map: 5th/95th-percentile levels per 16 px
   cell pooled over 7×7 cells, a contrast floor of 80 levels, ink meaning anything more than
   20% of the local range below paper white (so a symbol is one solid region, light hues
   included), and connected regions on an 8 px grid. Each region's hull is tried largest
   first, and the union of all ink last.
3. **Hull corners sat up to 4 px off under blur r = 4.** A third of a module at 12 px/module:
   the format read perfectly and the payload CRC failed. Hence the edge-line refinement.

**Result.** Roundtrip 17/17 and degradation 12/12, including defocus, the realistic
combination, and two symbols in one frame. Single-frame noise, measured on v3 `C1` at 12
px/module:

| σ | 0.35 | 0.36 | 0.37 | 0.38 | 0.39 | 0.40 |
|---|---:|---:|---:|---:|---:|---:|
| single frame decodes | 6/6 | 6/6 | 3/6 | 3/6 | 0/6 | 0/6 |
| 12 accumulated frames | yes | yes | yes | no | no | no |

That left accumulation under 0.02 of σ in which to beat a single frame, so
`multi_frame_beats_single_frame` now uses glare that moves between frames (radius 0.3,
strength 1.0: 0/12 single frames decode, 12 accumulated do) — the impairment §12.4 names.
Its old comment put the single-frame limit at σ ≈ 0.06; a classic symbol measured now
decodes 11/12 at σ = 0.10, so that figure was already stale for finders too.

**Scanner size: 180,270 → 203,741 bytes, 99% of the raw budget, 82% compressed.** The first
working detector put the scanner at 237,766 (116%, over). It had to earn its way back
without D15's declined `opt-level = "z"`. Every cut below was measured, and none changed
behaviour: roundtrip, degradation and all 24 diagnostic cases were re-run after each.

| change | bytes saved |
|---|---:|
| Gift-wrapping hull instead of a sorting one; bounded region selection instead of a sort; closed-form edge fit instead of libm trig; mask-then-scan boundary points | 24,364 |
| `detect::find_finders`' stable sort replaced by an insertion sort with identical order (pre-D17 code) | 6,633 |
| Shoelace orientation instead of an angle sort in `quad`; family comparison instead of rebuilding a palette in the format check; radius flood fill instead of a dilation pass | 3,028 |
| `#[inline(never)]` on the ring helpers — tried, **cost 582**, reverted | — |

**That leaves 1,059 bytes, which is not room for the next feature.** The largest lead
found: about 11 KB of float formatting (`core::fmt::float`'s Dragon and Grisu paths),
reached only through `<f32 as Debug>::fmt`.

**Traced afterwards, and it was not ours to delete.** Parsing the shipped binary — data
section, element segments and every `i32.const` in the code section — finds that function in
the indirect function table with **no live vtable and no caller**: the linker retained it and
its Dragon/Grisu dependencies, and no edit to this project's source reaches it. Removing it
needed a tool that does whole-module dead-code elimination, which is what D20 added; that is
where the budget pressure actually went, not into another round of hand-shaving.

**Limits, stated plainly.**

* **A region is assumed to be one symbol.** Two symbols whose quiet zones touch, or a symbol
  over printed text or other dark scene content, merge into one region whose hull is not a
  ring. That fails safely — it does not correlate — but it fails.
* **Pitch coverage was not re-measured on D16's 88-cell grid.** Measured: the diagnostic's
  clean renders at 8, 12, 20, 64 and 80 px/module across the phases it tries, and the
  roundtrip range test.
* **Cost not timed.** The ring attempt runs before the finder scan on every frame, including
  empty ones and classic symbols.
* **Unmeasured on a real capture.** The sweep is the test, and real scenes are where the
  one-region assumption will be tried.

### D18. One module between the ring and the grid, not two

**Status:** implemented. `border::GAP`, the renderer's canvas margin, the decoder's ring
transform, and all 32 regenerated sequences.

**What changed.** The white gap between the symbol grid and the §6.4 ring is one module.
The quiet zone *outside* the ring is still §6.1's two, and border-less symbols are untouched
— `QUIET` stays 2 and a new `GAP` carries the inner width. A bordered canvas goes from
`side + 12` modules across to `side + 10`.

**Why two was not load-bearing.** §6.1's quiet zone exists to isolate the **finders** for the
1:1:3:1:1 run-length scan, and a finder-less symbol (D17) has none; the ring is found by its
outer edge, which faces the other way. Nothing samples the gap — no white reference, no
geometry. Its one remaining job is keeping ring colour out of the grid's edge modules under
defocus, and one module is several times the ~⅓ module a blur at the §13.1 pitch floor
spreads.

**What it buys.** Nothing in payload: the grid is fixed by version. It buys *area*, or
equivalently module pitch at a fixed size on screen — and pitch is range:

| | v1 | v2 | v5 | v32 |
|---|---:|---:|---:|---:|
| canvas, modules across | 33 → 31 | 37 → 35 | 49 → 47 | 157 → 155 |
| area saved | 11.8% | 10.5% | 8.0% | 2.5% |

**What it cost, measured.** The defocus envelope, by ~10%: blur radius ~0.33 modules before,
~0.30 now. `ring_diag::defocus_and_glare_envelope` across pitches:

| px/module | r=2 | r=3 | r=4 | r=5 | r=6 |
|---|---|---|---|---|---|
| 12 | ok | ok (0.25) | **fails** (0.33) | — | — |
| 14 | ok | ok | ok (0.29) | fails (0.36) | — |
| 16 | ok | ok | ok | ok (0.31) | fails |
| 20 | ok | ok | ok | ok | ok (0.30) |

**The cost is geometric, not optical.** Payload sampling never sees ring colour: the luma
kernel's taps sit 3 px inside the grid edge, a radius-4 blur reaches 1 px past them, and the
ring is 12 px away. What changed is the reference square the homography is fitted from — the
ring's outer square is now 31 modules across rather than 33, so the same 2–3 px corner error
under blur is a larger fraction of it. `survives_defocus` states this and keeps a radius-4
case at 14 px/module rather than dropping it.

`multi_frame_beats_single_frame` needed its glare strengthened from 1.0 to 1.2: on the
smaller canvas the blob covers proportionally less symbol, and 2 of 12 single frames were
surviving. Everything else is unchanged — roundtrip 17/17, degradation 12/12, 555 tests.

**Every version's ring got shorter** — v1 112 → 104 positions, v2 128 → 120, v32 608 → 600 —
so all 32 sequences were regenerated and re-verified (`docs/border-sequence.md`).

**A generator bug this surfaced, worth recording.** `xtask`'s `ring_positions` carried its
own copy of the canvas geometry (`side + 12`), so after the gap narrowed it kept emitting
sequences for the *old, longer* ring: every version's table was quietly wrong, and the
decoder failed with `FormatUndecodable` on symbols it had just rendered. lc-core's
`every_sequence_window_is_unique` caught it. The generator now calls `border::positions`, so
there is one definition of the ring's length rather than two.

**Scanner:** 203,649 raw (99% of budget), 84,898 gzip (82%).

**Spec deviation, stated.** §6.1 says "quiet zone: 2 modules on all sides". For a ringed
symbol the outer two remain and the inner gap is one. Open decision #10 — whether 2 is
enough against textured backgrounds — is about the *outer* margin and is untouched by this.

### D19. The ring's inner layer carries payload

**Status:** implemented. `border::payload_cells`/`payload_cell_index`, `Layout::ring_cells`,
`Symbol::ring`, `modulate`/`demodulate`, the renderer's `canvas_color`, and the decoder's
`demodulate`, `collect_references` and ring reads. Golden vectors are now schema 2.

**What changed.** P12 gave every ring position two radial layers of the same colour — a
2-module-deep sampling target. That depth is now kept only where it is load-bearing: the
**reference positions** (every third) and the **four corners**. A data position's inner cell
is an ordinary payload module: one luma bit plus the profile's chroma bits, riding in the
tail of the same two codewords the grid uses, so the existing LDPC codes and the §8.3
interleaver spread them with no special handling. `C2` takes luma only — its 2×2 chroma cells
cannot be formed in a band one module deep.

**What it buys, in body bytes at default ECC:**

| | v1 | v2 | v3 | v5 | v32 |
|---|---:|---:|---:|---:|---:|
| `C1` | 62 → **73** (+17.7%) | 92 → **106** (+15.2%) | +8.6% | +7.6% | +2.0% |
| `C3` | 92 → **108** (+17.4%) | 135 → **152** (+12.6%) | +9.8% | +7.6% | +1.9% |

In the gallery: `membership` v3 → v1, `event-admission` v3 → v2, `profile-c3` v2 → v1.

**Why any of the ring stays 2-deep, measured rather than assumed.** The probe repainted every
data position's inner cell with unrelated palette colours — what the ring would look like
carrying payload — and had the decoder read data positions from the outer layer only:

| case | sequence agreement | format margin | decode |
|---|---|---|---|
| clean, blur r=3, blur r=4 | 1.000 | 224 | ok |
| noise σ = 0.10 / 0.20 | 1.000 | 175 / 160 (was 224) | ok |
| realistic combination | **0.941** (was 1.000) | 224 | ok |

Nothing that passes today stops passing; what shrinks is cushion, and it shrinks on the
hardest case — 0.941 against the 0.85 accept threshold. Giving up the whole inner layer,
references included, was worth +18.5% at v2 against the hybrid's +15.2%: three bytes, for the
margins above and for the illumination fit's known-colour samples. Not a good trade.

**Defocus came back.** D18 cost a step of the blur envelope; D19 returned it on the
degradation fixture, because the 96-byte payload moved from v3 to v2 and now sits in a
shorter, more redundant code — radius 5 at 12 px/module (0.42 modules) decodes where radius 4
had stopped. The envelope is a property of the fixture as much as of the format, and
`survives_defocus` says so.

The multi-frame test's glare had to grow again, to radius 0.35 at strength 1.2: a
fixed-fraction highlight covers less of a smaller symbol.

**Wire format.** The grid array no longer describes the whole symbol, so the vectors carry
`ring` — one palette index per ring payload cell — and the schema is 2. 31 of 34 vectors
changed.

**Limits, stated plainly.**

* **Payload now rides on the most glare-exposed part of the symbol.** Interleaving spreads
  it, but a highlight across one arc of the ring costs payload bits as well as format margin.
  The probe above is synthetic; a real capture has not been run.
* `C2` gains luma only, and profile `M` and border-less symbols gain nothing.
* **The outer layer's spare hue slots are still unused** — every data position has
  `log2(hues)` bits of colour choice and the format codeword uses 60 of them, leaving 2–4%
  more capacity on the table. Cheap to add now the plumbing exists; not done here.

### D20. Binaryen's size pass in the build, on the core module

**Status:** implemented, optional. `justfile` (`wasm_opt_flags`, `_component-full`,
`_component-scanner`).

**Why it exists.** D19 put the scanner 1,376 bytes over §14.2's raw budget, and hand-shaving
had run out: 1,185 bytes came back from deduplicating an LLR function, dropping an
allocation, and collapsing a dead branch, and the next largest item — 11 KB of float
formatting — turned out to be linker-retained code with no live caller that no source edit
reaches (see D17). Whole-module dead-code elimination is the tool for that, and binaryen has
it.

**What it does, measured:**

| artifact | without | with | change |
|---|---:|---:|---:|
| scanner | 206,176 (101%) | **182,324 (89%)** | −23,852 |
| full | 305,609 | **272,729** | −32,880 |

**And it is faster, which was the open question.** D15 declined `opt-level = "z"` because it
cost 11% median decode time; a size win has to be priced the same way. Timed through
wasmtime, 372×372, 20 frames after a warm-up (`decode_cost_through_the_component`):

| build | per frame |
|---|---:|
| without wasm-opt | 26.88 ms |
| with wasm-opt | **23.01 ms** |

14% **faster**, not slower — which is the difference between changing rustc's codegen and
running a post-pass over it. Measured under wasmtime rather than jco, so read the ratio.

**Two things about how it is wired.**

* **On the core module, before `wasm-tools component new`.** Binaryen does not read
  component-model binaries.
* **Exactly the features rustc emits — `--enable-bulk-memory --enable-sign-ext
  --enable-mutable-globals --enable-nontrapping-float-to-int` — and not `-all`.** With
  `-all`, binaryen optimised into instructions wasmtime rejects by default and every
  component failed to parse at load. `wasm-tools validate --features=component-model`
  **passed that broken artifact**; what caught it was `cargo test -p lc-wasm`, which
  instantiates what is actually shipped. The lesson is about the gate, not the flag.

**Optional by design.** Without binaryen on `PATH` the build prints a note and carries on, so
a contributor without it still builds; with it, a failure is a hard failure rather than a
silently unoptimised artifact. The budget figures above and in D19 assume it is present,
which is worth remembering if CI ever runs somewhere it is not.

### D22. Luma is read a second time, against its own neighbourhood

**Status:** implemented. `crates/lc-decode/src/demod.rs` (`add_local_luma`, `local_fit`).
Decoder-only; no wire-format change.

**How it was found.** `docs/robustness-ideas.md` §3.4 proposed a hue-angle chroma
likelihood for glare, borrowed from JABCode's brightness-normalised classifier: glare adds
white, white pulls `C*` toward zero but mostly keeps hue angle, so read the angle. Before
building it, `--example glare` split every glared failure by the stage that gave up. **Not
one failure was chroma's.** Across `C1`/`C2`/`C3` at v4 and every specular strength and
radius swept, a located symbol failed on the luma layer or not at all. The hue-angle idea
has no target in the harness, and was not built.

**Why luma, measured per module.** A throwaway autopsy scored each glared LLR against the
clean render's decisions (`C1`, strength 1.2, radius 0.3, 8 trials):

| | luma | chroma |
|---|---:|---:|
| LLRs with the wrong sign | 9.6% | 1.7% |
| …of those, with confidence (`\|llr\| > 4`) | **0** | **0** |
| luma modules effectively erased (`\|llr\| < 1`) | 33% | — |
| wrong luma modules that were *not* clipped | 693 of 897 | — |

`ChannelStats::reliability` was already doing its job: nothing went in confidently wrong.
The problem is volume. Inside a highlight's halo no low-order §12.3 surface can follow the
blob, the corrected sample matches no palette colour, and a third of the luma layer is
erased. Chroma's two-step stronger code (D10) absorbs its 1.7%; luma's cannot absorb 33%.
And most of the damage is in the *unclipped* halo, not the clipped core.

**The fix: glare is additive, so the level gap survives it.** In linear light a reflection
adds the same light to a dark module and a light one, so the *difference* between the
levels is unchanged even where the global fit has lost both. Around every module,
`local_fit` fits

```text
Y  =  p0 + p1·dx + p2·dy  +  p3·bit        over its 7×7 payload neighbours
```

(an additive plane for the halo, plus a level step), refits until the classification stops
moving, and gives the module an LLR from where it sits against the local step. That LLR is
**added** to the global one.

**Each design choice was measured, and several plausible ones lost.** Luma convergence out of
16 trials (`C1` unless marked):

| variant | 1.2 / 0.30 | 1.6 / 0.30 | 0.8 / 0.6 | 0.6 / 0.50 | `C3` 0.9 / 0.30 |
|---|---:|---:|---:|---:|---:|
| global only (before) | 6 | 1 | 0 | 0 | 8 |
| flat two-level split, blended | 10 | — | — | 16 | 16 |
| plane + step, clipped samples excluded from the fit *and* clipped modules erased | 13 | — | — | 15 | 16 |
| plane + step, clipped modules erased | 14 | 9 | — | 16 | — |
| plane + step, local LLR **replaces** global | 9 | 0 | 16 | 16 | 16 |
| plane + step, local blended in where global is weak | 16 | 3 | 7 | 16 | 16 |
| **plane + step, local added to global** | **16** | **8** | **16** | **16** | **16** |

(columns: specular strength / radius. These came from the autopsy harness, before the colour
check below existed; the shipped version's figures are under *Result*.)

- **A plane, not a flat split.** Across a steep halo a dark module on the bright side outshines
  a light one on the dim side. The harness's radius-0.3 highlight peaks at ~0.06 linear per
  module, which is enough for that to happen.
- **Clipping is evidence, not an erasure.** JABCode-style clipping erasures were the second
  half of §3.4, and they measured *worse*. A light module clipped to white is still brighter
  than a dark one that did not clip. Information is only lost where both levels clip, and
  there the fitted step collapses and the `(p3 / gap)²` factor fades the LLR out by itself.
  The one cell where erasing clipped modules won, strength 1.6 (9 against 8), is past where
  anything decodes reliably; it lost at 1.2, the cell that matters.
- **Added, not substituted.** When a wide highlight half-fits the global surface, the global
  read is confidently wrong (|LLR| ≈ 5) and the local read is right. A sum lets the local read
  overrule it; a blend weighted by global confidence cannot.

**Luminance alone cannot tell glare from an occluder.** The first working version regressed
occlusion badly: the harness's grey patch at 0.35 of the symbol went 100% → 34% on `C1`. A
dark finger, which matches the dark level's luminance, went 100% → 72% at 0.30. The local
read was vouching for exactly the modules `reliability` exists to erase. The fix reuses
that test. The fit's additive offset and gain are undone on the module's full colour, and
the de-glared colour must look like a palette entry to `ChannelStats::reliability`. A glared
module de-glares back to itself; a finger does not. A monochrome capture or profile `M`
skips the check, as the global read does. Both cases are pinned by unit tests
(`local_read_sees_through_additive_glare`, `local_read_does_not_vouch_for_a_dark_occluder`).
The second of those also caught a real bug: three fixed refit passes, with no refit after
the last reclassification, left the plane biased on a steep ramp.

**Result.** `--example glare`, v4 at full capacity, 32 trials per point:

| | before | after |
|---|---|---|
| `C1` strength 1.0 / 1.2 (radius 0.3) | 91% / 34% | 100% / 100% |
| `C1` radius 0.40 / 0.50 (strength 0.6) | 19% / 0% | 100% / 100% |
| `C3` strength 0.9 / 1.0 / 1.2 | 56% / 28% / 19% | 100% / 100% / 100% |
| `C3` radius 0.35 / 0.40 / 0.50 | 28% / 0% / 0% | 100% / 100% / 100% |

All 48 glare cells across the three profiles now decode 32/32. `just profiles 32`:

| m50 | `C1` | `C2` | `C3` |
|---|---|---|---|
| specular radius | 0.370 → **> 0.40** | 0.366 → **> 0.40** | 0.342 → **> 0.40** |
| blur radius | 4.758 → 4.758 | 5.864 → **> 6.0** | 4.704 → 4.704 |
| noise, specular strength, shading, occlusion | unchanged | unchanged | unchanged |

`C2`'s blur gain was not aimed at and is not explained yet. Its luma is 89% of its payload,
and defocus also leaves a smooth local bias that a plane absorbs, which is a plausible
account but not a measured one. `just envelope 32` (`C1`, 96 B) moves specular strength 0.9
from 75% to 100% and radius 0.40 from 16% to 100%. Occlusion at 0.30 / 0.35 reads 94% / 84%
against 97% / 81% before: one trial each way, inside the ±9-point error bar. `--example ab`,
which adds a dark occluder, shows nothing else moved beyond one trial.

**Cost.** The local fit is ~2 µs a module, a third of a v32 decode if run everywhere, so it
is skipped wherever the global LLR is already ≥ 12 (`LOCAL_SKIP`); a module that confident
is one `LOCAL_CLAMP` = 8 could not overturn. Native, `--example decode_time`:

| | v4 clean | v4 glared | v32 clean | v32 glared |
|---|---:|---:|---:|---:|
| before | 26 ms | 95–100 ms | 147–154 ms | 360 ms |
| after | 26 ms | 28 ms | 146–151 ms | 176 ms |

Clean frames are unchanged. Glared frames got *faster*, because a frame that decodes stops
early, where the old one burned LDPC iterations and D13's candidate retries on failing.
**Scanner: 185,771 → 190,203 bytes, 90% → 92% raw, 83% → 85% gzip.**

`multi_frame_beats_single_frame` needed its glare raised a third time, from radius 0.35 /
strength 1.2 to 0.45 / 1.6. At the old setting 9 of 12 frames now decode on their own.

**Limits, stated plainly.**

- **The harness's glare is neutral white.** The de-glare treats the offset as neutral light. A
  tinted reflection, such as warm ceiling lights, will de-glare imperfectly. The colour check
  then fades the local read out, which is the safe direction, but the gain will be smaller
  than measured here. Sweep run 4 (`glare-c1`) is the test.
- **Unmeasured on a real capture**, like everything since D17. The face-on sweep bins (5%
  decoded) are what this is aimed at, and only a sweep can say how much of that was this
  failure mode rather than detection.
- **Ring detection under glare is untouched.** Every harness failure here was a located
  symbol. On real captures, 57–64% of frames found nothing, and this does nothing for those.

### D23. Ring geometry from edges, and a lens model

**Status:** implemented. `crates/lc-decode/src/ring.rs` (`edge_quad`, `estimate_lens`),
`crates/lc-decode/src/transform.rs` (`Transform::with_radial`). Decoder-only.

**How it was found.** `--example stages` bins every failed trial by the first stage that gave
up, across impairments the §15 envelope did not have: damage *at a ring corner*, and lens
distortion (`barrel` in `tests/support/ops.rs`, new). Both were severe:

| v4, 16 trials | before |
|---|---|
| a patch 6% of the canvas wide over one ring corner (~2.6 modules) | **0%**, `C1` and `C3` |
| a highlight on one ring corner, strength 0.8 | 6% / 0% |
| 10% barrel distortion | 0% `C1` |
| v32 at **2%** barrel — a phone main camera's residual after its own correction | 0% |

**Why corners.** The ring's quad came from the four extreme points of its ink hull. A patch
over a corner is ink and pulled that corner outward (24 px → 9 px on a 516 px canvas). A
highlight erased the corner and chopped it inward (24 → 47 px). `refine_quad` could not
repair either, because it starts from the hull's corners and refuses to move one more than 8
px. The occluded case was worse than a rejection: the ring was *accepted* at 0.875 agreement
with a wrong transform, and the format or payload failed downstream.

**Edges, not corners.** `edge_quad` fits each edge by consensus: deterministic trial lines
through pairs of boundary points near the hull's version of the edge, keeping the line most
points agree with. It then refits over the edge's middle and intersects neighbouring edges.
An occluded or erased corner is reconstructed from the two edges that meet there; every
corner case above lands exactly on the true corner.

**Two measured refinements.**

- **Used only when the two quads disagree** (> 3 px, `CORNER_AGREE_PX`). Used everywhere, it
  cost `C3` half its blur-radius-4 decodes (88% → 44%). Under blur, light and dark ring cells
  smear the boundary into two parallel populations, and the consensus line ran through one
  of them. On an undamaged ring, `refine_quad`'s corners are the more precise.
- **The refit centres the line.** After the consensus, a wide band then a tight one, as
  `refine_quad` does. That removes the one-population bias.

**Lens distortion.** `Transform` may now carry one radial coefficient, Brown's first-order
model about the frame centre. The homography maps into an undistorted image and `apply`
re-distorts. Estimating it is closed-form: a straight edge at distance `h` from the centre,
of length `L`, bows by `κ·h·L²·t(1 − t)`. `estimate_lens` fits that per edge, alongside free
line terms so corner positions drop out, and refines once on straightened points. The
boundary points are undistorted before any quad is fitted. Three things had to be right,
and each was found by a regression:

- **Free line terms, not the hull's chord.** Measured against the chord, a chopped glare
  corner read as distortion and cost `C3` 100% → 69% under strong corner glare.
- **All four edges must agree.** A lens bends every edge; damage bends one. Each edge's κ
  carries a standard error from its own residuals, edges combine by inverse variance, and an
  edge more than 3σ + 25% from the estimate vetoes it.
- **Agreement against the total κ.** The refining pass measures a small residual. Slack
  proportional to that rejected every real lens on its second look, which broke
  distortion-plus-blur entirely until it was fixed.

Below `κ·R² = 0.004` (`LENS_MIN`) the estimate is dropped, and an undistorted capture takes
exactly its old path. Every non-distortion row of the breakdown is identical with and
without the lens model.

**Result.** `--example stages`, v4, 16 trials (lens rows at v16/v32 are 8 trials):

| | before | after |
|---|---|---|
| corner occlusion 0.06 / 0.10 / 0.14 | 0 / 0 / 0% | 100 / 100 / 100% (`C1`, `C3`) |
| corner glare 0.8 / 1.2 / 1.6 | 6 / 0 / 0% `C1`, 0 / 0 / 0% `C3` | 100% all |
| shading 0.80 (`--example ab`, 32 trials) | 47% `C1`, 0% `C3` | 100%, 100% |
| shading 0.85 (`stages`) | 0%, 0% | 88% `C1`, 75% `C3` |
| barrel 0.10, v4 | 0% `C1` | 100% |
| barrel 0.05, v16 | 0% | 100% (and 0.10) |
| barrel 0.02 / 0.05 / 0.10, v32 | 0 / 0 / 0% | 100 / 100 / 100% |
| pincushion, and a symbol off-centre in a frame twice its size, to 0.10 | — | 100% at v4 |
| barrel + blur 3, 0.05 / 0.10 | 0 / 0% `C3`, 100 / 0% `C1` | 100% all |

The shading gain was not aimed at: a dark corner under 80% shading falls below the ink map's
contrast floor and chops the hull exactly as glare does. Pincushion 0.10 at v16 still fails,
because it pushes the ring's corners off the frame; a real camera framed that way would clip
them too.

**Cost.** Edge fit 3.4 KB and lens 3.5 KB of scanner. Clean decode time +1–4%, as both run
on every located ring.

### D24. Neighbour bleed removed by decision feedback, after the CRC rejects

**Status:** implemented. `crates/lc-decode/src/equalize.rs`, the retry in `Decoder::finish`,
and `DecodeResult::codeword` in `lc-core`. Decoder-only.

**How it was found.** The same breakdown: at blur radius ≥ 5 on v4, **every** lost decode had
a converged luma layer and a failed chroma layer, on `C1` and `C3` alike (16/16 each).
`robustness-ideas.md` §3.3's "decision-directed refit" was the starting point, but blur's
damage is not an illumination error. It is inter-symbol interference: each module's colour
carries a few per cent of each neighbour's, and `C1`'s two hues are near-opposites.

**An oracle first.** A throwaway probe equalised chroma using the *true* neighbour colours, to
find the ceiling before building anything. At radius 5, 85–112 of 1,170 chroma bits were
wrong; equalised, 0–13, and every trial converged. A realistic variant was then given only
what a decoder has: levels from luma, hues from a crude first-pass decision. **It matched the
oracle to within two bits on every trial**, at radius 5 and 6. The fitted kernel is small
(centre ≈ 0.93, each edge neighbour ≈ 0.08), and that is enough to flip `C1`'s hue decisions.

**The mechanism.** Once luma converges, the decoder knows:
- every payload module's level, from the codeword;
- every cell's nominal luminance, because the hue states are iso-luminant (§4.1) and the
  fixed cells, ring, gap and quiet zone are known a priori;
- so the blur kernel itself — centre, edge and diagonal weights plus a bias, fitted on
  luminance over thousands of cells.

It then subtracts each chroma module's estimated neighbours and re-decides, twice. The same
idea also rescues **luma** when luma fails, using its own confident decisions
(|LLR| ≥ 4) for the kernel fit. That moved the limit from radius 6 to 7.

**Two implementation findings.**

- **The CRC gates the retry, not convergence.** The first version retried whenever chroma did
  not converge. On a v8 frame that had always decoded, chroma ran 50 iterations
  unconverged, *and the CRC passed anyway*, because the bits were right. Retrying there added
  12 ms for nothing. Now the retries run only after the CRC rejects the first pass, so every
  frame that decoded before takes exactly its old path.
- **No encoder in the scanner.** Re-deriving each module's level first used `Code::encode`,
  which brought the LDPC encoder into the decode build. A converged decoder's hard
  decisions on every transmitted bit *are* the codeword, so `DecodeResult` now carries them
  (`codeword`), pinned by `converged_codeword_is_the_transmitted_one`. That saved 2.7 KB.

**Result.** `--example stages`, v4, 16 trials:

| blur radius | 4 | 5 | 6 | 7 | 8 |
|---|---|---|---|---|---|
| `C1` before → after | 100 → 100 | 0 → **100** | 0 → **100** | 56 → **100** | 0 → **69** |
| `C3` before → after | 88 → **100** | 0 → **100** | 0 → **100** | 0 → **100** | 0 → **88** |

`--example ab` agrees: blur 5 goes 3% → 100% on `C1` and 0% → 100% on `C3`. 4:2:0 plus blur 4
on `C3` goes 94% → 100%. Noise is unchanged, as it should be, because noise is not bleed.
Nothing else moved.

**Cost.** ~14 KB of scanner, mostly unrolled three-channel arithmetic and ring-colour lookups.
With D23's 7 KB, that took the scanner from **190,203 to 215,546 bytes**, past §14.2's 200 KB.
**The budget was raised to 250 KB raw (256,000), and 125 KB gzipped, on 2026-10-05**
(`justfile`; D15's half-of-raw rule for the compressed limit). Time, native, min of four
interleaved runs:

| | clean | blur 5 |
|---|---|---|
| v8 | 42.6 → 43.4 ms | 48.4 → 49.0 ms (decodes either way) |
| v16 | 67.5 → 68.6 ms | 231 ms failing → **114 ms decoding** |
| v32 | 145 → 151 ms | 494 → 555 ms, failing either way |

**Limits, stated plainly.**

- **Single-frame only.** `FrameAccumulator` has no single frame to go back to, so it passes
  `None` and never retries. Equalising each frame before accumulating would be the natural
  extension.
- **`C2` is excluded.** Its 2×2 chroma cell averages over its own four modules, and it
  measured no chroma-only blur failures.
- **The harness's blur is a box.** A real defocus kernel is closer to a disc with soft edges.
  The kernel is *fitted*, not assumed, so the shape matters less than the extent. Still
  unmeasured on a real capture.
- **The finder path also passes its ring mode** and would equalise a classic symbol, but no
  classic-symbol case was measured.

### D25. Equalisation for `C2` and for accumulated frames, turbo rounds, and a defocus fallback

**Status:** implemented. `crates/lc-decode/src/equalize.rs` (`Observation`, `Hue`,
`chroma_hues`), the retry loops in `Decoder::finish`, `FrameAccumulator`, and
`ring::accept`'s relaxed rule. Decoder-only.

Four additions to D24, each measured with `--example stages` (v4, 16 trials) and the new
`--example multiframe`, plus four things that were tried and lost.

**1. `C2`.** D24 left `C2` out, because its 2×2 chroma cell averages its own modules. The
luma half of D24 already applied, and it was most of `C2`'s gain: blur radius 6 / 7 / 8 went
69 / 6 / 0% → 100 / 100 / 69%. Chroma now works for `C2` too. Each payload module's hue has
a *source*:
- shared with its chroma cell, decided from the mean of the cell's equalised modules;
- its own, for the ring's cells on one-module profiles;
- or fixed at 0, for modules outside any chroma cell and `C2`'s ring cells, which the encoder
  always draws at hue 0.

A `C2` cell is read from the mean of its equalised modules. That took blur 8 from 69% to 75%
on its own.

**2. Turbo rounds.** The luma retry started from the channel's own decisions, and past
radius 8 too many of those are wrong for the neighbour estimates to help. A decode that does
not converge still has far better posterior decisions than the channel, and
`DecodeResult::codeword` carries them. So each round equalises against the levels the last
decode settled on, then decodes again. Chroma does the same with hues, mapped back from its
posterior by `chroma_hues`. `levels_and_hues_invert_modulate` pins both inverse mappings
against `modulate` on every profile.

| blur (48 trials, final build) | `C1` | `C2` | `C3` |
|---|---|---|---|
| 8, before → after | 69 → **100** | 75 → **100** | 88 → **98** |
| 9, before → after | 0 → **88** | 0 → **98** | 0 → **79** |
| 10, before → after | 0 → **25** | 0 → **31** | 0 → 0 |

("Before" is D24 at 16 trials. The final figures include item 5 below.)

Three rounds left `C3` at 31% at radius 9; five reached 88%; eight gained nothing over five.
`TURBO_ROUNDS = 5`, and a round whose decisions equal the last one's ends the loop.

**3. Accumulated frames.** Accumulating LLRs averages away noise, but blur is a bias that is
the same in every frame, so **`FrameAccumulator` decoded 0% under blur radius 7–8 even where
single frames decoded 100%.** It had no frame to go back to for D24's retry. It now keeps an
`Observation`: every payload module's corrected colour, summed over frames. That costs
nothing per frame, because demodulation already samples exactly those colours
(`Demodulation::colours`); `push` timed the same as before at v4, v16 and v32. `try_decode`
runs the same retry against the mean. `--example multiframe`, 8 trials × 12 frames, blur
fixed, fresh sensor noise each frame:

| | single frames | accumulator before | after |
|---|---|---|---|
| `C1` blur 8, σ 0.05 | 100% | 0% | **100%** |
| `C1` blur 9, σ 0.10 | 23% | 0% | **50%** |
| `C1` blur 8 + glare wandering between frames | 0% | 0% | **100%** |
| `C3` blur 8, σ 0.10 | 14% | 0% | **88%** |
| `C3` blur 9, σ 0.05 | 39% | 0% | **75%** |
| `C3` blur 8 + wandering glare | 0% | 0% | **75%** |

**4. A defocus fallback for the ring.** At radius 9–10 the right version correlated at
0.84–0.93, below `MIN_AGREEMENT`, while leading the runner-up by 0.25 or more. Ink that is
not this ring reads near 0.5 on every version. `accept` therefore also takes agreement ≥ 0.75
with a lead ≥ 0.2. This is applied **only after no candidate in the frame passes the ordinary
rule**. Applied per candidate first, it let an earlier, worse quad pre-empt a later good one,
and cost `C2` a trial at radius 8. It decodes nothing by itself: those frames now fail at
luma rather than at detection. But `probe` can now say "found, hold steadier" instead of
"nothing here", and an accumulator can lock onto a frame it previously ignored.

**5. Which cells the kernel is fitted on, and a second rung.** Fitting the blur kernel on
payload modules alone — free, since they are already sampled — turned out better than
sampling every cell for `C1` and worse for `C3`. At blur radius 9, 48 trials: `C1` 88%
against 38%, `C3` 54% against 79%. Excluding only the white gap and quiet zone from the full
fit restored `C1` to 88%, so white targets were what biased it: after colour correction,
white is not exactly nominal white. `C3` gains from the extra known cells, because few of its
payload decisions are confident enough to join the fit at that blur. So the retry has two
rungs. The first uses payload colours (or an accumulator's mean). If the CRC still rejects a
single frame, a second samples the surround (`Observation::surround`) and fits on
everything. That gives both profiles their better figure.

**Tried and rejected, all measured:**

| idea | result |
|---|---|
| 5×5 kernel, for bleed two modules out | worse everywhere: `C3` blur 8 **88% → 6%**. Twenty-four neighbour estimates carry more decision error than the outer weights remove. |
| no reliability weight in the luma retry | `C3` blur 9 **88% → 0%** |
| accumulating observations weighted by per-frame reliability, against wandering glare | blur 7 + glare **100% → 0%**. Neighbouring cells end up averaged over different frames, so the residue is no longer spatially smooth. |
| sampling every cell for every accumulated frame | 34 ms more per v32 frame; kept only as the single-frame second rung (item 5) |

**Cost.** Scanner 215,875 → **222,107 bytes, 86% raw and 78% gzip** of D24's raised budget.
Frames that decoded before take exactly their old path, and the A/B bench and every non-blur
row of the breakdown are unchanged. `push` costs the same. A located frame that still fails
pays for up to five rounds per layer per rung: 11.2 → 17.4 ms for a v4 frame at blur 12. At
that blur v16 and v32 frames are never located, and cost the same as before.

### D26. `transfer.html` presents full screen, at whole device pixels only

**Status:** implemented and confirmed on a real capture (2026-10-07). `web/transfer.html`.

**How it was found.** A v40 `C3` file transfer (15,716 B original, 6,992 B compressed) at
ECC 1 and 2 decoded from `transfer.html` itself, but failed every time once the symbol was
moved to a page of its own. The decoder was not at fault. The page draws one canvas pixel
per *device* pixel, which is what §13.1.3's integer-multiple rule asks for; a separate page
or an image viewer scales the image to fit its window instead. At a non-integer ratio — 11.3
device pixels per module rather than 8 or 16 — every module is resampled into its
neighbours. A large symbol at the 8 px/module floor has no margin for that, and no amount of
camera distance recovers it.

**What it does.** A *Present full screen* button appears once a symbol is rendered:

- **The symbol on a white ground and nothing else**, full screen where the browser allows,
  with a screen wake lock so the display does not dim mid-scan. The lock is re-taken when the
  tab returns from the background, since browsers drop it then.
- **Enlarged by whole numbers only.** It picks the largest integer device pixels per canvas
  pixel that fits the screen, so 8 px/module becomes 16 or 24, never anything between. The
  tab title reports the result, for example `v40 — 16 device px/module`.
- **Placed at a whole device-pixel offset.** Ordinary centring can put the canvas on a half
  pixel, which resamples every module exactly as scaling does. The page computes the offset
  in device pixels.
- **Warns rather than shrinks.** If the symbol is larger than the screen even at 1×, a bar
  says so and suggests a smaller version or a bigger display. Shrinking to fit would break
  §13.1.3.
- **Esc, leaving full screen, or clicking the symbol** ends the presentation. The last is for
  a tablet or phone used as the display, which may have no Esc key.

**Result.** The same v40 transfer that failed from a separate page decodes from
presentation mode, and is easier to scan, being larger at a whole-number scale.

**Limits the page cannot see.**

- **macOS "Looks like …" scaled display modes** render at 2× and then resample to the panel,
  which undoes the whole-pixel guarantee below anything the browser can detect. Use the
  display's *Default* resolution for scanning.
- **Browser zoom** other than 100% changes `devicePixelRatio`. The page follows it, but a
  fractional ratio can still resample. Scan at 100%.
- `display.html` has had an equivalent presentation mode since the capture sweep;
  `transfer.html` now matches it. The two are separate implementations and could be shared.

### D27. Reviewer fixes: combined format copies, per-level chroma noise — and 27 KB back

**Status:** implemented. `crates/lc-decode/src/{demod,ring,lib,image,math}.rs`.
Decoder-only. Four fixes an outside reviewer proposed, each measured; two shipped, one was
rejected on measurement, one kept on probation. Then a size scan.

**1. Soft-combined format copies.** Both format readers decoded copy A and copy B separately
and kept the larger margin. They now also decode the copies' averaged LLRs
(`demod::format_candidates`) and keep the best of all three; averaged rather than summed,
so the margins share a scale. `--example format_probe`, v4, 32 trials, format read correctly:

| | σ 0.30 | 0.35 | 0.40 | 0.45 | 0.50 |
|---|---|---|---|---|---|
| `C1` | 100 → 100 | 97 → **100** | 97 → **100** | 94 → **97** | 94 → **100** |
| `C3` | 75 → **91** | 59 → **66** | 28 → **44** | 9 → **19** | 6 → **12** |

End to end, every format failure in `C3`'s noise rows disappeared.

**2. Chroma noise per luma level.** One pooled `a*b*` noise scale served both levels. In
linear light a dark colour has far less chroma signal than a light one at the same `C*`, so
the pooled scale overstated confidence in dark hues and understated it in light ones.
`ChannelStats::sigma_ab_level` measures each level separately. `sigma_ab_at` interpolates by
the sample's lightness, so a `C2` cell averaged over both levels gets a scale in between;
`hue_log_likelihoods` and `reliability` use it. **`C3` at noise σ 0.26 went from 19% to 100%**
(16 trials; 31% → 97% at 32), and 0.22 from 94% to 100%. At 48 trials the blur edge moved
too: `C1` blur 9 / 10 went 88 / 25% → **98 / 48%**, `C2` blur 9 98% → 100%, `C3` unchanged.

**3. Blur-aware mask selection — rejected.** An oracle showed the lever is real: per
payload, *some* luma mask decodes far more often under heavy blur (`C1` blur 10: the chosen
mask 54%, best of 8 per trial 96%). But nothing the encoder can compute finds that mask.
Pattern penalties (isolated modules, transitions, isolated hues) were no better than the
current QR-derived penalty on held-out trials. Even rehearsing each mask through a blur of
the same width but a different shape picked 47% against the oracle's 97%. Which mask
survives depends on the exact kernel, so an encoder cannot choose for an unknown lens.

**4. Illumination refit from decoded cells — on probation.** When luma converges and chroma
does not, the last rung of the retry refits §12.3 using every payload module as a reference:
level from the luma codeword, hue from the chroma posterior. New `stages` scenarios —
vignetting, colour cast with shading, two highlights — at v4 and v16 found illumination
limiting almost nowhere: 33 of 36 cells were already 100%, and the refit moved one trial.
It costs 2.5 KB and runs only after the CRC rejects. It stays because it targets
real-capture colour behaviour the harness cannot produce. It is the first thing to cut if
captures never show it helping.

**Size: 226,581 → 199,141 bytes (88% → 77% raw; 80% → 71% gzip).**

- **One `clamp` kept the float printer alive: −23.4 KB.** `ImageView::linear_bilinear` clamped
  with `self.width as f32 - 1.0`, a bound known only at run time. `f32::clamp` asserts
  `min <= max`, and that assertion's panic message Debug-formats both floats. That one call
  kept `<f32 as Debug>::fmt` and the Dragon4/Grisu printers — ~20 KB of code and ~3.5 KB of
  their tables — reachable, which is what D17 traced and D20 believed binaryen had removed.
  `max(0.0).min(w - 1.0)` is identical for any image `ImageView::new` accepts. **A `clamp`
  with a non-constant bound anywhere in the scanner costs this much**; constant bounds fold
  the assertion away.
- **Median by quickselect: −4.0 KB.** `math::median` sorted to find a middle value, which
  brought in the standard library's pattern-defeating quicksort and its small-sort helpers.
  Quickselect gives the identical median; `median_matches_a_sorted_reference` checks it
  against a full sort over 1,140 random arrays.

## Pinned — filling holes the draft left open

| # | Spec item | Pinned as | Where |
|---|---|---|---|
| P1 | §7 BCH(30,15) generator (open #6) | Shortened BCH(31,16), `g(x) = 0x8FAF`, `d = 7` verified exhaustively. Decoded by exhaustive maximum-likelihood over all 32768 codewords in a Gray-code walk — one XOR per step, so it is cheaper than Berlekamp–Massey *and* accepts soft input. | `lc-core/src/bch.rs` |
| P2 | §7 format mask | `(0x5412 << 15) \| 0x5412` — the spec's constant applied to both halves of the 30-bit word. | `lc-core/src/bch.rs` |
| P3 | §8.2 LDPC base graphs (open #4) | QC-LDPC, `dv = 3`, PEG rows over a fixed bidiagonal parity structure, shifts chosen greedily to minimise four-cycles across the lift sizes actually used. Three families: `n_b = 60`, `20`, `10`. | `xtask`, `lc-core/src/ecc/tables.rs` |
| P4 | §8.3 interleaver (open #7) | Golden-ratio affine permutation, `a = coprime(round(n/φ), n)`. Three-distance property means no burst length is pathological. | `lc-core/src/interleave.rs` |
| P5 | §6.2 palette anchors + §7 format region placement | Greedy claim over a fixed candidate order. The two features overlap if read literally — the ring outside a finder has 17 cells and they want 8 and 30. Ring walk goes outward from `d = 8`, alternating arms inward from the symbol edge; anchors claim first, then format copy A, then copy B. | `lc-core/src/layout.rs` |
| P6 | §5 partial chroma cells | A chroma cell carries a symbol only if **all** its modules are payload. Averaging `a*b*` over a cell where some modules are a fixed hue would bias the §12.4 LLR by a known-but-unmodelled amount, at the cells where geometry error is already worst. | `lc-core/src/layout.rs` |
| P7 | §6.2 interior alignment markers (open #9) | QR-style coordinate list: 6 to `side − 7`, evenly spaced, rounded to even, as few steps as keeps every spacing ≤ 28. Even coordinates mean a marker crossing row/column 6 agrees with the timing pattern module-for-module. | `lc-core/src/layout.rs` |
| P8 | §6.2 luma-only rendering | Fixed patterns render at **hue state H0**, not black/white. A four-colour palette has no neutral pair; using black and white would put fixed patterns outside the palette and give the decoder luma centroids from the finders that do not match the payload's. | `lc-core/src/palette.rs` |
| P9 | §8.5 chroma mask indexing | Rotation is indexed by **chroma-cell** coordinates, not module coordinates. Module coordinates give `(2cx + 2cy) mod 2 = 0` for every cell on `C2`/`C3` — the mask would do nothing at all on the profiles that most need it. | `lc-core/src/mask.rs` |
| P10 | §9.1 what the CRC covers | `header \|\| body`, zero-padded to the next byte boundary. The compact header is 15 bits so the frame is never byte-aligned; padding is constant per frame format and lets both ends use a stock byte-wise CRC. | `lc-core/src/frame.rs` |
| P11 | §12.4 hue labelling | Hue states numbered by increasing `a*b*` angle and **Gray-labelled**, so confusing two hues adjacent on the colour wheel costs one bit rather than two. This is what makes per-bit chroma LLRs well-founded. | `lc-core/src/symbol.rs` |
| P12 | §6.4 border cycle | Palette in index order, then white, then black. A cell's index is its coordinate *along* its edge; corners resolve to whichever axis they are further outside on. Both radial layers share a colour, giving each entry a 2-module-deep sampling target. | `lc-core/src/border.rs` |
| P13 | §6.4 outer quiet zone | Two modules of white outside the border. Not in the spec, but without it the border's dark cells run into whatever the symbol is displayed on and the finder scan has no clean termination. | `lc-render/src/lib.rs` |
| P14 | §8.6 bit-to-level convention | Luma bit maps straight to level: `0` = L0 (dark), `1` = L1 (light). With `index(l, h) = l·k + h`, a payload module's palette index is `bit · k + hue` with no lookup. | `lc-core/src/modulate.rs` |
| P15 | §9.1 spec's "40% vs 28%" | Overhead as a fraction of the *framed total*, not of the body. Both readings are defensible; this is the one that reproduces the spec's numbers. | `lc-core/src/frame.rs` |

---

## Still open

- **§9.3 `cflag 11`** (Brotli preset dictionary, open #11) — rejected as
  `UnsupportedCompression` rather than silently accepted. Needs a real corpus to build
  from; §9.3 is right that hand-writing it is the wrong move.

  **Measured 2026-09-05: brotli itself does not fit the scanner, which makes both brotli
  cflags unusable by a conforming minimal decoder.** Building the scanner component with
  `lc-decode/brotli` enabled takes it from **192,876 to 392,293 bytes** — the codec alone
  costs 199,417 bytes, or **97% of §14.2's entire 200 KB budget**, and lands the scanner at
  192% of it. So `cflag 10` already produces symbols the default scanner rejects as
  `UnsupportedCompression`, and `cflag 11` would inherit that plus a dictionary.

  That reorders the question. A preset dictionary is worth building when brotli is present;
  the finding is that brotli cannot be present in the build §14.2 describes. Before
  spending a corpus on `cflag 11`, decide what `cflag 10` is *for* — a full build that
  already carries the encoder can afford brotli, a scanner cannot, and a compression flag
  half the decoders reject is a poor use of two scarce wire-format bits.

  **zstd was considered and does not fit either**, though the tooling argument for it is
  real.
  Measured on representative payloads: a 62-byte JSON document goes to **68 bytes** under
  `zstd -19` — it *expands* — against 58 for raw DEFLATE; a 168-byte document goes to 133
  against DEFLATE's 125. The cause is frame overhead, which is **13 bytes** on empty input
  against raw DEFLATE's 2, and at §9.1 payload sizes a fixed 13-byte header is most of the
  win. Three further obstacles: `cflag` is two bits with all four values allocated, so
  zstd would displace something rather than join it; the scanner has ~11.6 KB of headroom
  against §14.2's budget and a pure-Rust zstd decoder is several times that; and §9.3's
  own point stands, which is that the win at these sizes comes from a *preset dictionary*,
  not from the algorithm. `zstd --train` is genuinely better tooling for building one than
  brotli offers — that is the one real argument, and it is not enough to overcome a
  13-byte header on a 60-byte payload.
- **§4.5 print palette** (open #3) — not started. `C2` currently uses the screen palette.
- **§8.2 ECC rate table** (open #5) — **closed, values unchanged.** See D10: the two-step
  luma/chroma offset is both necessary (narrowing it costs the blur envelope 81% → 3% at
  radius 4) and maximal (level 3 already sits on the last rate in the family). The
  reasoning §8.2 gives for it is wrong, and D10 records what should replace it. Standing
  caveat: measured on `C1` at one payload size, and `C2`/`C3` have different chroma-cell
  geometry, so re-run `just tune` before freezing those.
- **§6.1 quiet zone = 2** (open #10) — untested against textured backgrounds.
- **§13.1 device px/module minimum** (open #14) — 8 is now the working floor as the spec
  requires (D6); `RenderOptions` clamps to it and defaults to 10. Moiré onset still not
  measured, which is what would actually close #14.
- **§13.3 print module pitch** (open #15) — not measured.
- **§10.2 CWT claim labels** (open #12) and **§11 schema registry** (open #13) — untouched;
  both are signing/registry work, which §1.2 puts outside the common path.


---

## Unwinding the provisional decisions

D1, D2 and D3 are held deliberately cheap to revert. None of them is load-bearing for
anything structural: no other module hard-codes a palette value or assumes a particular
correction model.

### D1 / D2 — the palettes

Everything flows from **four constants per palette**, all in
`crates/lc-core/src/palette.rs`:

```rust
pub const C4_L0: f32 = 32.0;      pub const C8_L0: f32 = 30.0;
pub const C4_L1: f32 = 78.0;      pub const C8_L1: f32 = 74.0;
pub const C4_CHROMA: f32 = 55.0;  pub const C8_CHROMA: f32 = 40.0;
pub const C4_HUES: [f32; 2] = …;  pub const C8_HUES: [f32; 4] = …;
```

To change a palette: edit the constants, run the tests, and paste the new hex values the
`palettes_match_their_derivations` failure prints. That test exists precisely so the
pinned constants and the derivation cannot drift apart.

To choose the constants rather than guess them, run `just palette`. Picking `L*` and hues
by hand is what produced the gamut-clipped palettes described above: the sRGB boundary is
not intuitive at high `L*`, and a value that is comfortably reachable at one hue can be
several units out of reach 10° away. The search emits paste-ready constants, and
`no_shipped_colour_is_gamut_clipped` fails in release if a hand-edit gets it wrong.

To revert to §4.3 wholesale: point `Palette::for_profile` at `SPEC_4_3_COLORS`, which is
still in the tree, and delete the two tests that assert §4.2 compliance and hue stability.

Nothing downstream reads a colour literal. `Palette` computes its own level and hue
centroids, the renderer and decoder both take a `&Palette`, and `index(l, h) = l·k + h`
is the only ordering assumption anywhere — so palette size and colour values are free to
move without touching layout, modulation, ECC or detection.

**What is *not* cheap to unwind** is the iso-luminant-shared-hue property itself. §12.4's
chroma demodulation is built on one hue centroid per state, shared across luma levels. If
the review concludes the hue states really should differ by level, `demod.rs` needs a
second centroid set and a luma-conditioned chroma LLR — which also reintroduces the layer
coupling §4.1 warns about. That is a design change, not a constant change.

### D3 — the illumination model

Entirely contained in `crates/lc-decode/src/illum.rs`; `IlluminationModel::fit` and
`::correct` are the whole public surface, and everything else in the decoder takes the
fitted model as an opaque value. Reverting to a literal reading of §12.3 means rewriting
`fit_once` and `correct` and nothing else.

The tests that would then fail — `cancels_a_linear_shading_gradient` and
`cancels_shading_and_flare_together` — are the argument for the change, so they are worth
reading before reverting rather than deleting.
