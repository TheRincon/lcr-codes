//! The 15-bit format region (§7).
//!
//! | Bits | Field | Values |
//! |---|---|---|
//! | 0–4 | Version | 1..32, stored as `v − 1` |
//! | 5–6 | Profile | `00`=M, `01`=C1, `10`=C2, `11`=C3 |
//! | 7–8 | Luma ECC level | 0..3 |
//! | 9–10 | Chroma ECC level | 0..3 |
//! | 11–13 | Mask ID | 0..7 |
//! | 14 | Reference border present | 0/1 |
//!
//! Bit 0 is the **most significant** bit of the 15-bit word, matching the MSB-first
//! convention used everywhere else on the wire.
//!
//! Version is stored biased by one: §6.1's range is `1..=32`, which needs 32 states and
//! exactly fills the five bits. Storing the version unbiased would waste the `0` state
//! and cap the range at 31.

use crate::bch;
use crate::error::LcError;
use crate::version::{EccLevel, Profile, Version};

/// Everything the format region carries.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct FormatInfo {
    /// Size class.
    pub version: Version,
    /// Colour configuration.
    pub profile: Profile,
    /// Per-layer ECC levels.
    pub ecc: EccLevel,
    /// Selected luma mask, `0..=7`.
    pub mask: u8,
    /// Whether the symbol carries a §6.4 reference border.
    pub reference_border: bool,
}

impl FormatInfo {
    /// Pack into the 15-bit payload word.
    pub fn to_bits(self) -> u16 {
        let v = (self.version.get() - 1) as u16;
        (v << 10)
            | ((self.profile.to_bits() as u16) << 8)
            | ((self.ecc.luma as u16) << 6)
            | ((self.ecc.chroma as u16) << 4)
            | ((self.mask as u16 & 7) << 1)
            | self.reference_border as u16
    }

    /// Unpack from the 15-bit payload word, validating every field.
    pub fn from_bits(w: u16) -> Result<FormatInfo, LcError> {
        let version = Version::new(((w >> 10) & 0x1F) as u8 + 1)?;
        let profile = Profile::from_bits(((w >> 8) & 3) as u8)?;
        let ecc = EccLevel::new(((w >> 6) & 3) as u8, ((w >> 4) & 3) as u8)?;
        Ok(FormatInfo {
            version,
            profile,
            ecc,
            mask: ((w >> 1) & 7) as u8,
            reference_border: w & 1 == 1,
        })
    }

    /// Encode to the 30-bit BCH codeword actually placed in the symbol.
    pub fn to_codeword(self) -> u32 {
        bch::encode(self.to_bits())
    }

    /// Pack into the 15-bit word a **ringed** symbol carries (D21).
    ///
    /// | Bits | Field | Values |
    /// |---|---|---|
    /// | 0–3 | Profile | 4 bits: `0`=M, `1`=C1, `2`=C2, `3`=C3, `4`–`15` reserved |
    /// | 4–5 | Luma ECC level | 0..3 |
    /// | 6–7 | Chroma ECC level | 0..3 |
    /// | 8–10 | Mask ID | 0..7 |
    /// | 11–14 | Reserved | must be zero |
    ///
    /// **Why this differs from §7.** A finder-less symbol's version is recovered from the
    /// ring's position sequence *before* the format is read — that is how the decoder finds
    /// the symbol at all — and its border flag is true by construction. Those six bits
    /// therefore tell a ring decoder nothing it does not already know. Spending them here
    /// widens the profile field from 2 bits to 4, taking the ceiling from 4 profiles to 16,
    /// and leaves 4 reserved — without lengthening the codeword, consuming ring slots, or
    /// needing a second BCH code.
    ///
    /// The reserved bits are checked to be zero on read, which keeps the wrong-read
    /// rejection that the version cross-check used to provide: a garbage word survives the
    /// BCH decode only to fail here.
    pub fn to_ring_bits(self) -> u16 {
        ((self.profile.to_bits() as u16) << 11)
            | ((self.ecc.luma as u16) << 9)
            | ((self.ecc.chroma as u16) << 7)
            | ((self.mask as u16 & 7) << 4)
    }

    /// Unpack a ringed symbol's word. `version` comes from the ring, not from the word.
    pub fn from_ring_bits(w: u16, version: Version) -> Result<FormatInfo, LcError> {
        if w & 0xF != 0 {
            return Err(LcError::FormatUndecodable);
        }
        let profile = Profile::from_bits(((w >> 11) & 0xF) as u8)?;
        let ecc = EccLevel::new(((w >> 9) & 3) as u8, ((w >> 7) & 3) as u8)?;
        Ok(FormatInfo {
            version,
            profile,
            ecc,
            mask: ((w >> 4) & 7) as u8,
            reference_border: true,
        })
    }

    /// The 30-bit BCH codeword a ringed symbol's colour choices carry (D21).
    pub fn to_ring_codeword(self) -> u32 {
        bch::encode(self.to_ring_bits())
    }

