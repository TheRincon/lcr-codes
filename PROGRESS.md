# LC Code — implementation progress

Reference implementation of [`lc-code-spec-v0.4.md`](lc-code-spec-v0.4.md).

**State:** encode and decode are both complete and round-trip end to end through a
rendered image — detection, illumination fit, soft demodulation, LDPC, framing, CRC. The
§15 degradation harness is built and the operating envelope is measured. `lc-wasm` ships
both paths as a **WebAssembly component** with a WIT interface (D8), in two builds:
decode-only and full.

**Provisional, pending review:** the palette (D1/D2) and illumination-model (D3) changes in
[`docs/decisions.md`](docs/decisions.md) are being carried forward so work can continue,
not signed off. Both are cheap to unwind — see "Unwinding the provisional decisions" below.

**262 tests, all passing, zero warnings.** `just test`

---

## Picking this up again

*Written 2026-09-01, before a restart. Delete this section once it is stale.*

### Nothing is half-finished

This is a clean stopping point. The last piece of work — the C host trial and D9 — is
complete, tested and documented. No edit is partially applied, no test is skipped, and no
recipe is left broken. Nothing needs unwinding before continuing.

### ⚠️ Nothing is committed to git

`git init` has been run and `just hooks` has configured `core.hooksPath`, but **zero files
are tracked** and there are no commits. All work exists only as files on disk. Committing
is the first thing to do:

```bash
git add -A && git status        # expect ~13 paths, no target/ or dist/
git commit -m "LC Code: encode/decode, degradation harness, wasm component"
```

`.gitignore` already excludes `/target`, `/dist` and `*.wasm`, so build output stays out.
Note the pre-push hook runs `just ci` — first push will take a few minutes.

### Verifying the world after a restart

```bash
just component     # rebuild dist/ — it is gitignored, so it will be missing
just test          # 262 tests; depends on `component`, so the line above is optional
just c-host        # the C trial, needs the Go module cache (see below)
```

Expect **262 passing, 0 warnings**. If `just test` cannot find `dist/lc-code.wasm`, the
component build did not run — `just test` depends on it, so that would mean the justfile
was edited.

### Things a restart may take with it

- **`dist/`** — build output, not tracked. `just component` regenerates it in ~30s.
- **`~/go/pkg/mod/.../wasmtime-go/v34@v34.0.0`** — `just c-host` borrows the
  component-capable `libwasmtime.a` bundled there (D9). If it is gone:
  `go get github.com/bytecodealliance/wasmtime-go/v34`, or point `WASMTIME_C_API` at an
  unpacked wasmtime-c-api release.
- **Scratchpad backups** in `/private/tmp/...` — `image.rs.orig`, `border.orig`,
  `detect.rs.fixed`, `gotest/`. All disposable. They were before/after copies used while
  fixing D6 and measuring the border width; every one of those changes is landed and
  covered by tests.

### What I was about to pick up

Nothing was in progress. The next item is **§8.2 ECC rate tuning** — see "What is left"
below. Briefly, so it can be started cold:

- The rate table in §8.2 is still the spec's own guesses, labelled `[TBD]` there.
- The §15 harness that replaces them now exists and reports success rate against
  degradation magnitude, which is exactly the input §14.5 step 9 calls for.
- It is **the last thing blocking a v1.0 wire-format freeze**, and it changes
  `crates/lc-core/src/ecc/`, so it wants doing before anything depends on those rates.
- Start by reading §8.2 and `crates/lc-decode/tests/degradation.rs`, then sweep rates
  against the harness rather than changing the table by hand.

Second, smaller: `just ci` runs locally via the pre-push hook but nothing runs it on a
fork. The scanner sits at **93% of its size budget** measured raw (D8), so that gate wants
to be automatic rather than trusted to a hook someone can `--no-verify` past.

---

## Where things stand

| Crate | State | Tests | Notes |
|---|---|---|---|
| `lc-core` | ✅ Complete | 138 | Geometry, palette, ECC, container, masking, modulation |
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
large end (D5); §5's bits/module figures are unreachable for `C2`/`C3` (D4); §13.1's ≥ 8
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

### 2. Tune the ECC rates

§8.2's rates are still the spec's guesses, and the harness that replaces them now exists —
§14.5 step 9. This is the last thing blocking a v1.0 wire-format freeze.

### 3. Signing (§10) — optional, lower priority

`lc-verify`: COSE_Sign1, CWT claims, ES256 + Ed25519, `KeyResolver`. §1.2 is explicit that
this is a feature and not the point, so it sits behind the decode path. `lc-encode/sign.rs`
is a placeholder.

Worth doing carefully when we get there: test against externally generated vectors, not
just our own encoder — self-consistent crypto bugs are the dangerous kind.

### 4. Everything else

- [ ] `lc-cli` — encode/decode/inspect dev tool.
- [ ] `lc-ffi` — C ABI. Note edition 2024 needs `#[unsafe(no_mangle)]`.
- [ ] `fuzz/` — targets for `decode_rgb` and the CBOR parser, both of which take untrusted
      input.
- [ ] Golden vector file, frozen at v1.0, for cross-implementation checks.
- [ ] Print palette (§4.5) — `C2` currently borrows the screen palette.
- [ ] Brotli preset dictionary (§9.3 `cflag 11`) — currently rejected rather than silently
      mishandled. Needs a real corpus.
- [ ] Remaining §15 screen impairments: PWM banding, subpixel geometry, a moiré sweep.

---

## Fixed

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

- **Edition 2024** throughout, `rust-version = 1.85`.
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
