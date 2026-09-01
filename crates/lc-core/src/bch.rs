//! Format-region error correction: shortened BCH(31,16) → BCH(30,15) (§7).
//!
//! ## Pinned: the generator (spec open decision #6)
//!
//! `g(x) = m₁(x)·m₃(x)·m₅(x)` over GF(2⁵) with primitive polynomial `x⁵ + x² + 1`,
//! giving the degree-15 generator
//!
//! ```text
//! g(x) = x¹⁵ + x¹¹ + x¹⁰ + x⁹ + x⁸ + x⁷ + x⁵ + x³ + x² + x + 1   (0x8FAF)
//! ```
//!
//! This is the primitive triple-error-correcting BCH code of length 31. Shortening one
//! information bit yields (30,15); the shortened code retains **minimum distance 7**
//! (verified exhaustively in [`tests::minimum_distance_is_seven`]), so it corrects any
//! 3 errors and detects any 6 — comfortable for a 30-module region that sits right next
//! to a finder, where glare damage is both likely and spatially clustered.
//!
//! ## Decoding is exhaustive, not algebraic
//!
//! There are only 2¹⁵ = 32768 codewords. Scoring all of them is a few hundred
//! microseconds once per decode, and buys two things a Berlekamp–Massey decoder does not:
//! it is *maximum likelihood* rather than bounded-distance (so it often recovers 4+
//! errors), and it accepts **soft** input directly ([`decode_soft`]), which matters
//! because the format region is sampled from the same noisy capture as everything else
//! and §12.4 has already produced LLRs for it.

use crate::error::LcError;

/// Generator polynomial of the shortened BCH(30,15) format code.
pub const GENERATOR: u32 = 0x8FAF;

/// Data bits carried by the format region (§7).
pub const DATA_BITS: u32 = 15;

/// Modules the format codeword occupies, per copy (§7).
pub const CODE_BITS: u32 = 30;

/// Constant XOR applied to the codeword so an all-zero format does not render as a
/// blank region (§7).
///
/// The spec names `0x5412`, QR's 15-bit format mask. The LC codeword is 30 bits, so the
/// constant is applied to both halves: `mask = (0x5412 << 15) | 0x5412`.
pub const MASK: u32 = (0x5412 << 15) | 0x5412;

/// Basis codewords: `BASIS[i]` is the *unmasked* codeword of the message `1 << i`.
///
/// The code is linear, so any codeword is the XOR of the basis vectors selected by the
/// set bits of the message. Precomputing them turns both decoders into a Gray-code walk
/// over the 32768 codewords with **one XOR per step**, which is what makes exhaustive
/// maximum-likelihood decoding cheap enough to prefer over Berlekamp–Massey.
const BASIS: [u32; DATA_BITS as usize] = build_basis();

const fn build_basis() -> [u32; DATA_BITS as usize] {
    let mut out = [0u32; DATA_BITS as usize];
    let mut i = 0;
    while i < DATA_BITS as usize {
        out[i] = encode_raw(1u16 << i);
        i += 1;
    }
    out
}

/// Systematic encode without the [`MASK`] XOR. `const` so [`BASIS`] can be built at
/// compile time.
const fn encode_raw(data: u16) -> u32 {
    let data = (data & 0x7FFF) as u32;
    let mut rem = data << DATA_BITS;
    // Long division by the degree-15 generator, high term first.
    let mut shift = DATA_BITS as i32 - 1;
    while shift >= 0 {
        if rem & (1 << (shift as u32 + DATA_BITS)) != 0 {
            rem ^= GENERATOR << shift;
        }
        shift -= 1;
    }
    (data << DATA_BITS) | rem
}

/// Systematically encode 15 data bits into a masked 30-bit codeword.
///
/// Layout is `data(15) || parity(15)`, MSB first, then XOR [`MASK`].
pub const fn encode(data: u16) -> u32 {
    encode_raw(data) ^ MASK
}

/// Standard reflected binary code: successive values differ in exactly one bit.
#[inline]
const fn gray(i: u16) -> u16 {
    i ^ (i >> 1)
}

/// Hard-decision maximum-likelihood decode of a 30-bit received word.
///
/// Returns `(data, hamming_distance)`, or [`LcError::FormatUndecodable`] if the closest
/// codeword is further than `⌊(d−1)/2⌋ = 3` away — the point past which a "correction"
/// is more likely to invent a plausible wrong version than to recover the right one.
pub fn decode(received: u32) -> Result<(u16, u32), LcError> {
    let target = (received & 0x3FFF_FFFF) ^ MASK;
    let mut word = 0u32; // codeword for message gray(0) = 0
    let mut best = (0u16, (word ^ target).count_ones());
    for i in 1u32..=0x7FFF {
        // gray(i) differs from gray(i-1) in bit `trailing_zeros(i)`.
        word ^= BASIS[i.trailing_zeros() as usize];
        let d = (word ^ target).count_ones();
        if d < best.1 {
            best = (gray(i as u16), d);
            if d == 0 {
                break;
            }
        }
    }
    if best.1 > 3 { Err(LcError::FormatUndecodable) } else { Ok(best) }
}

