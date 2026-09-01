# LC Code — Specification Draft v0.4

**L**uma / **C**hroma code

A colour 2D matrix symbology for **portable offline data transmission** — structured,
JSON-shaped payloads at higher density than a monochrome matrix code, displayed primarily on
phone screens and secondarily in print. Signed, offline-verifiable packets are an optional
feature layered on top.

**Status:** design draft for implementation. Sections marked **[NORMATIVE]** define wire
format or conformance and must be frozen before v1.0. Sections marked **[INFORMATIVE]** are
rationale or guidance. Items marked **[TBD]** need a decision or calibration data.

**Changes from v0.3:** Signing demoted from primary design
driver to an optional feature (§1, §10); the goals and version guidance no longer assume a
signature is present. Compact frame added for small payloads (§9.1). Full version range
restored for unsigned use, with the signed-packet floor scoped to signed packets only
(§6.1). Verification removed from the default decode path (§14.4).

**Changes from v0.2:** trust-list format, distribution, and rotation removed from scope —
key resolution is a deployment concern (§10.5). Revocation removed entirely; `exp` is the
sole freshness control (§10.6). `cti` claim removed. `TrustList` replaced by a `KeyResolver`
trait at the API boundary (§14.3–14.4).

**Changes from v0.1:** screen-first channel model (§3, §13); CWT/COSE payload container
replacing raw JSON (§9); schema registry generalised (§11); WASM as a first-class target
(§14); v0.1's separate addendum merged throughout.

---

## 1. Goals and non-goals

### 1.1 Goals

1. **Portable offline data transmission.** Carry a structured payload — the JSON data model,
   in practice — in a symbol that reads reliably from a phone screen at arm's length, with
   no network involved at any point.
2. **Density.** Exceed monochrome matrix density by 1.5×–2× per unit area, so payloads that
   would need an unreasonably large QR code fit in a scannable symbol.
3. **Degrade gracefully.** Bad light, a monochrome sensor, or an unrecoverable chroma layer
   should yield a partial result rather than a hard failure.
4. **Survive screens.** Be robust to the impairments that actually break colour codes on
   glass: specular glare, panel/rolling-shutter interaction, moiré, and display colour modes.
5. **Be cheap to embed.** Implementable in safe Rust, decodable in a small WASM bundle, fast
   enough for real-time camera preview decoding on a mid-range phone.
6. **Optional signing.** Support offline-verifiable signed packets for deployments that need
   attestation, as an opt-in layer.

### 1.2 Signing is a feature, not the point **[INFORMATIVE]**

Most LC Code payloads are expected to be **unsigned structured data** — a config blob, a
contact record, a device pairing token, a location fix, a small JSON document someone wants
to hand to another device without a network in between.

**Nothing in the format should make an unsigned payload pay for a capability it does not
use.** A 10-byte payload must fit in the smallest symbol the geometry allows, with framing
overhead proportionate to its size (§9.1). The ~110-byte cost of a COSE signature (§10.4)
applies only to packets that carry one, and the version guidance in §6.1 is scoped
accordingly.

Where signing *is* used, it must work with no network at verification time — see §10.

### 1.3 Non-goals

- Beating JAB Code on raw density. JAB targets ~3 bits/module; LC Code targets 1.5–2.0 with
  much wider operating margins. This is a deliberate trade.
- Compatibility with QR, Data Matrix, Aztec, or JAB decoders.
- Confidentiality. See §10.8 — LC Code is a public channel by construction.
- Payloads above a few kilobytes.

### 1.4 Design thesis **[INFORMATIVE]**

Three observations drive the design.

**Luma and chroma are different channels with different noise and different bandwidth.**
Luminance is high-bandwidth but corrupted by illumination effects. Chroma is comparatively
stable once white balance is solved, but is *low-bandwidth*: a Bayer sensor allocates half
its photosites to green and a quarter each to red and blue, demosaicing interpolates the
rest, and a 4:2:0 JPEG stage halves chroma resolution again in both axes. Treating a colour
module as one atomic symbol ignores this and wastes margin on both sides. LC Code splits the
payload into a **luma layer** at full module pitch and a **chroma layer** at equal or coarser
pitch, with independent code rates. The luma layer alone is a complete monochrome code.

**Colour errors are correlated across space, not independent.** A shadow, a glare
highlight, or a colour-mode shift moves a whole region coherently. So a small number of
scattered known-colour reference modules can fit a smooth correction surface and cancel most
of the error, rather than needing per-module redundancy. LC Code spends ~2% of area on
references and recovers far more than that.

**On a screen, the displaying application controls most of the impairments.** Unlike print,
where the channel is fixed at manufacture, a display app can force maximum brightness,
disable colour-adaptive modes, and choose integer pixel alignment. This asymmetry is worth
exploiting aggressively, and it is why the screen profile is denser than the print profile.

---

## 2. Terminology **[NORMATIVE]**

| Term | Meaning |
|---|---|
| **Module** | Smallest addressable cell of the symbol grid. |
| **Luma module** | A module considered as carrying one bit of the luma layer. Every module is a luma module. |
| **Chroma cell** | A group of modules (1×1 or 2×2 by profile) sharing one chroma symbol. |
| **Palette** | Ordered set of printable/displayable colours for a profile. |
| **Luma level** | Brightness dimension of the palette. Always 2: `L0` (dark), `L1` (light). |
| **Hue state** | Chromatic dimension: 2 or 4 states by profile. |
| **Reference module** | Module at a known position in a known colour, used to fit the capture transform. Carries no payload. |
| **Symbol** | One complete LC Code code including quiet zone. |
| **Version** | Size class, determining grid side length. |
| **Profile** | Colour/subsampling configuration. |
| **Packet** | The framed, typed, usually signed payload carried by a symbol. |
| **Issuer** | Entity holding a signing key, asserting the contents of a packet. |
| **Verifier** | Entity decoding and checking a packet, offline. |
| **Key resolver** | Verifier's local source of issuer public keys. Out of scope for LC Code (§10.5). |

---

## 3. Channel model **[INFORMATIVE]**

LC Code targets two channels that share vocabulary but behave differently.

| Impairment | Print | Screen | Design consequence |
|---|---|---|---|
| Dot gain / ink spread | Major | Absent | Module pitch floor much lower on screen |
| Halftone chroma noise | Major | Absent | Screen palette can be denser |
| Gamut clipping | Major | Minor | sRGB primaries directly addressable |
| Fade / wear | Real | Absent | — |
| Specular glare | Moderate (matte) | **Severe** | Glass is always glossy. Dominant screen impairment. |
| Illuminant shift | Severe | Minor | Display is emissive |
| Display colour mode | — | **Severe** | Night Shift / True Tone / Vivid shift whole palette |
| Auto-brightness | — | Moderate | Luma drift mid-capture |
| PWM / refresh banding | — | **Severe** | Rolling shutter × panel refresh |
| Moiré | — | **Severe** | Display pixel grid × sensor grid aliasing |
| Chroma subsampling | Real | Real | Capture-side; unchanged |

