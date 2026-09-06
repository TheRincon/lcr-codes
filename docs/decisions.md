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