/// Soft-decision maximum-likelihood decode.
///
/// `llrs[i]` is the log-likelihood ratio for codeword bit `i` in MSB-first order, with
/// **positive meaning bit 0 is more likely** (the sign convention of `Llr` in §14.3).
///
/// Returns `(data, margin)` where `margin` is the correlation gap between the best and
/// second-best codeword — a directly usable confidence signal, and the right basis for
/// choosing between the two format copies of §12.2. Comparing margins beats comparing
/// Hamming distances, because two copies that both "pass" at distance ≤ 3 can still
/// disagree, and the one whose runner-up is further away is the one to believe.
pub fn decode_soft(llrs: &[f32; CODE_BITS as usize]) -> (u16, f32) {
    // Fold the mask into the observation so the walk stays over raw codewords.
    let mut l = *llrs;
    for (i, v) in l.iter_mut().enumerate() {
        if MASK >> (CODE_BITS as usize - 1 - i) & 1 == 1 {
            *v = -*v;
        }
    }
    // Correlation of codeword c is Σ (1 − 2·c_i)·L_i; start from the all-zero codeword.
    let mut corr: f32 = l.iter().sum();
    let mut word = 0u32;
    let mut best = (0u16, corr);
    let mut second = f32::NEG_INFINITY;
    for i in 1u32..=0x7FFF {
        let mut delta = BASIS[i.trailing_zeros() as usize];
        word ^= delta;
        while delta != 0 {
            let bit = delta.trailing_zeros();
            let pos = (CODE_BITS - 1 - bit) as usize;
            // The bit has just been flipped, so `word` already holds its new value:
            // it contributed −L before if it is now 1, and +L before if it is now 0.
            corr += if word >> bit & 1 == 1 { -2.0 * l[pos] } else { 2.0 * l[pos] };
            delta &= delta - 1;
        }
        if corr > best.1 {
            second = best.1;
            best = (gray(i as u16), corr);
        } else if corr > second {
            second = corr;
        }
    }
    (best.0, best.1 - second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn generator_divides_x31_plus_1() {
        // x^31 + 1 mod g(x) must be zero for a length-31 cyclic code.
        let mut rem: u64 = (1u64 << 31) | 1;
        while rem.leading_zeros() <= 63 - 15 {
            let sh = 63 - rem.leading_zeros() - 15;
            rem ^= (GENERATOR as u64) << sh;
        }
        assert_eq!(rem, 0);
    }

    #[test]
    fn minimum_distance_is_seven() {
        let mut min = u32::MAX;
        for m in 1u16..=0x7FFF {
            // Unmasked weight: MASK cancels in a difference of codewords.
            min = min.min((encode(m) ^ MASK).count_ones());
        }
        assert_eq!(min, 7, "shortened BCH(30,15) must retain d=7");
    }

    #[test]
    fn roundtrip_clean() {
        for m in (0u16..=0x7FFF).step_by(7) {
            assert_eq!(decode(encode(m)).unwrap(), (m, 0));
        }
    }

    #[test]
    fn corrects_up_to_three_errors() {
        // The code is linear and the decoder is distance-based, so behaviour under an
        // error pattern is independent of the message; two messages are belt and braces.
        for m in [0x0000u16, 0x2A5C] {
            let c = encode(m);
            for a in 0..30 {
                for b in (a + 1)..30 {
                    for d in (b + 1)..30 {
                        let corrupt = c ^ (1 << a) ^ (1 << b) ^ (1 << d);
                        assert_eq!(decode(corrupt).unwrap(), (m, 3), "flips {a},{b},{d}");
                    }
                }
            }
        }
    }

    #[test]
    fn gray_walk_visits_every_message_once() {
        let mut seen = alloc::vec![false; 1 << 15];
        for i in 0u32..=0x7FFF {
            let g = gray(i as u16) as usize;
            assert!(!seen[g]);
            seen[g] = true;
        }
        assert!(seen.into_iter().all(|s| s));
    }

    #[test]
    fn all_zero_data_is_not_a_blank_region() {
        assert_ne!(encode(0), 0);
        assert_ne!(encode(0), 0x3FFF_FFFF);
    }

    #[test]
    fn soft_decode_beats_hard_on_four_errors() {
        let m = 0x2A5Cu16;
        let c = encode(m);
        // Four flips: outside the hard decoder's radius.
        let flips = [2u32, 9, 17, 26];
        let mut corrupt = c;
        for f in flips {
            corrupt ^= 1 << f;
        }
        // Four errors is past the guaranteed radius: the hard decoder either gives up or
        // — worse, and what happens here — confidently returns a different version.
        assert_ne!(decode(corrupt).map(|(m, _)| m), Ok(m));

        // Soft: the flipped bits are the *least* confident ones, which is the realistic
        // case — a sample near the decision boundary is what flips in the first place.
        let mut llrs = [0f32; 30];
        for (i, l) in llrs.iter_mut().enumerate() {
            let pos = CODE_BITS as usize - 1 - i;
            let bit = (c >> pos) & 1;
            let weak = flips.contains(&(pos as u32));
            let mag = if weak { 0.3 } else { 4.0 };
            *l = if bit == 0 { mag } else { -mag };
        }
        // Flip the sign of the weak bits to reflect the erroneous hard decision.
        for f in flips {
            llrs[(CODE_BITS - 1 - f) as usize] *= -1.0;
        }
        let (got, margin) = decode_soft(&llrs);
        assert_eq!(got, m);
        assert!(margin > 0.0);
    }

    #[test]
    fn soft_and_hard_agree_when_clean() {
        let msgs: Vec<u16> = (0..8).map(|i| i * 4001).collect();
        for m in msgs {
            let c = encode(m);
            let mut llrs = [0f32; 30];
            for (i, l) in llrs.iter_mut().enumerate() {
                let bit = (c >> (CODE_BITS as usize - 1 - i)) & 1;
                *l = if bit == 0 { 5.0 } else { -5.0 };
            }
            assert_eq!(decode_soft(&llrs).0, m);
        }
    }
}