The screen impairments concentrate in effects a conforming display app can control (§13.1),
which is the asymmetry the design leans on.

---

## 4. Palette **[NORMATIVE]**

### 4.1 Structure

The palette is the Cartesian product of a **luma axis** and a **hue axis**:

```
colour(l, h)   where   l ∈ {L0, L1},   h ∈ {H0 .. H(k−1)}
```

The luma axis carries the base layer; the hue axis carries the bonus layer. Critically,
**the hue axis is defined at approximately constant luminance within each luma level.**
Tints and shades of a single hue would vary along exactly the dimension that illumination,
glare, and auto-brightness corrupt, collapsing the two layers into one another.

### 4.2 Separation requirements **[NORMATIVE]**

Nominal colours converted to CIELAB under D65:

- **Inter-level luma gap:** `min(L*[L1]) − max(L*[L0]) ≥ 40`. Guarantees monochrome fallback
  margin.
- **Intra-level luma spread:** `≤ 12` within a luma level. Keeps hue decisions off brightness.
- **Hue separation:** same-level pairs separated by `≥ 300°/k` in the a\*b\* plane, with
  `C* ≥ 35`.
- **Reproducibility:** within ΔE00 ≤ 6 on the target medium.

### 4.3 Screen palette, 4 colours (profiles `C1`, `C2`) **[INFORMATIVE — verify on panels]**

| Index | Luma | Hue | Name | sRGB | Rec.709 luma |
|---|---|---|---|---|---|
| 0 | L0 | H0 | Deep blue | `#141E82` | 35 |
| 1 | L0 | H1 | Deep red | `#961914` | 51 |
| 2 | L1 | H0 | Cyan | `#3CC8DC` | 172 |
| 3 | L1 | H1 | Yellow | `#F0CD28` | 200 |

Inter-level gap ≈ 121/255 gamma-encoded — comfortable. The light pair's 28-unit spread
should be tightened during calibration.

**[TBD]** Re-derive against measured OLED and LCD panels including PenTile subpixel
rendering. The v0.1 values were print-oriented; emissive sRGB permits more saturated choices.

### 4.4 Screen palette, 8 colours (profile `C3`) **[TBD]**

Two luma levels × four hue states ~90° apart in a\*b\*. Requires panel measurement.

### 4.5 Print palette **[TBD]**

Separate calibration. Expect desaturation relative to §4.3 and a smaller usable inter-level
gap. Print defaults to profile `C2`.

### 4.6 Monochrome palette (profile `M`) **[NORMATIVE]**

`L0 = #000000`, `L1 = #FFFFFF`. No hue axis.

---

## 5. Profiles **[NORMATIVE]**

| Profile | Palette | Chroma cell | Bits/module | Intended use |
|---|---|---|---|---|
| `M` | 2 | — | 1.00 | Fallback, worst-case media |
| `C1` | 4 | 1×1 | 2.00 | **Default for screen** |
| `C2` | 4 | 2×2 | 1.25 | **Default for print**, degraded screen |
| `C3` | 8 | 2×2 | 1.50 | High-quality screen capture |

`C2` derivation: each 2×2 block carries 4 luma bits + 1 chroma bit = 5 bits per 4 modules.
`C3`: 4 luma + 2 chroma = 6 bits per 4 modules.

**Implementations MUST support `M`, `C1`, and `C2`.** `C3` is optional.

**Why `C1` on screen and `C2` on print [INFORMATIVE]:** chroma subsampling is a *capture*
constraint, but its severity depends on how many sensor pixels cover each module. The
≥8 device-pixels-per-module rendering requirement (§13.1) gives the demosaicer far more to
work with than a printed module sitting near the resolution limit, which is enough to make
co-sited chroma viable. Print keeps the conservative default.

---

## 6. Symbol geometry **[NORMATIVE]**

### 6.1 Versions

```
side(v) = 21 + 4·(v − 1),    v ∈ 1..=32     →   21×21 … 145×145
```

**The full version range is available for any payload.** Small payloads get small symbols;
there is no floor imposed by the format.

| Version | Side | Est. usable payload, `C1`, ECC level 1 |
|---|---|---|
| 1 | 21 | ~18 B |
| 2 | 25 | ~42 B |
| 3 | 29 | ~76 B |
| 5 | 37 | ~165 B |
| 8 | 49 | ~330 B |
| 12 | 65 | ~640 B |
| 20 | 97 | ~1.6 KB |
| 32 | 145 | ~3.9 KB |

Figures are net of fixed patterns, format region, palette anchors, scattered references,
ECC, and compact-frame overhead (§9.1). A v1 symbol comfortably carries a short JSON object
— `{"name":"Fred","loc":"45.00,55.0"}` is 37 bytes as JSON, ~28 as CBOR, and fits at v2.

**[TBD]** These are hand-estimated from the module accounting in §6.2–6.4 and **must** be
replaced with values computed by the reference implementation before freeze. Treat as
order-of-magnitude only. The v1 figure in particular is sensitive to the format-region size
(§7), which at 60 modules consumes 13.6% of a v1 symbol — see the compact format region
question in §16.

**Signed packets only:** a minimal COSE_Sign1 packet costs ~110 bytes of envelope (§10.4),
which does not fit below roughly v5. Encoders **SHOULD** select v5 or above when signing and
**MUST NOT** apply that floor to unsigned payloads. This is a consequence of signature size,
not a property of the symbology.

**[INFORMATIVE]** A sub-v1 "micro" series with a single finder pattern (as Micro QR does)
would push the smallest symbol to ~11×11 for payloads under ~8 bytes. Not specified here —
it requires a separate detection path and its own format region — but the version field has
room if very small payloads turn out to matter.

Quiet zone: 2 modules on all sides. **[TBD: validate against textured backgrounds.]**

### 6.2 Fixed patterns

**Finders (3).** 7×7 concentric bullseye at top-left, top-right, bottom-left, each with a
1-module separator. **Rendered in `L0`/`L1` only** — the finder must be locatable from luma
alone, before any colour processing exists.

**Alignment marker (1).** 5×5 bullseye centred at `(side−7, side−7)` for v ≥ 2, supplying
the fourth point for a full homography rather than an affine fit. Additional interior
markers for v ≥ 14 **[TBD]**.

**Timing patterns.** Alternating `L0`/`L1` on row 6 and column 6.

**Palette anchors.** A ring outside each finder separator cycling through the full palette:
`2k` modules per finder (8 for 4-colour, 16 for 8-colour). First colour references available
after detection; used for the initial global white-balance estimate.

