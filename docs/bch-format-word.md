# The format word, by hand

A complete worked example of the BCH(30,15) code that protects the format region (§7):
packing the fields, encoding by polynomial long division, masking, and four decodes —
clean, one error, three errors, and the two ways it fails.

Companion to `docs/error-correction.md`, which covers the LDPC codes that protect the
payload. This is the other half: the small algebraic code that protects the *instructions
for reading everything else*.

---

## 1. Why this field gets its own code

The format word says which version, profile, ECC levels and mask the rest of the symbol
uses. Nothing else can be read until it is known — including the LDPC layers, whose lengths
and rates it determines. It is therefore the one field where an undetected error is
catastrophic rather than merely expensive: get it wrong and you demodulate the entire
payload against the wrong parameters.

That rules out the payload's approach. LDPC needs thousands of bits before belief
propagation has anything to propagate; at 15 bits there is no graph worth walking. It also
rules out relying on a CRC, because there are no spare bits for one. What this field needs
is a short code with a **hard guarantee**, and that is exactly what the algebraic families
provide.

So: `crates/lc-core/src/bch.rs`, a shortened BCH(31,16) → **BCH(30,15)**, minimum distance
**7**, corrects any 3 errors and detects any 6.

```
15 data bits  →  30 codeword bits  →  XOR mask  →  two copies in the symbol
```

---

## 2. The code

The generator is the product of three minimal polynomials over GF(2⁵) with primitive
polynomial `x⁵ + x² + 1`:

```
g(x) = m₁(x)·m₃(x)·m₅(x)
     = x¹⁵ + x¹¹ + x¹⁰ + x⁹ + x⁸ + x⁷ + x⁵ + x³ + x² + x + 1
```

As bits, high term first — this is `GENERATOR = 0x8FAF`:

```
   x¹⁵ x¹⁴ x¹³ x¹² x¹¹ x¹⁰ x⁹  x⁸  x⁷  x⁶  x⁵  x⁴  x³  x²  x¹  x⁰
    1   0   0   0   1   1   1   1   1   0   1   0   1   1   1   1
   └──8──┘ └──F──┘ └──A──┘ └──F──┘
```

Three facts that make it the right code here, all checked by tests in that file:

- **It divides x³¹ + 1** (`generator_divides_x31_plus_1`), which is what makes the
  length-31 code cyclic and the polynomial arithmetic below valid.
- **Shortening preserves d = 7** (`minimum_distance_is_seven`, exhaustive over all 32,767
  non-zero messages). Shortening — dropping one information bit and fixing it to zero —
  can only ever *help* distance, but it is worth verifying rather than assuming.
- **d = 7 ⟹ correct ⌊(7−1)/2⌋ = 3.** The format region sits next to a finder, where glare
  damage is both likely and spatially clustered, so three is a meaningful budget.

Throughout: **bit 29 is the most significant, bit 0 the least**, and the region renders
MSB first.

---

## 3. Step one — pack the fields

Take a realistic symbol: **version 5, profile C3, luma ECC 1, chroma ECC 1, mask 3, with a
reference border.** The classic §7 layout (`FormatInfo::to_bits`) is:

| field | bits | width | value | binary |
|---|---|---:|---|---|
| version − 1 | 14–10 | 5 | 5 − 1 = 4 | `00100` |
| profile | 9–8 | 2 | C3 = `0b11` | `11` |
| luma ECC | 7–6 | 2 | 1 | `01` |
| chroma ECC | 5–4 | 2 | 1 | `01` |
| mask | 3–1 | 3 | 3 | `011` |
| reference border | 0 | 1 | yes | `1` |

Concatenate, MSB first:

```
00100 │ 11 │ 01 │ 01 │ 011 │ 1   =   001001101010111   =   0x1357
```

Version is stored as `version − 1` so that five bits cover versions 1–32 rather than 0–31.
That is the whole trick, and it is why the capacity table stops at 32.

## 4. Step two — encode

Systematic encoding: append 15 zeros (multiply by x¹⁵), divide by `g(x)`, and the remainder
is the parity. Division over GF(2) is just repeated XOR — no borrows, no carries. Walk down
from bit 29; wherever a bit is set, XOR in `g` aligned to it.

