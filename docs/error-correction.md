# How LC Codes recover the message

Why there is no Reed–Solomon anywhere in the payload path, what the "information" and
"parity" cells in `examples/anatomy-expanded-c3.png` actually are, and a complete
by-hand decode of an eight-bit symbol.

Companion to `docs/ldpc.md` (which is the generated threshold table, not an explanation)
and to §8 / §9.1 / §12.5 of the spec.

---

## 1. Information vs. parity

Our LDPC codes are **systematic**. `Code::encode`
(`crates/lc-core/src/ecc/ldpc.rs:216`) copies your bits out verbatim and then appends
parity computed from them:

```rust
let mut full = vec![false; kb * z];
full[..self.k].copy_from_slice(info);     // your bytes, untouched
// ... staircase solve produces p ...
```

So in the expanded anatomy figure:

- **Information cells** literally carry your frame — container header, body, CRC. Read a
  perfect symbol and you could skip decoding entirely and just concatenate them.
- **Parity cells** carry nothing of yours. They are constraints that let the decoder work
  out which of the information cells the camera got wrong.

For the v5 C3 in that figure:

| layer | information | parity | rate |
|---|---:|---:|---:|
| brightness | 1,077 | 375 | 0.74 |
| colour | 1,581 | 1,323 | 0.54 |

Colour is always two ECC steps more protected than brightness — `LUMA_RATE_INDEX =
[0,1,2,3]` against `CHROMA_RATE_INDEX = [2,3,4,5]` over `RATES = [0.85, 0.75, 0.65, 0.55,
0.45, 0.35]` — because hue survives a phone camera far worse than light-versus-dark does.
The effective rates land slightly under nominal (0.74, not 0.75) because the code is
shortened to fit the exact symbol capacity.

Information and parity are *scattered*, not blocked, because §8.3 interleaves everything
before it reaches the grid. A thumb smudge that destroys a contiguous patch of modules
becomes a thin sprinkle of damage across many parity checks, which is the situation the
decoder handles best.

---

## 2. Why LDPC and not Reed–Solomon

Reed–Solomon and BCH are **algebraic**. The codeword is a polynomial with known roots;
errors perturb it; you solve a system of equations — syndromes, then an error-locator
polynomial, then a Chien search — for exactly where the errors are. Deterministic, and it
gives a hard guarantee: *t* symbol errors corrected, always, and one more fails loudly.

LDPC throws the algebra away. A codeword is just a bit vector satisfying a large set of
parity checks, each touching only a handful of bits — that sparsity is the "low density".
There is no formula for the errors. There is a graph, and you negotiate.

Two reasons that fits this channel better:

**Soft information is free here, and RS cannot use it.** A camera hands us continuous
values, not bits. `ChannelStats::luma_llr` (`crates/lc-decode/src/demod.rs:135`) never
decides "this module is dark" — it measures L\*, estimates noise from known-colour
reference modules, and emits a log-likelihood ratio: *probably 1, and I am this sure*.
Classic RS decoding begins by discarding that. It costs roughly 2 dB (§12.4), which in
practice is the difference between decoding at arm's length in a dim room and not.

**Our errors are not byte-shaped.** RS's advantage is bursts: one corrupted byte costs one
symbol whether one bit flipped or eight. That suits scratched discs and QR's contiguous
codeword blocks. Our damage is glare, defocus and tilt, and after interleaving it lands as
scattered low-confidence bits. RS would spend its correction budget on singletons.

There is also a structural reason. RS works over GF(2⁸) on bytes, but we have two
independent channels with different reliabilities wanting different rates. Two separate
LDPC codes over the same grid is natural; splitting a byte-oriented RS code across them is
not.

**We do use an algebraic code, though — just not for your payload.** The format word is
BCH(30,15) masked with `0x5412` (`crates/lc-core/src/format.rs`), and Reed–Solomon is a
subclass of BCH, so it is the same family. That is the right tool there: 15 bits is far too
short for belief propagation to have anything to propagate, and we need a hard guarantee on
the field that tells us how to decode everything else. The D21 ring word gets the same
treatment. `docs/bch-format-word.md` works that code through by hand, the same way §3 below
does for LDPC.

---

## 3. A complete decode by hand

The shipped codes are protographs lifted by `Z` and then shortened — `n = 1452` for the
brightness layer of that v5 C3. Nobody decodes that by hand. So here is a toy with the same
*shape*: systematic, staircase parity, column weight 3, decoded with the identical
normalised min-sum lc-core runs.

### 3.1 The code