**Format region.** 30 modules adjacent to the top-left and top-right finders, **luma-only**.
See §7.

### 6.3 Scattered reference modules

Payload-area modules satisfying

```
(x mod 16, y mod 16) ∈ {(3,3), (11,3), (3,11), (11,11)}
```

are reference modules, cycling through the palette in raster order of tile index. One
reference per 64 modules (~1.6% overhead), four palette colours spread evenly across the
field. Excluded from the payload sequence and from masking.

**Rationale [INFORMATIVE]:** a single corner calibration patch corrects a global illuminant
shift but is useless against a glare gradient or a shadow across half the symbol. Scattered
references let the decoder fit a smooth low-order surface per channel and cancel spatial
non-uniformity — which on a glossy screen is the dominant error.

### 6.4 Screen reference border **[NORMATIVE when signalled]**

Screen symbols MAY carry a 2-module-wide ring immediately outside the quiet zone cycling
through the full palette plus white and black.

This gives the illumination fit (§12.3) a dense set of references at the geometric outside of
the symbol, which is where a global fit is weakest and where glare gradients are usually
strongest. Signalled by format bit 14.

**[INFORMATIVE]** On print this overhead would be unacceptable. On screen it costs pixels
nobody was using.

---

## 7. Format information **[NORMATIVE]**

15 payload bits, expanded to 30 modules, placed twice (top-left, top-right).

| Bits | Field | Values |
|---|---|---|
| 0–4 | Version | 1..32 |
| 5–6 | Profile | `00`=M, `01`=C1, `10`=C2, `11`=C3 |
| 7–8 | Luma ECC level | 0..3 |
| 9–10 | Chroma ECC level | 0..3 |
| 11–13 | Mask ID | 0..7 |
| 14 | Reference border present | 0/1 |

Protection: BCH(30,15), generator **[TBD]**, XORed with `0x5412` to avoid the all-zero
pattern. Luma-only; read before any colour processing.

---

## 8. Encoding pipeline **[NORMATIVE]**

```
application data
  → §9  container framing (typed, compressed, usually signed)
  → §8.1 layer split
  → §8.2 per-layer LDPC encode  (luma rate ≠ chroma rate)
  → §8.3 interleave
  → §8.4 module mapping
  → §8.5 masking
  → §8.6 colourisation
```

### 8.1 Layer split

Split the framed bitstream into luma and chroma streams sized to the available capacity of
the version+profile. The ratio is determined by geometry, not choice.

**Ordering rule:** the stream is placed **luma-first**, so the luma layer contains a complete
prefix of the packet. Combined with §12.5, a decoder that cannot recover chroma at all still
recovers a prefix.

**Interaction with signing [IMPORTANT].** A prefix of a signed packet **cannot be verified** —
the signature sits at the end of the COSE structure and the whole payload is required. So
partial recovery is a *diagnostic and retry* aid, not a usable result for signed packets. A
verifier MUST NOT act on unverified partial data. See §12.5.

### 8.2 Error correction **[TBD — base matrices must be pinned]**

Quasi-cyclic LDPC, systematic, soft-decision decoded.

| Level | Luma rate | Chroma rate |
|---|---|---|
| 0 | 0.85 | 0.65 |
| 1 | 0.75 | 0.55 |
| 2 | 0.65 | 0.45 |
| 3 | 0.55 | 0.35 |

**Unequal protection is load-bearing.** Chroma bits are measured over a noisier,
lower-bandwidth channel. Equal rates would either overprotect luma or underprotect chroma.

Derive QC-LDPC base graphs by PEG construction, girth ≥ 6, lifting sizes matched to the
capacity table. Publish as static tables in the crate — **do not generate at runtime.**

An outer CRC (§9.1) handles the LDPC error floor. **[TBD]** These rates are guesses; replace
with values measured on the §15 harness.

### 8.3 Interleaving

Interleave each layer's codeword across the module map so a physical burst — a crease, a
glare blob, a finger — spreads across many codeword positions. Version-dependent affine
permutation over module index; specify exactly **[TBD]**.

### 8.4 Module mapping

Boustrophedon column-pair walk from bottom-right upward, skipping fixed patterns, format
region, and scattered references. Luma bits consume one module each; chroma symbols consume
one chroma cell each, cells visited in the same order at cell granularity.

### 8.5 Masking

Eight masks applied **only to the luma layer**, selected by penalty score (run lengths, 2×2
blocks, finder-like sequences, dark/light balance).

Chroma is masked separately: hue index rotated by `(x + y) mod k`. Unconditional, not
signalled — prevents large monochromatic regions without adding a search dimension.

### 8.6 Colourisation

Per module: masked luma bit → level `l`; covering chroma cell's masked hue index → `h`; emit
`colour(l, h)`. In profile `M` the chroma layer is empty.

---

## 9. Payload container **[NORMATIVE]**

### 9.1 Framing

Two frame formats. The leading bit selects between them.

**Compact frame** — payloads ≤ 255 bytes:

```
| 0 | ctype:4 | cflag:2 | length:8 | body | CRC-16/CCITT |
```
Overhead: 15 bits + 2 bytes ≈ **3.9 bytes**.

**Extended frame** — payloads up to 16383 bytes:

```
| 1 | ctype:4 | cflag:2 | length:14 | body | CRC-32/ISO-HDLC |
```
Overhead: 21 bits + 4 bytes ≈ **6.6 bytes**.

Encoders **MUST** use the compact frame when the body fits, since at the small end the
difference is material: on a 10-byte payload, extended framing is 40% overhead and compact
is 28%.

**[INFORMATIVE]** CRC-16 gives ~1/65536 undetected-error probability against residual LDPC
error-floor events, versus ~1/4×10⁹ for CRC-32. That is an acceptable trade for small
payloads, where the alternative is spending a meaningful fraction of the symbol on integrity
checking that the ECC has already largely done. Signed packets have the signature as a far
stronger integrity check regardless, so the CRC there is only a fast reject path.

The CRC is the authoritative accept/reject for *decoding*. Where a signature is present
(§10) it is the authoritative accept/reject for *trusting*. LDPC convergence alone MUST NOT
be treated as decode success, and CRC success MUST NOT be treated as verification.

### 9.2 Content type (`ctype`)

| Value | Type | Notes |
|---|---|---|
| `0x0` | Opaque bytes | No interpretation |
| `0x1` | UTF-8 text | |
| `0x2` | JSON | Discouraged on the wire — see §9.4 |
| `0x3` | CBOR (RFC 8949) | **Typical case.** Unsigned structured data |
| `0x4` | COSE_Sign1 (RFC 9052) | Optional signed packet — see §10 |
| `0x5` | Multipart | Sequence of typed parts, §9.5 |
| `0x6`–`0xF` | Reserved | |

### 9.3 Compression flag (`cflag`)