```
start                  001001101010111000000000000000

bit 27 set → XOR g<<12 001000111110101111000000000000
                     = 000001010100010111000000000000

bit 24 set → XOR g<<9  000001000111110101111000000000
                     = 000000010011100010111000000000

bit 22 set → XOR g<<7  000000010001111101011110000000
                     = 000000000010011111100110000000

bit 19 set → XOR g<<4  000000000010001111101011110000
                     = 000000000000010000001101110000

bit 16 set → XOR g<<1  000000000000010001111101011110
                     = 000000000000000001110000101110
```

Only five of the fifteen positions triggered; the rest had a zero at the leading bit and
were skipped. The remainder is everything left below x¹⁵:

```
parity = 001110000101110 = 0x1C2E
```

Concatenate data and parity — this is the unmasked codeword:

```
      001001101010111 │ 001110000101110      =  0x09AB9C2E
      └─ data 0x1357 ─┘ └─ parity 0x1C2E ─┘
```

Systematic, so `0x1357` is still sitting there in the top half, exactly as the LDPC
information bits sit in the payload.

## 5. Step three — mask

An all-zero format would render as a blank region, which a detector cannot distinguish from
a blank *anything*. So the codeword is XORed with a constant. §7 names QR's 15-bit
`0x5412`; our codeword is 30 bits, so it is applied to both halves:

```
MASK = (0x5412 << 15) | 0x5412 = 0x2A095412
```

```
  unmasked   0x09AB9C2E   001001101010111001110000101110
  MASK       0x2A095412   101010000010010101010000010010
  XOR        0x23A2C83C   100011101000101100100000111100   ← what renders
```

Two things worth noting. The mask is a constant, not a function of the data, so it is
*linear*: it cancels in any difference of codewords, which is why the minimum-distance test
can strip it and why the decoders fold it into the observation rather than trying codewords
both ways. And because it is applied after encoding, the masked word is **not** a codeword
of `g(x)` — a decoder must unmask before dividing. Forget that step and every syndrome comes
out garbage.

---

## 6. Step four — decode a clean word

The syndrome is the remainder of the received word modulo `g(x)`, after unmasking. A valid
codeword is by construction a multiple of `g(x)`, so:

```
syndrome(0x23A2C83C ^ MASK) = syndrome(0x09AB9C2E) = 0x0000   ✔
```

Zero syndrome, no errors, read the top 15 bits back out: `0x1357`, unpack the fields, done.

## 7. Step five — one error, located by hand

Flip bit 22 in the rendered word. Syndromes are linear — `syn(c ⊕ e) = syn(c) ⊕ syn(e)` and
`syn(c) = 0` — so **the syndrome depends only on the error, never on the message.** For a
single error at position *i* it is simply `x^i mod g(x)`, which gives a 30-entry lookup
table you can build once:

| error bit | syndrome |
|---:|---|
| 0 | `0x0001` |
| 1 | `0x0002` |
| 7 | `0x0080` |
| 14 | `0x4000` |
| 15 | `0x0FAF` |
| 22 | `0x04B5` |
| 29 | `0x643C` |

Bits 0–14 are trivial (`x^i` is already below degree 15, so it *is* its own remainder). Bit
15 is the first that needs reducing, and `x¹⁵ mod g = g − x¹⁵ = 0x0FAF`. All 30 entries are
distinct, which is the algebraic statement that every single error is uniquely locatable.

So:

```
received  0x23E2C83C
unmask  → 0x09EB9C2E
syndrome  0x04B5   →  table  →  bit 22  →  flip it back  →  0x09AB9C2E  ✔
```

That is genuinely hand-doable, and it is how the classical algebraic decoders work. For two
or three errors the same idea extends — syndromes still identify the error pattern — but the
table grows to 435 and then 4,060 entries, and the textbook route becomes Berlekamp–Massey
plus a Chien search.

## 8. Step six — what our decoder actually does instead

We do not do any of that. `bch::decode` scores **all 32,768 codewords** and returns the
nearest:

```rust
for i in 1u32..=0x7FFF {
    word ^= BASIS[i.trailing_zeros() as usize];   // one XOR per step
    let d = (word ^ target).count_ones();
    if d < best.1 { best = (gray(i as u16), d); }
}
```