    /// Hard-decision decode of one 30-bit copy.
    pub fn from_codeword(word: u32) -> Result<(FormatInfo, u32), LcError> {
        let (bits, dist) = bch::decode(word)?;
        Ok((FormatInfo::from_bits(bits)?, dist))
    }

    /// Decode the two copies and pick the better one (§12.2).
    ///
    /// Copies are ranked by Hamming distance to their nearest codeword, so a clean copy
    /// always beats a damaged one. A copy that decodes to a *structurally invalid* format
    /// — a reserved profile or an out-of-range ECC level — is discarded even at distance
    /// 0, because the alternative is a decoder that walks off into a nonsense geometry.
    pub fn from_copies(a: u32, b: u32) -> Result<FormatInfo, LcError> {
        let ra = FormatInfo::from_codeword(a);
        let rb = FormatInfo::from_codeword(b);
        match (ra, rb) {
            (Ok((fa, da)), Ok((fb, db))) => Ok(if da <= db { fa } else { fb }),
            (Ok((f, _)), Err(_)) | (Err(_), Ok((f, _))) => Ok(f),
            (Err(e), Err(_)) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> impl Iterator<Item = FormatInfo> {
        Version::all().flat_map(|version| {
            Profile::ALL.into_iter().flat_map(move |profile| {
                (0..4u8).flat_map(move |luma| {
                    (0..4u8).flat_map(move |chroma| {
                        (0..8u8).flat_map(move |mask| {
                            [false, true]
                                .into_iter()
                                .map(move |reference_border| FormatInfo {
                                    version,
                                    profile,
                                    ecc: EccLevel { luma, chroma },
                                    mask,
                                    reference_border,
                                })
                        })
                    })
                })
            })
        })
    }

    /// The ring word carries everything the §7 word does except the two fields a ring
    /// decoder already knows.
    #[test]
    fn the_ring_word_roundtrips() {
        for f in samples() {
            let back = FormatInfo::from_ring_bits(f.to_ring_bits(), f.version).unwrap();
            assert_eq!(
                back,
                FormatInfo {
                    reference_border: true,
                    ..f
                },
                "ring word lost a field"
            );
        }
    }

    /// The four reserved bits and the twelve unused profile codes are what replaces the
    /// version cross-check: a garbage word that survives BCH has to fail here instead.
    #[test]
    fn the_ring_word_rejects_what_is_reserved() {
        let f = FormatInfo {
            version: Version::new(7).unwrap(),
            profile: Profile::C3,
            ecc: EccLevel { luma: 1, chroma: 2 },
            mask: 5,
            reference_border: true,
        };
        let w = f.to_ring_bits();
        assert!(FormatInfo::from_ring_bits(w, f.version).is_ok());
        for bit in 0..4 {
            assert!(
                FormatInfo::from_ring_bits(w | (1 << bit), f.version).is_err(),
                "reserved bit {bit} was accepted"
            );
        }
        for code in 4..16u16 {
            let w = (code << 11) | (w & 0x07FF);
            assert!(
                FormatInfo::from_ring_bits(w, f.version).is_err(),
                "profile code {code} is reserved and was accepted"
            );
        }
    }

    #[test]
    fn every_field_combination_roundtrips() {
        for f in samples() {
            assert_eq!(FormatInfo::from_bits(f.to_bits()).unwrap(), f);
            assert_eq!(FormatInfo::from_codeword(f.to_codeword()).unwrap(), (f, 0));
        }
    }

    #[test]
    fn packing_uses_all_fifteen_bits() {
        let mut union = 0u16;
        for f in samples() {
            let b = f.to_bits();
            assert!(b < 0x8000, "format word overflowed 15 bits: {b:#x}");
            union |= b;
        }
        assert_eq!(union, 0x7FFF, "some format bit is never exercised");
    }

    #[test]
    fn three_errors_per_copy_are_survivable() {
        let f = FormatInfo {
            version: Version::new(7).unwrap(),
            profile: Profile::C2,
            ecc: EccLevel { luma: 2, chroma: 3 },
            mask: 5,
            reference_border: true,
        };
        let c = f.to_codeword();
        let damaged = c ^ 0b111;
        assert_eq!(FormatInfo::from_copies(damaged, c).unwrap(), f);
        assert_eq!(FormatInfo::from_copies(c, damaged).unwrap(), f);
        assert_eq!(FormatInfo::from_copies(damaged, damaged).unwrap(), f);
    }

    #[test]
    fn a_destroyed_copy_falls_back_to_the_other() {
        let f = FormatInfo {
            version: Version::new(12).unwrap(),
            profile: Profile::C1,
            ecc: EccLevel::DEFAULT,
            mask: 3,
            reference_border: false,
        };
        // A glare blob over one finder takes its whole format copy with it.
        assert_eq!(
            FormatInfo::from_copies(0x2AAA_AAAA, f.to_codeword()).unwrap(),
            f
        );
    }

    #[test]
    fn both_copies_destroyed_is_an_error() {
        assert!(FormatInfo::from_copies(0x1234_5678, 0x2AAA_AAAA).is_err());
    }
}