| Value | Meaning |
|---|---|
| `00` | None |
| `01` | DEFLATE (RFC 1951) |
| `10` | Brotli, standard dictionary |
| `11` | Brotli, LC Code preset dictionary v1 **[TBD]** |

Compression is applied to the framed body. For signed packets, compression is applied
**outside** the COSE structure — the signature covers the uncompressed COSE payload, so
compression cannot affect verification.

**[INFORMATIVE]** `cflag 11` matters more than it looks. In the 100–400 byte range these
packets occupy, general-purpose compressors spend most of their budget warming up a
dictionary and frequently *expand* the input. A preset dictionary containing common CBOR key
strings, CWT claim labels, ISO country/language codes, and registered schema vocabulary
moves small-payload compression from useless to substantial. Build it from a real corpus;
do not hand-write it.

Note also that **signature bytes are incompressible**. For a minimal signed packet, ~64 of
~100 bytes is high-entropy signature, so expect modest total ratios and measure rather than
assume.

### 9.4 On JSON

JSON is supported but **SHOULD NOT** be used on the wire. It carries key strings,
punctuation, and whitespace that compress poorly at these sizes. CBOR is a lossless
representation of the same data model, typically 30–50% smaller before compression.

Author in JSON. Transmit CBOR. The crate exposes this at the API boundary:

```rust
pub fn encode_json(json: &str, opts: &EncodeOptions) -> Result<Symbol, LcError>;
```

### 9.5 Multipart (`ctype 0x5`)

```
| part_count:8 | (ctype:4 | len:14 | body)* |
```

Allows a signed part and an unsigned part to coexist — useful when only some claims need
attestation, or when a signed core is accompanied by mutable local annotation.

### 9.6 No text-safe encoding layer **[NORMATIVE]**

LC Code carries arbitrary bytes natively. Implementations **MUST NOT** apply base45, base64,
or any other text-safe encoding to the body. The base45 layer used by the EU Digital COVID
Certificate exists solely to satisfy QR's alphanumeric mode restriction and costs ~37%
expansion. Avoiding it is a straight saving over the closest existing deployment of this
pattern.

---

## 10. Optional signing and offline verification **[NORMATIVE when used]**

Everything in this section is **opt-in**. An encoder that never sets `ctype = 0x4` and a
decoder built without `lc-verify` are both fully conformant, and §1.2 expects that to be the
common case.

Where attestation *is* needed — a third party asserting something about the payload rather
than the holder simply carrying it — this section defines how, with no network at
verification time.

### 10.1 What "offline" means here **[INFORMATIVE but important]**

Offline verification means **no network at the moment of scanning**. The verifier holds
issuer public keys locally and checks the signature and `exp` against its own clock. Nothing
else is consulted.

It does not follow that the verifier never connects. Keys have to reach it somehow, and if
an issuer key is ever compromised or rotated, verifiers only learn about it as fast as they
update. LC Code takes no position on that mechanism (§10.5) — but a deployment that genuinely
never connects has no way to change a key after the fact, and should size its `exp` windows
and key lifetimes accordingly.

### 10.2 Structure

Signed packets use **COSE_Sign1** (RFC 9052, CBOR tag 18) — single signer, no recipient
structure, minimal overhead.

Protected header:

| Label | Name | Requirement |
|---|---|---|
| 1 | `alg` | REQUIRED. See §10.3 |
| 3 | `content type` | REQUIRED. Identifies the inner body type / schema family |
| 4 | `kid` | REQUIRED. 4-byte key hint, §10.5 |

Unprotected header MUST be empty. Any header affecting verification MUST be protected.

Payload: a **CWT claims map** (RFC 8392).

| Claim | Name | Requirement |
|---|---|---|
| 1 | `iss` | OPTIONAL. Prefer relying on `kid` (§10.5) |
| 4 | `exp` | **REQUIRED.** See §10.6 |
| 6 | `iat` | REQUIRED |
| −262144 | `lc.schema` | OPTIONAL. uint schema ID, §11 |
| −262145 | `lc.body` | REQUIRED. Application body |

`cti` (claim 7) is **deliberately absent**. It exists to support revocation, which LC Code does
not have (§10.6), and it is by construction a stable unique identifier. Omitting it means
**no field in an LC Code packet is a designed tracking token** — a real privacy benefit that
falls out of the decision to rely on expiry alone.

**[TBD]** Private claim labels are placeholders. Either register properly with IANA or move
to a documented private range and stay there.

### 10.3 Algorithms **[NORMATIVE]**

| COSE `alg` | Algorithm | Signature | Verifier | Issuer |
|---|---|---|---|---|
| −7 | ES256 (ECDSA P-256 / SHA-256) | 64 B | MUST implement | MAY use |
| −8 | EdDSA (Ed25519) | 64 B | MUST implement | MAY use |

**Verifiers MUST implement both.** Verification code is cheap and a verifier that rejects
half the ecosystem is useless.

**Issuer guidance [INFORMATIVE]:** Ed25519 is the better algorithm — deterministic, so no
nonce-reuse footgun, faster to verify, easier to implement in constant time. But **P-256 is
what hardware keystores support**. If signing keys must live in an Apple Secure Enclave or
Android StrongBox, ES256 is your only option. Choose by where the key lives, not by
elegance.

No RSA. A 256-byte signature does not fit the design.

### 10.4 Size budget **[INFORMATIVE]**

Minimal signed packet, ES256 or Ed25519:

| Component | Bytes |
|---|---|
| COSE_Sign1 tag + array | 2 |
| Protected header (alg, cty, kid) | ~15 |
| Empty unprotected map | 1 |
| Payload bstr header | 2–3 |
| CWT claims (`exp`, `iat`) | ~13 |
| `lc.body` wrapper | ~4 |
| Signature bstr | 66 |
| LC Code compact frame + CRC | ~4 |
| **Overhead subtotal** | **~107** |

**The signature dominates.** Optimising the application body below a few dozen bytes has
rapidly diminishing returns; the 64-byte signature is irreducible. Design effort is better
spent on §11 schemas keeping bodies small than on squeezing the envelope.

**This cost applies only to signed packets.** An unsigned CBOR payload pays ~4 bytes of
framing and nothing else — see §1.2 and §9.1. The v5-and-above guidance in §6.1 follows from
this table, not from the symbology.

### 10.5 Key resolution **[NORMATIVE]**

**`kid` is a 4-byte truncation of SHA-256 over the DER-encoded public key.**

Four bytes is a *lookup hint*, not an identifier. Collisions are expected at scale (birthday
bound ~65k keys) and are not fatal: a verifier holding multiple keys with the same `kid`
**MUST** attempt verification against each and accept if any succeeds. This is why 4 bytes
suffices where the EU DCC used 8 — the collision cost is a few extra signature checks, and
4 bytes of symbol area is worth more than that.