Eight bits: four information (`v0..v3`), four parity (`v4..v7`). Four parity checks, each
of which must XOR to zero:

```
c0:  v0 ⊕ v1 ⊕ v3 ⊕ v4                 = 0
c1:  v0 ⊕ v1 ⊕ v2 ⊕ v4 ⊕ v5            = 0
c2:       v1 ⊕ v2 ⊕ v3 ⊕ v5 ⊕ v6       = 0
c3:  v0      ⊕ v2 ⊕ v3      ⊕ v6 ⊕ v7  = 0
```

As a Tanner graph — the picture the decoder actually walks:

```
      v0    v1    v2    v3    v4    v5    v6    v7
       │╲    │╲    │╲    │╲    │╲    │╲    │╲    │
       │ ╲   │ ╲   │ ╲   │ ╲   │ ╲   │ ╲   │ ╲   │
      ┌c0┐ ┌c1┐ ┌c2┐ ┌c3┐        (each v touches 2–3 checks,
      └──┘ └──┘ └──┘ └──┘         each c touches 4–5 variables)

  c0 ── v0 v1 v3 v4
  c1 ── v0 v1 v2 v4 v5
  c2 ── v1 v2 v3 v5 v6
  c3 ── v0 v2 v3 v6 v7
```

Every information bit sits in exactly three checks. That is the `dv = 3` the `ALPHA = 0.75`
comment in `ldpc.rs:61` refers to. The parity part is a staircase — `v4` in two checks,
`v5` in two, `v6` in two, `v7` in one — which is what makes encoding a forward substitution
rather than a matrix inversion, exactly as in the real encoder.

### 3.2 Encoding, by hand

Message: **1 0 0 1**.

Read each check as a definition of its newest parity bit and walk down the staircase:

```
p0 = v0 ⊕ v1 ⊕ v3           = 1 ⊕ 0 ⊕ 1        = 0
p1 = v0 ⊕ v1 ⊕ v2 ⊕ p0      = 1 ⊕ 0 ⊕ 0 ⊕ 0    = 1
p2 = v1 ⊕ v2 ⊕ v3 ⊕ p1      = 0 ⊕ 0 ⊕ 1 ⊕ 1    = 0
p3 = v0 ⊕ v2 ⊕ v3 ⊕ p2      = 1 ⊕ 0 ⊕ 1 ⊕ 0    = 0
```

Codeword: **1 0 0 1 | 0 1 0 0**. Information first, parity appended — systematic, and you
can still see `1001` in the front half.

### 3.3 What the camera saw

Positive LLR means bit 0, which is the dark palette level — the sign convention asserted by
`luma_llr_sign_follows_the_bit_convention` in `demod.rs`. With `l0 = 30`, `l1 = 80`,
`σ = 12`, `luma_llr` gives:

| module | sent | measured L\* | LLR | reads as | |
|---|:--:|---:|---:|:--:|---|
| v0 | 1 | 78.0 | −8.0 | 1 | clean |
| v1 | 0 | 52.1 | **+1.0** | 0 | defocused — barely a decision |
| v2 | 0 | 72.3 | **−6.0** | **1** | **glare: confidently wrong** |
| v3 | 1 | 78.0 | −8.0 | 1 | clean |
| v4 | 0 | 32.0 | +8.0 | 0 | clean |
| v5 | 1 | 78.0 | −8.0 | 1 | clean |
| v6 | 0 | 32.0 | +8.0 | 0 | clean |
| v7 | 0 | 32.0 | +8.0 | 0 | clean |

Slice at zero and you get `1 0 1 1 | 0 1 0 0` — one bit wrong, in the information half, and
the channel is *confident* about it. A hard-decision decoder starts from here having
already lost the byte.

### 3.4 Iteration 1, longhand

Each check tells each of its bits what the *others* imply it should be. Normalised min-sum:
the sign is the product of the other messages' signs, the magnitude is the smallest of
their magnitudes, scaled by `ALPHA = 0.75`. On the first pass the incoming messages are
just the channel LLRs above.

**What `c3` tells `v2`.** `c3` touches `v0, v2, v3, v6, v7`; excluding `v2` itself:

```
others:      v0 = −8.0    v3 = −8.0    v6 = +8.0    v7 = +8.0
sign:        two negatives → even → positive
magnitude:   min(8.0, 8.0, 8.0, 8.0) = 8.0
message:     0.75 × (+1) × 8.0 = +6.0
```