The trick that makes brute force cheap: the code is linear, so every codeword is the XOR of
basis vectors chosen by the message's set bits, and walking the messages in **Gray code
order** — successive values differing in exactly one bit — means each step costs a single
XOR against a precomputed basis vector. 32,768 XORs and popcounts, a few hundred
microseconds, once per decode.

Two things this buys over Berlekamp–Massey, both of which matter more than the speed:

1. It is **maximum likelihood**, not bounded-distance. An algebraic decoder is defined only
   inside the radius-3 sphere; this one always returns the genuinely closest codeword, so it
   often recovers patterns outside that radius.
2. It accepts **soft input** directly (§10 below), which an algebraic decoder cannot.

Running our example through increasing damage:

| flipped bits | syndrome | result | distance | |
|---|---|---|---:|---|
| none | `0x0000` | `0x1357` | 0 | ✔ |
| 22 | `0x04B5` | `0x1357` | 1 | ✔ |
| 22, 5 | `0x0495` | `0x1357` | 2 | ✔ |
| 22, 5, 13 | `0x2495` | `0x1357` | 3 | ✔ guaranteed radius |
| 22, 5, 13, 28 | `0x168B` | *undecodable* | 4 | declines |

## 9. Step seven — the two ways it fails

**It gives up.** That last row is the honest failure. The nearest codeword was still the
right one, at distance 4 — but `decode` refuses anything past 3:

```rust
if best.1 > 3 { Err(LcError::FormatUndecodable) } else { Ok(best) }
```

That refusal is deliberate. Past the guaranteed radius a "correction" is more likely to
invent a plausible wrong version than to recover the right one, and for *this* field a
plausible wrong answer is far worse than no answer. No answer means try the other copy, or
the next camera frame.

**It is confidently wrong.** With d = 7, a 4-error pattern can land within 3 of a
*different* codeword, and then nothing inside the decoder can tell. Flip bits 0, 1, 2 and 5
of our example and the nearest codeword is `0x134F` at distance 3, while the truth sits at
distance 4. Unpack what that would mean:

| field | true `0x1357` | decoded `0x134F` | |
|---|---|---|---|
| version | 5 | 5 | — |
| profile | C3 | C3 | — |
| luma ECC | 1 | 1 | — |
| chroma ECC | 1 | **0** | ✗ |
| mask | 3 | **7** | ✗ |
| border | yes | yes | — |

The symbol would then be unmasked with the wrong pattern and its chroma layer sized at the
wrong LDPC rate. Every payload bit downstream is garbage — caught, eventually, by the §9.1
payload CRC, at the cost of a wasted frame. This is precisely why §7 stores **two copies**
of the codeword in physically separate places, so that clustered glare cannot take both.

## 10. Step eight — soft decoding, and the margin

The format region is sampled from the same noisy capture as everything else, and §12.4 has
already produced LLRs for it. Throwing that away to make hard 0/1 decisions before decoding
is the same mistake discussed in `docs/error-correction.md` §2 — so `decode_soft` doesn't.

It scores each codeword by correlation against the soft input:

```
corr(c) = Σ (1 − 2·cᵢ) · Lᵢ
```

Each bit contributes `+Lᵢ` if the codeword says 0 and `−Lᵢ` if it says 1, so a codeword that
agrees with confident observations scores high and disagreeing with an *unconfident* one
costs almost nothing. Same Gray-code walk, incremental update.

Take the four-error case that the hard decoder declined — but now with the realistic detail
that the bits which flipped are the ones the camera was least sure of (magnitude 0.3 against
4.0 elsewhere), because a sample near the decision boundary is what flips in the first
place:

```
flips at bits 22, 5, 13, 28, all with |L| = 0.3

soft decode → 0x1357  ✔   margin 29.60
```

Recovered, where the hard decoder gave up. The four wrong bits cost the true codeword only
4 × 2 × 0.3 = 2.4 of correlation, while any rival codeword has to fight the 26 confident
bits at 4.0 each.

The second return value is the **margin** — the correlation gap between the best and
second-best codeword. It is what §12.2 uses to choose between the two format copies, and
the reasoning in the source is worth repeating:

> Comparing margins beats comparing Hamming distances, because two copies that both "pass"
> at distance ≤ 3 can still disagree, and the one whose runner-up is further away is the one
> to believe.