**How a verifier obtains public keys is out of scope.** LC Code specifies no trust list
format, no distribution mechanism, and no rotation procedure. Deployments pin keys at build
time, ship them with an application, fetch them from an endpoint on their own schedule, or
do something else entirely. This is the same position COSE itself takes, and it keeps the
symbology from carrying an infrastructure design nobody asked for.

**Exactly one requirement survives, and it is not negotiable:**

> **Verifiers MUST NOT accept a public key transported in the symbol.**

**Rationale [INFORMATIVE].** This is the single line separating "signed" from "signed by
someone the verifier decided to trust." If the key travels with the packet, an attacker
generates their own keypair, signs arbitrary content, embeds the matching public key, and
verification succeeds — the check confirms only that the packet is internally consistent
with a key the attacker chose. The result is a 64-byte checksum, not authentication.
Embedding a certificate or JWK is also the most common way these payloads outgrow a
scannable symbol, so the rule pays for itself twice.

Unknown `kid` **MUST** produce `VerifyFailure::UnknownKid` and fail closed. It MUST NOT fall
back to any key material found in the packet.

### 10.6 Expiry **[NORMATIVE]**

`exp` is REQUIRED on every signed packet. A packet without `exp` **MUST** be rejected
(`VerifyFailure::MissingExpiry`), never treated as non-expiring.

**LC Code has no revocation.** There is no revocation list, no Bloom filter, no `cti`, and no
status endpoint. A validly signed, unexpired packet is accepted. This is a deliberate
simplification, and the consequences below follow directly from it rather than being
edge cases.

**A signature proves origin and integrity, never freshness.** `exp` is the only freshness
control the format has.

**Consequences [INFORMATIVE but load-bearing]:**

1. **The `exp` window is the entire blast radius.** A packet issued in error, or captured
   and replayed, stays valid until it expires and there is no mechanism to cut it short.
   Choose the shortest window the use case tolerates. Where a long-lived assertion is
   genuinely needed, prefer frequent re-issuance of short-lived packets over one long-lived
   packet.

2. **Key rotation is the only remedy for compromise, and it is blunt.** If an issuer key
   leaks, the sole response is to stop verifiers accepting that `kid` — which invalidates
   *every* packet signed by it, not just the bad ones, and propagates only as fast as
   verifiers update their key material. Issuers **SHOULD** therefore use short-lived signing
   keys with regular `kid` rotation, which bounds the damage without any revocation
   infrastructure. Rotating keys is the poor man's revocation and, at this scale, usually
   the right one.

3. **The verifier clock is now the only control, and it is load-bearing.** A device with a
   wrong clock accepts expired packets or rejects valid ones, with no network to correct it.
   Verifiers **SHOULD** sanity-check their clock against `iat` and against the issuance
   metadata of their own key material, and warn on large discrepancies. A verifier that
   cannot establish a trustworthy time **MUST NOT** fall back to accepting packets
   regardless of `exp`; failing closed is the required behaviour.

**Privacy benefit.** Because no revocation means no packet identifier, nothing in an LC Code
packet is a designed tracking token. Note the residual, though: a *static* signed packet is
still a unique byte string, so a long-lived packet is trivially correlatable by any reader
that logs it. Short `exp` with re-issuance mitigates this too — the same lever helps both
problems.

### 10.7 Unsigned packets

`ctype` values other than `0x4` carry unsigned data. These remain valid for cases where no
attestation is meaningful — self-declared descriptive information, device-to-device transfer
within a trusted context, or plain data transport.

**Verifiers MUST surface signed and unsigned packets as distinguishable states** and MUST
NOT present unsigned content in a way that implies attestation. This is a UI requirement as
much as a protocol one, and it is where comparable systems have failed in practice.

### 10.8 Security properties **[NORMATIVE — state plainly in user-facing docs]**

- **No confidentiality.** Anyone within camera range can capture the symbol. Everything
  displayed is public to observers. Encrypt the body if it must be private — but note the
  verifier then needs a decryption key, which is a different and harder distribution problem
  than the key resolver (§10.5).
- **Screenshots are trivial.** Anything screen-displayed is copyable without the holder's
  device. A static LC Code symbol is not a bearer credential for anything of value. If it
  must be, bind it to a challenge or a very short `exp`.
- **A signature is not freshness, and there is no revocation.** See §10.6. The `exp` window
  is the only bound on a compromised or mistakenly-issued packet.
- **Stable packets are tracking identifiers.** A static signature is itself a unique byte
  string, so any long-lived packet is correlatable by every reader that logs it. Prefer
  short-lived re-issued packets.
- **Key distribution is the deployment's responsibility** (§10.5). LC Code's security is no
  better than the process by which verifiers got their keys, and the format provides no help
  there by design.
- **Fail closed.** Unknown `kid`, unknown `alg`, absent `exp`, expired, or malformed COSE
  MUST all result in rejection, never in "decoded but unverified" being presented as success.

### 10.9 Data protection **[INFORMATIVE — not legal advice]**

Where packets carry personal data, the *recipient* becomes a controller the moment they
capture and retain it. Under GDPR, health data and several other categories are Article 9
special categories with elevated requirements.

Design consequences that belong in the schemas, not in a compliance appendix:

- **Carry no identity fields unless the use case genuinely requires them.** A schema without
  a name field cannot leak one.
- Prefer coded fields over free text; free text invites over-disclosure with no operational
  benefit.
- Reader applications should be explicitly ephemeral by default: decode, display, discard.
- Data minimisation is easiest to enforce at schema-design time (§11) and nearly impossible
  to retrofit.

---

## 11. Schema registry **[NORMATIVE framework, registry TBD]**

For payloads conforming to a known structure, encode field *indices* rather than key strings.
This is where the real capacity lives for repetitive structured data, and it is
language-independent — which matters whenever issuer and verifier may not share a language.

```
lc.schema = uint16 schema_id
lc.body   = schema-defined structure
```

**Registry policy [TBD]:**

- 16-bit ID space.
- `0x0000` reserved.
- `0x0001`–`0xEFFF` registered, with a documented allocation process.
- `0xF000`–`0xFFFF` private use, no registration, no collision guarantees.

Third parties cannot build on this without an allocation process, so define one before v1.0
even if the registry starts with three entries.

Candidate schema families, none privileged over the others: event admission, membership and
entitlement, device provisioning and pairing, asset and inventory tagging, contact exchange,
offline vouchers, safety and medical alert, credential attestation. See Appendix A for one
worked example.

---

## 12. Decoding pipeline

Only the output is normative; decoders may implement differently. But the ordering matters
and departing from it will hurt.

### 12.1 Luma extraction and detection

Convert capture to a luma estimate. Locate the three finders by 1:1:3:1:1 run-ratio scan,
plus the alignment marker. Compute the homography.