`c3` has four confident, mutually consistent neighbours, and they leave exactly one value
for `v2` that satisfies the check: **0**, asserted at strength +6.0 — against the camera's
−6.0. The graph and the pixel disagree, and the graph is about to win.

**What `c1` tells `v2`.** `c1` touches `v0, v1, v2, v4, v5`; excluding `v2`:

```
others:      v0 = −8.0    v1 = +1.0    v4 = +8.0    v5 = −8.0
sign:        two negatives → even → positive
magnitude:   min(8.0, 1.0, 8.0, 8.0) = 1.0
message:     0.75 × (+1) × 1.0 = +0.75
```

Same conclusion, a twelfth of the force. This is min-sum's defining behaviour: **a check is
only as strong as its weakest member.** One defocused module (`v1`) throttles everything
`c1` has to say.

**Totals.** Each bit adds its channel value to every message it received:

```
v2:  −6.00  +0.75 (c1)  +0.75 (c2)  +6.00 (c3)  =  +1.50  →  bit 0   ✔ corrected
v1:  +1.00  +6.00 (c0)  −4.50 (c1)  −4.50 (c2)  =  −2.00  →  bit 1   ✘ newly wrong
```

| | v0 | v1 | v2 | v3 | v4 | v5 | v6 | v7 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| total | −3.50 | −2.00 | +1.50 | −3.50 | +8.00 | −6.50 | +2.75 | +3.50 |
| hard | 1 | **1** | 0 | 1 | 0 | 1 | 0 | 0 |

Syndrome `[1, 1, 1, 0]` — three checks still unsatisfied, so we continue. Note what
happened: the glare bit got fixed, but the poisoned `v2` had already fed `c1` and `c2`
during the same pass and dragged the weak `v1` across zero. Error briefly *spreads*. This
is normal, and it is why the decoder iterates instead of deciding.

### 3.5 Iteration 2

Now `v2` sends out `+1.5` instead of `−6.0`, so `c1` and `c2` reverse what they say about
`v1`, and `c0` — which never touched the damaged bit — keeps pushing `v1` positive:

```
v1:  +1.00  +2.06 (c0)  +0.56 (c1)  +0.56 (c2)  =  +4.19  →  bit 0   ✔ healed
```

| | v0 | v1 | v2 | v3 | v4 | v5 | v6 | v7 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| total | −3.12 | +4.19 | +3.19 | −3.12 | +6.50 | −9.12 | +5.19 | +4.62 |
| hard | 1 | 0 | 0 | 1 | 0 | 1 | 0 | 0 |

Syndrome `[0, 0, 0, 0]`. Every check satisfied, so `syndrome_ok` returns true, `converged`
is set and the loop breaks (`ldpc.rs:376`) — two iterations of the fifty it was allowed.

Take the first `k = 4` bits: **1 0 0 1**. The message, recovered from a read where one
information bit arrived confidently inverted and another was barely legible. Notice that
`v2` ended at `+3.19`: the decoder is now *more* sure of that bit than the camera ever was
of `v1`, because five other modules vouch for it.

### 3.6 Where it goes wrong — and why the CRC is the real gate

Same code, same message, but now glare hits **two** modules, `v2` and `v5`:

```
rx:       −8.0  +8.0  −6.0  −8.0  +8.0  +6.0  +8.0  +8.0
hard-in:    1     0     1     1     0     0     0     0
```

The decoder converges — in two iterations, all four checks satisfied, `converged = true` —
on:

```
1 0 1 1 | 0 0 0 1
```

That is a **perfectly valid codeword**. It is the encoding of `1011`, and you can verify it
against §3.2's staircase by hand. It is simply not the one that was sent; it sits at
Hamming distance 3, which is this code's minimum distance. Two confident lies were enough
to push the negotiation into the next valley, and nothing inside the LDPC decoder can tell.
It reports success.

This is the error floor, and it is why the spec says, flatly:

> LDPC convergence alone MUST NOT be treated as decode success.

`Decoder::finish` (`crates/lc-decode/src/lib.rs:365`) therefore ignores `converged` as a
verdict — it is carried in `Diagnostics` and nowhere else. It concatenates luma then chroma
and lets the §9.1 CRC decide: CRC-16 for compact frames (~1/65,536 undetected), CRC-32 for
extended (~1/4×10⁹). Here the recovered message `1011` would fail its CRC, the frame is
rejected, and the scanner simply asks for another look at the symbol — which, one frame
later, has different glare.

Two separate accept gates, and they answer different questions:

| gate | question | authority |
|---|---|---|
| LDPC `converged` | is this *a* codeword? | diagnostic only |
| CRC (§9.1) | is this *your* codeword? | authoritative for **decoding** |
| signature (§10) | is this *their* codeword? | authoritative for **trusting** |

---

## 4. Scaling the toy up

Everything above is what really happens; only the sizes change.

| | toy | v5 C3 brightness layer |
|---|---:|---:|
| information bits `k` | 4 | 1,077 |
| parity bits | 4 | 375 |
| codeword `n` | 8 | 1,452 |
| checks | 4 | 375 |
| column weight | 3 | 3 |
| iterations used | 2 | up to `MAX_ITERATIONS = 50` |
| minimum distance | 3 | not computed — the graph is designed, not solved |

Four differences worth knowing:

1. **The real graph is lifted, not written out.** A small base graph is expanded by a
   circulant shift `Z`, which is why `encode` works in `z`-sized chunks with
   `chunk[(t + shift) % z]`. It keeps the graph sparse and the description tiny.
2. **Shortening is free information.** Bits between `self.k` and `kb * z` are known zeros,
   and `channel_llrs` feeds them in at `CERTAIN = 1e6` (`ldpc.rs:64`). They act as the
   perfectly confident neighbours that `c3` happened to have in §3.4.
3. **Real LLRs are clamped** to ±20 (`LLR_CLAMP`, `demod.rs:49`) and weighted down when a
   sample sits far from every palette colour, because confident-and-wrong is the one thing
   this decoder cannot absorb — as §3.6 demonstrates.
4. **Longer codes are better codes.** Compare the waterfalls in `docs/ldpc.md`: at rate
   0.65 the `n = 162` layer needs 3.94 dB for 5% FER and the `n = 19406` layer needs
   3.47 dB. A bigger graph gives belief more room to propagate before it comes back around
   on itself.

### The whole path

```
bytes
  → frame + CRC (§9.1)
  → split across luma / chroma
  → LDPC encode, per layer, at its own rate   (information ++ parity)
  → interleave (§8.3)
  → mask
  → modules

modules
  → sample
  → LLRs, scaled by measured noise            (soft, never thresholded)
  → deinterleave
  → normalised min-sum, ≤50 iterations
  → information bits
  → concatenate luma-first (§12.5)
  → CRC ✓
```

Recovery is the camera saying *I am 60% sure that one is dark*, a few thousand XOR
constraints arguing about it for up to fifty rounds, and a CRC at the end refusing to sign
off unless the bytes are genuinely right.

---

## Appendix — reproducing §3

The toy decoder, with lc-core's schedule and constants. Every number above came out of it.

```python
ALPHA = 0.75
CHECKS = [[0,1,3,4], [0,1,2,4,5], [1,2,3,5,6], [0,2,3,6,7]]
N = 8

def encode(i0, i1, i2, i3):
    p0 = i0 ^ i1 ^ i3
    p1 = i0 ^ i1 ^ i2 ^ p0
    p2 = i1 ^ i2 ^ i3 ^ p1
    p3 = i0 ^ i2 ^ i3 ^ p2
    return [i0, i1, i2, i3, p0, p1, p2, p3]

def decode(rx, max_iter=50):
    v2c = {(c, v): rx[v] for c, vs in enumerate(CHECKS) for v in vs}
    c2v = {(c, v): 0.0   for c, vs in enumerate(CHECKS) for v in vs}
    total = list(rx)
    for it in range(1, max_iter + 1):
        for c, vs in enumerate(CHECKS):                 # check nodes
            for v in vs:
                other = [v2c[(c, u)] for u in vs if u != v]
                sign = -1.0 if sum(m < 0 for m in other) % 2 else 1.0
                c2v[(c, v)] = ALPHA * sign * min(abs(m) for m in other)
        for v in range(N):                              # variable nodes
            inc = [c for c, vs in enumerate(CHECKS) if v in vs]
            total[v] = rx[v] + sum(c2v[(c, v)] for c in inc)
            for c in inc:
                v2c[(c, v)] = total[v] - c2v[(c, v)]
        syn = [sum(total[v] < 0 for v in vs) % 2 for vs in CHECKS]
        if not any(syn):
            return [int(t < 0) for t in total], it, True
    return [int(t < 0) for t in total], max_iter, False

print(decode([-8.0, 1.0, -6.0, -8.0, 8.0, -8.0, 8.0, 8.0]))   # §3.4–3.5, recovers 1001
print(decode([-8.0, 8.0, -6.0, -8.0, 8.0,  6.0, 8.0, 8.0]))   # §3.6, converges to 1011
```