It surfaces as `Diagnostics::format_margin`. A low margin on a successful decode is the
early warning that the format region is marginal even though this frame happened to work.

---

## 11. The ring variant (D21)

Finder-less symbols (§6.4) have no format region — the ring carries the field instead, in
its hue choices. The code is identical; only the field layout changes, because the ring
sequence already encodes the version and the border flag is implicit:

| field | bits | width |
|---|---|---:|
| profile | 14–11 | 4 |
| luma ECC | 10–9 | 2 |
| chroma ECC | 8–7 | 2 |
| mask | 6–4 | 3 |
| reserved (must be 0) | 3–0 | 4 |

Same `g(x)`, same mask, same decoders — `to_ring_codeword` just calls `bch::encode` on a
differently packed 15 bits. The four reserved bits are checked to be zero on decode
(`from_ring_bits` returns `FormatUndecodable` otherwise), which is a free extra consistency
test: a wrong-but-valid codeword has a 15/16 chance of being caught by it.

Worth knowing when reading `vectors/golden-v1.json`: a vector with a `ring` field carries
the ring word, everything else the classic word. Unpacking one with the other layout
produces nonsense that *looks* plausible — `profile-c1-border` reads as "version 3, C2" if
you use the classic layout and correctly as "C1" if you use the ring layout.

---

## Appendix — reproducing everything above

Verified against the shipped implementation: this reproduces all 34 format codewords in
`vectors/golden-v1.json` exactly.

```python
GEN  = 0x8FAF
MASK = (0x5412 << 15) | 0x5412

def encode_raw(data):                       # systematic, unmasked
    data &= 0x7FFF
    rem = data << 15                        # multiply by x^15
    for s in range(14, -1, -1):             # long division by g(x)
        if rem & (1 << (s + 15)):
            rem ^= GEN << s
    return (data << 15) | rem

def encode(data):
    return encode_raw(data) ^ MASK

def syndrome(word):                         # word must already be unmasked
    r = word
    for s in range(29, 14, -1):
        if r & (1 << s):
            r ^= GEN << (s - 15)
    return r & 0x7FFF

def decode(received):                       # exhaustive maximum likelihood
    target = (received & 0x3FFFFFFF) ^ MASK
    best = (None, 99)
    for m in range(0x8000):
        d = bin(encode_raw(m) ^ target).count('1')
        if d < best[1]:
            best = (m, d)
    return best if best[1] <= 3 else ('UNDECODABLE', best[1])

def decode_soft(llrs):                      # llrs[0] is bit 29, MSB first
    l = [(-v if (MASK >> (29 - i)) & 1 else v) for i, v in enumerate(llrs)]
    best, second = (None, -1e18), -1e18
    for m in range(0x8000):
        cw = encode_raw(m)
        corr = sum((-l[29 - b] if (cw >> b) & 1 else l[29 - b]) for b in range(30))
        if corr > best[1]:
            second, best = best[1], (m, corr)
        elif corr > second:
            second = corr
    return best[0], best[1] - second        # (message, margin)

# §3–§5
w = (4 << 10) | (3 << 8) | (1 << 6) | (1 << 4) | (3 << 1) | 1
assert w == 0x1357
assert encode_raw(w) == 0x09AB9C2E and encode(w) == 0x23A2C83C

# §7–§9
c = encode(w)
assert syndrome(c ^ MASK) == 0x0000
assert syndrome((c ^ (1 << 22)) ^ MASK) == 0x04B5
assert decode(c ^ (1 << 22) ^ (1 << 5) ^ (1 << 13)) == (0x1357, 3)
assert decode(c ^ (1 << 22) ^ (1 << 5) ^ (1 << 13) ^ (1 << 28))[0] == 'UNDECODABLE'
assert decode(c ^ 0b100111)[0] == 0x134F          # confidently wrong

# §10
flips = {22, 5, 13, 28}
llrs = []
for i in range(30):
    pos = 29 - i
    mag = 0.3 if pos in flips else 4.0
    v = mag if (c >> pos) & 1 == 0 else -mag
    llrs.append(-v if pos in flips else v)        # weak samples fell the wrong way
assert decode_soft(llrs)[0] == 0x1357
```