**Do all of this before touching colour.** The finders are luma-only precisely so this stage
cannot be blocked by a white-balance failure.

### 12.2 Format decode

Sample the format region, BCH-decode both copies, accept whichever passes. Yields version,
profile, ECC levels, reference-border flag.

### 12.3 Illumination model fit

1. Sample palette anchors (§6.2) and the reference border if present (§6.4). Fit a global
   3×4 affine transform mapping observed linear RGB → nominal linear RGB.
2. Sample scattered references (§6.3). Fit a per-channel **bilinear** (v < 10) or
   **biquadratic** (v ≥ 10) correction surface over the residuals.
3. Reject samples whose residual exceeds a robust MAD-based threshold and refit.

**[INFORMATIVE]** Step 3 matters more than it looks. One reference sitting under a specular
highlight can drag a global fit badly, and on a glossy screen that is a routine occurrence
rather than an edge case.

### 12.4 Soft demodulation

Sample each module with an averaging kernel (§12.6), apply the §12.3 correction, then compute
**log-likelihood ratios, not hard decisions**:

- **Luma LLR:** corrected L\* against the two level centroids, scaled by locally estimated
  noise variance.
- **Chroma LLRs:** average corrected a\*b\* over the chroma cell, per-bit LLRs against hue
  centroids, Gray-coded so adjacent hue states differ in one bit.

Feeding soft information to the LDPC decoder is worth roughly 2 dB — a larger gain than most
palette redesigns would buy.

**Multi-frame combining.** Where a frame sequence is available, accumulate LLRs across
frames before decoding. Banding and glare move between frames, so this is very effective and
much cheaper than tightening the code rate. This is the single best use of a live camera
preview.

### 12.5 Layered decode and fallback

1. Deinterleave and LDPC-decode the **luma layer** — independent of all colour processing.
2. Decode the chroma layer if available.
3. Concatenate, verify CRC-32.
4. **Then** verify the signature (§10) if `ctype = 0x4`.

**On chroma failure:** the luma layer yields a payload prefix. For **unsigned** packets —
the common case per §1.2 — this MAY be returned flagged `Partial`, and for a
length-prefixed CBOR body a prefix is often genuinely useful. For **signed** packets it
MUST NOT be returned as content: a prefix cannot be verified. Report it as a decode-quality diagnostic to drive
retry and user guidance ("move closer", "reduce glare"), nothing more.

### 12.6 Sampling kernels

- Luma: central ~50% of module footprint.
- Chroma: central ~70% of the **chroma cell** footprint — deliberately wider, since chroma is
  spatially low-passed by the capture pipeline anyway and a wider aperture buys SNR at no
  real resolution cost.

---

## 13. Display and capture guidance

### 13.1 Conforming display application **[NORMATIVE]**

While a symbol is on screen, a conforming display app **MUST**:

1. **Set display brightness to maximum.** Highest-value single action. Overrides
   auto-brightness drift, dominates ambient, and on most OLED panels moves the driver from
   PWM to DC dimming — which is what actually removes rolling-shutter banding.
2. **Disable colour-adaptive modes**: Night Shift / Blue Light Filter, True Tone, vendor
   "Vivid"/"Saturated" profiles. Request sRGB. These shift the palette further than any
   print process does, and unlike print you cannot pre-calibrate for them.
3. **Render at integer device-pixel multiples, ≥ 8 device pixels per module.** Non-integer
   module sizes produce resampling artefacts that alias badly against the capturing sensor.
   Prefer 10–12 where area permits. **[TBD: measure moiré onset.]**
4. **Keep the symbol static.** No animation, transitions, or scroll.
5. **Render the quiet zone as pure white** at full brightness, not app chrome.

**SHOULD**: suppress notifications and status-bar overlays; include the §6.4 reference
border; prompt the user to wipe the screen after repeated failures — fingerprint smear is a
common and trivially fixed cause of glare failure.

### 13.2 Capture guidance **[INFORMATIVE]**

- **Do not force the flash when the target is a screen.** Flash on glass produces a specular
  blowout that destroys a region of the symbol. (Flash remains correct for *matte print*.)
- Prefer a slight off-axis angle, 10–20°, to move the capturing device's own reflection off
  the symbol. The homography handles the perspective.
- Prefer shorter exposure with higher gain over long exposure, reducing the number of panel
  refresh cycles integrated per readout line.
- Capture multiple frames and combine LLRs (§12.4).
- Request the rawest available frame; prefer YUV or high-quality JPEG over aggressive 4:2:0.
- Disable platform denoise/sharpening where possible — sharpening rings at module edges.

### 13.3 Print guidance **[INFORMATIVE]**

- Spot inks over CMYK halftones; halftone dithering injects spatial chroma noise directly
  into the channel LC Code depends on.
- Matte stock; pigment over dye ink.
- Profile `C2`, minimum module pitch ~0.4 mm **[TBD — measure]**.

---

## 14. Rust crate architecture

### 14.1 Workspace layout

WASM is a first-class target, which mainly means the decode+verify path must stay small and
free of C dependencies. Structure the workspace so that falls out naturally rather than
being fought for later.

```
lc-code/
├── crates/
│   ├── lc-core/        # no_std + alloc. Geometry, ECC, colour, container.
│   ├── lc-decode/      # no_std + alloc. Detect, sample, demod, decode.
│   ├── lc-verify/      # no_std + alloc. OPTIONAL. COSE/CWT, KeyResolver, ES256/Ed25519.
│   ├── lc-encode/      # std. Encoding, compression, signing.
│   ├── lc-render/      # std. SVG / raster output.
│   ├── lc-wasm/        # wasm-bindgen. Decode + verify only by default.
│   ├── lc-ffi/         # C ABI for native embedders.
│   └── lc-cli/         # std. Dev tool.
├── xtask/                 # Table generation: LDPC graphs, capacity, dictionaries.
└── fuzz/
```

The split exists so `lc-wasm` can depend on `lc-core + lc-decode` alone, adding
`lc-verify` only when a deployment actually verifies signatures. Encoding, rendering, and
signing do not belong in a scanner bundle, and neither does signature verification code in
a scanner that never encounters a signed packet.

### 14.2 WASM constraints **[NORMATIVE for the wasm target]**

- **Target bundle: < 200 KB gzipped** for decode only, **< 300 KB** with verification.
  **[TBD: validate.]** This is what
  makes it plausible for a third party to add support without a native dependency, which
  §17 argues is the highest-leverage adoption work available.
- **No C dependencies anywhere in the decode path.** `flate2` must use the `rust_backend`
  (miniz_oxide) feature; `brotli` is already pure Rust; `image` must be gated out.
- **Crypto: RustCrypto.** `p256` and `ed25519-dalek` are pure Rust and compile clean to
  `wasm32-unknown-unknown`.
- **`getrandom` must not be pulled in by the verify path.** Verification needs no
  randomness; only ECDSA *signing* does. If `getrandom` appears in the wasm dependency
  graph, a signing path has leaked into the scanner bundle — treat that as a build failure,
  not a warning.
- **No threads, no `wasm-bindgen-rayon`** in the default build.
- Accept `Uint8ClampedArray` from canvas `getImageData` directly, zero-copy where the
  bindings permit.
- Keep panics out of the hot path; build with `panic = "abort"` and return `Result`.

### 14.3 Core types

```rust
pub struct Version(u8);                       // 1..=32, validated
pub enum Profile { M, C1, C2, C3 }
pub struct EccLevel { pub luma: u8, pub chroma: u8 }

pub struct ModuleGrid<T> { side: u16, cells: Box<[T]> }

pub enum ModuleRole {
    Finder, Separator, Timing, Alignment,
    PaletteAnchor, Format, Reference, Border, Payload, Quiet,
}

/// Log-likelihood ratio. Positive = bit 0 more likely.
#[derive(Copy, Clone)] pub struct Llr(pub f32);

pub struct DecodeOutput {
    pub packet: Packet,
    pub diagnostics: Diagnostics,
}

pub enum Packet {
    /// Signature verified against a key supplied by the KeyResolver.
    Verified { body: Vec<u8>, schema: Option<u16>, exp: u64, kid: [u8; 4] },
    /// Well-formed COSE, but not verified. Body deliberately NOT exposed —
    /// see §10.8. Callers cannot accidentally use unverified content.
    Unverified { reason: VerifyFailure, kid: [u8; 4] },
    /// Decoded successfully and carries no signature.
    Unsigned { body: Vec<u8>, ctype: ContentType },
}

pub enum VerifyFailure {
    NoKeysConfigured,          // decoder built without verification
    UnknownKid, UnsupportedAlg, Expired { exp: u64 },
    MissingExpiry, BadSignature, MalformedCose, NoTrustedClock,
}

/// Key resolution is the deployment's concern (§10.5). LC Code asks only that
/// keys come from here — i.e. from anywhere other than the symbol itself.
/// Returns every candidate for a kid; 4-byte collisions are expected and the
/// verifier tries each in turn.
pub trait KeyResolver {
    fn resolve(&self, kid: [u8; 4]) -> impl Iterator<Item = PublicKey>;
}

/// Colour codes fail for diagnosable reasons; callers need them for
/// user guidance ("move closer", "tilt to reduce glare") and field tuning.
pub struct Diagnostics {
    pub luma_iterations: u16,
    pub chroma_iterations: u16,
    pub chroma_recovered: bool,
    pub illum_residual_rms: f32,
    pub references_rejected: u8,
    pub estimated_snr_db: f32,
    pub frames_combined: u8,
}
```

**Note the deliberate API shape:** `Packet::Unverified` carries no body. It is not possible
for a caller to obtain unverified content and forget to check a boolean, because there is no
content to obtain. Given §10.8, that guarantee is worth more than the flexibility it costs.

### 14.4 Public API sketch

```rust
// lc-encode
pub fn encode(data: &[u8], opts: &EncodeOptions) -> Result<Symbol, LcError>;
pub fn encode_json(json: &str, opts: &EncodeOptions) -> Result<Symbol, LcError>;
pub fn encode_signed(body: &[u8], signer: &dyn Signer, opts: &SignOptions)
    -> Result<Symbol, LcError>;

// lc-decode — no crypto, no clock, no keys.
pub struct Decoder<K = NoKeys> { /* ... */ }

impl Decoder<NoKeys> {
    pub fn new() -> Self;
}

impl<K: KeyResolver> Decoder<K> {
    /// Opt in to signature verification. Requires the `verify` feature.
    pub fn with_keys<K2: KeyResolver>(self, keys: K2, now: UnixTime) -> Decoder<K2>;

    pub fn decode_rgb(&self, px: &[u8], w: u32, h: u32, stride: u32)
        -> Result<DecodeOutput, LcError>;
}

pub struct FrameAccumulator { /* multi-frame LLR combining, §12.4 */ }
impl FrameAccumulator {
    pub fn push(&mut self, px: &[u8], w: u32, h: u32, stride: u32) -> Result<(), LcError>;
    pub fn try_decode<K: KeyResolver>(&self, d: &Decoder<K>)
        -> Result<DecodeOutput, LcError>;
}

pub trait Signer { fn alg(&self) -> CoseAlg; fn kid(&self) -> [u8; 4];
                   fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, SignError>; }
```

**The default decode path involves no cryptography at all.** `Decoder::new()` pulls in no
key material, no clock, and — with the `verify` feature off — no signature code in the
binary. A `Decoder<NoKeys>` encountering a signed packet returns
`Packet::Unverified { reason: NoKeysConfigured, .. }`: it decoded fine, but this decoder was
not built to make trust decisions and will not pretend otherwise.

`Signer` is a trait so HSM- and keystore-backed implementations (§10.3) drop in without the
crate ever holding private key material.

**Time is an explicit parameter, not read from the system clock.** Where signing is used,
`exp` is the only freshness control (§10.6), so the clock is security-critical; a caller that
cannot establish a trustworthy time should have to confront that rather than have
`SystemTime::now()` silently substituted. It also keeps `lc-verify` `no_std` and makes
expiry behaviour directly testable.

### 14.5 Build order

1. `color::space` + `palette`, with property tests for round-tripping and §4.2 validation.
   Everything downstream depends on these.
2. `symbol` geometry, layout, masking. Render random bits and eyeball. No ECC yet.
3. Container framing, CRC, BCH format region.
4. **The degradation harness (§15).** Before LDPC.
5. `ecc::ldpc`, validated standalone over AWGN with a BER-vs-SNR curve checked against
   theory, before wiring into the symbol path.
6. `decode::detect` + sampling against clean synthetic captures.
7. `color::illum` + `demod`. The harness now earns its keep.
8. *(Only if signing is in scope for your deployment.)* `lc-verify`: COSE/CWT, expiry,
   both algorithms. Test against externally-generated
   vectors, not only your own encoder — self-consistent crypto bugs are the dangerous kind.
9. Tune ECC rates against measured statistics. Replace §8.2 guesses.
10. `lc-wasm`, with bundle size in CI from the first commit.

---

## 15. Test strategy

The degradation harness is the most important non-shipping code in the project. Given a
rendered symbol, synthesise captures from composable, seeded degradations.

**Screen suite (primary):**
- Moiré: fractional device-pixel ratios (1.0/1.3/1.7/2.0×) resampled through a simulated
  sensor grid at 0–15° relative rotation.
- PWM banding: sinusoidal luminance modulation 60–480 Hz sampled with simulated rolling
  shutter at varying line readout rates.
- Specular blob: saturating sharp-Gaussian highlight swept across the symbol, including the
  pathological case directly over a finder.
- Display colour modes: Night Shift-style warm shift to ~2700 K, True Tone adaptation,
  vendor Vivid saturation boost.
- Subpixel geometry: PenTile and RGB-stripe rendering before the mosaic stage.
- Fingerprint smear: low-frequency multiplicative haze plus local contrast reduction.
- Multi-frame sequences with independent per-frame banding and glare, to validate §12.4.

**Print suite (secondary):**
- Dot gain, per-channel gamma error, registration offset, halftone simulation.
- Illuminant shift (D50/D65/tungsten/mixed LED), shading gradients 0.4–1.0×.
- Fade and abrasion.

**Shared:** defocus, motion blur, Bayer round-trip, JPEG at Q90/70/50 forced 4:2:0,
perspective warp, additive per-channel noise.

**Primary regression metric: success rate vs. degradation magnitude, per profile, reported
separately for screen and print.** A combined average hides regressions in both. A
pass/fail corpus will not tell you whether a change helped.

Also: property tests for `decode(encode(x)) == x` across payloads/versions/profiles;
fuzzing on `decode_rgb` and on the COSE parser (both take untrusted input); a frozen golden
vector file at v1.0 for cross-implementation checks; negative crypto vectors (wrong key,
tampered payload, expired, unknown `kid`) asserted to fail closed.

---

## 16. Open decisions

| # | Item | § | Notes |
|---|---|---|---|
| 1 | Screen palette values | 4.3 | Measure on OLED + LCD, incl. PenTile |
| 2 | 8-colour palette | 4.4 | Blocks `C3` |
| 3 | Print palette | 4.5 | Separate calibration |
| 4 | LDPC base matrices | 8.2 | PEG-generate, freeze as static tables |
| 5 | ECC rate table | 8.2 | Replace guesses with measured stats |
| 6 | BCH(30,15) generator | 7 | Pick, verify minimum distance |
| 7 | Interleaver permutation | 8.3 | Must be exact for interop |
| 8 | Capacity table | 6.1 | **Compute properly** — current values are estimates |
| 9 | Interior alignment markers, v≥14 | 6.2 | Placement rule |
| 10 | Quiet zone = 2 sufficient? | 6.1 | Test on textured backgrounds |
| 11 | Brotli preset dictionary | 9.3 | Build from real corpus |
| 11a | Compact format region for v1–v3? | 7, 6.1 | 60 modules is 13.6% of v1 |
| 12 | LC Code CWT claim labels | 10.2 | IANA registration or documented private range |
| 13 | Schema registry allocation policy | 11 | Needed before third-party use |
| 14 | Device px/module minimum | 13.1 | Measure moiré onset |
| 15 | Print module pitch minimum | 13.3 | Measure |
| 16 | WASM bundle size targets | 14.2 | Validate 200 KB decode-only / 300 KB with verify |

Trust list format, distribution, and root rotation were open decision #13 in v0.2. They are
now out of scope (§10.5) — the largest unbudgeted item in the previous draft, removed rather
than solved.

---

## 17. The adoption problem **[INFORMATIVE — read before building]**

Not a technical issue with the spec, and larger than every technical issue in it combined.

Every phone camera decodes QR natively with no app. **Nothing decodes LC Code.** Any use case
where the verifier is a stranger with no prior relationship, no incentive to install
anything, and limited patience faces a very steep hill. This is why essentially every
high-density colour barcode has remained niche regardless of technical merit.

Three responses, not mutually exclusive:

1. **Target closed loops.** Where both ends are controlled — an organisation, a venue, a
   device fleet, two instances of one app — the adoption problem vanishes and both the
   density gain and offline verification are straightforwardly valuable. This is likely the
   honest initial scope, and it is also where §10.5's silence on key distribution is
   cheapest: when you control the verifiers, shipping them keys is a build step rather than
   an ecosystem problem. The further the format drifts toward open, mutually-untrusting
   parties, the more that omission costs.

2. **Dual-render.** A standard QR alongside carrying a fallback URL. Works with strangers on
   day one, but reintroduces the network dependency the format exists to remove — so it is
   a bridge, not an answer.

3. **Make embedding cheap.** A small WASM bundle with a clean JS API lets a web app or POS
   vendor add support without a native dependency. §14.2 treats this as a build constraint
   from the first commit rather than a retrofit, because it is probably the highest-leverage
   engineering work available for adoption.

None of this argues against building it. But if the format succeeds it will be because
verifiers could adopt it cheaply, not because the LDPC implementation was elegant. Let that
shape where the effort goes.

---

## Appendix A — Example schema **[INFORMATIVE]**

One worked example, illustrating the §11 field-coding approach. **Not privileged** over
other schema families; included because it shows the size argument concretely.

**Schema 0x0001 — Dietary and allergen profile (draft)**

```
| allergens_eu:16 | allergens_ext:optional | severity:optional | free_text:optional |
```

- `allergens_eu`: bitmask over the 14 allergens of EU Regulation 1169/2011 Annex II. Two
  bits spare. The US FALCPA 9 are a subset, so one mask serves both markets.
- `allergens_ext`: optional CBOR array of codes outside the EU 14.
- `severity`: optional 2 bits per set allergen — avoid / intolerance / allergy /
  anaphylaxis. Operationally the most important field and frequently omitted from
  comparable systems.
- `free_text`: optional UTF-8 for cases the codes do not cover.

**Common case: 2–6 bytes.** A JSON equivalent runs 60–120. Language-independent by
construction, which matters when reader and author share no language.

Per §10.9, this schema deliberately has **no identity field**. The reader needs to know
about the allergen, not who the holder is.

Note that this example would typically be carried **unsigned** (§10.7) — self-declared
dietary information needs no attestation. It is included here to illustrate schema field
coding, not the signing path.

---

## Appendix B — Prior art and licensing **[INFORMATIVE]**

Read before publishing. JAB Code (Fraunhofer SIT) is LGPL and standardised as ISO/IEC 23634;
its LDPC usage and multi-symbol docking are the closest prior art to parts of §4–§8.
Microsoft's High Capacity Color Barcode was patent-encumbered. QR finder patterns are long
out of patent and Denso Wave has never enforced against the base symbology, but the
1:1:3:1:1 finder and boustrophedon walk are adopted here deliberately and should be
acknowledged. The COSE/CWT container design follows the EU Digital COVID Certificate closely
enough that its published specifications are worth reading as a reference implementation of
the same pattern — minus the base45 layer, which §9.6 drops.

None of this is legal advice. If LC Code goes anywhere commercial, get a freedom-to-operate
opinion rather than relying on this paragraph.
