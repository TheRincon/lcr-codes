//! Version, profile and ECC level — the three axes of the format region (§6.1, §5, §8.2).

use crate::error::LcError;

/// Symbol size class. `side(v) = 21 + 4·(v − 1)`, `v ∈ 1..=32` (§6.1).
///
/// The full range is available to any payload; the spec's v5 floor applies to *signed*
/// packets only and is advisory, enforced (if at all) by `lc-encode`.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Version(u8);

impl Version {
    /// Smallest symbol: 21×21.
    pub const MIN: Version = Version(1);
    /// Largest symbol: 145×145.
    pub const MAX: Version = Version(32);

    /// Construct from the wire value, validating the `1..=32` range.
    pub const fn new(v: u8) -> Result<Version, LcError> {
        if v >= 1 && v <= 32 {
            Ok(Version(v))
        } else {
            Err(LcError::BadVersion(v as u32))
        }
    }

    /// Construct without validating. Only for internal loops over `1..=32`.
    pub(crate) const fn new_unchecked(v: u8) -> Version {
        Version(v)
    }

    /// The version number, `1..=32`.
    #[inline]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Grid side length in modules, excluding the quiet zone.
    #[inline]
    pub const fn side(self) -> u16 {
        21 + 4 * (self.0 as u16 - 1)
    }

    /// Total modules in the grid, `side²`.
    #[inline]
    pub const fn modules(self) -> usize {
        let s = self.side() as usize;
        s * s
    }

    /// Iterator over every version, smallest first.
    pub fn all() -> impl Iterator<Item = Version> {
        (1u8..=32).map(Version::new_unchecked)
    }

    /// The smallest version whose `side` is at least `side`, if one exists.
    pub fn from_side(side: u16) -> Option<Version> {
        if side < 21 || side > 145 || (side - 21) % 4 != 0 {
            return None;
        }
        Some(Version(((side - 21) / 4 + 1) as u8))
    }
}

/// Colour / subsampling configuration (§5).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum Profile {
    /// Monochrome fallback. 2 colours, 1.00 bits/module.
    M,
    /// 4 colours, 1×1 chroma cell, 2.00 bits/module. Default for screen.
    C1,
    /// 4 colours, 2×2 chroma cell, 1.25 bits/module. Default for print.
    C2,
    /// 8 colours, 2×2 chroma cell, 1.50 bits/module. Optional.
    C3,
}

impl Profile {
    /// Palette size.
    #[inline]
    pub const fn colors(self) -> usize {
        match self {
            Profile::M => 2,
            Profile::C1 | Profile::C2 => 4,
            Profile::C3 => 8,
        }
    }

    /// Number of hue states `k` (§4.1). `M` has no hue axis.
    #[inline]
    pub const fn hues(self) -> usize {
        match self {
            Profile::M => 1,
            Profile::C1 | Profile::C2 => 2,
            Profile::C3 => 4,
        }
    }

    /// Bits carried by one chroma cell: `log2(k)`.
    #[inline]
    pub const fn chroma_bits_per_cell(self) -> usize {
        match self {
            Profile::M => 0,
            Profile::C1 | Profile::C2 => 1,
            Profile::C3 => 2,
        }
    }

    /// Side of a chroma cell in modules (1 or 2).
    #[inline]
    pub const fn chroma_cell(self) -> u16 {
        match self {
            Profile::M | Profile::C1 => 1,
            Profile::C2 | Profile::C3 => 2,
        }
    }

    /// Two-bit wire encoding used by the format region (§7).
    #[inline]
    pub const fn to_bits(self) -> u8 {
        match self {
            Profile::M => 0b00,
            Profile::C1 => 0b01,
            Profile::C2 => 0b10,
            Profile::C3 => 0b11,
        }
    }

    /// Inverse of [`Profile::to_bits`].
    pub const fn from_bits(b: u8) -> Result<Profile, LcError> {
        match b {
            0b00 => Ok(Profile::M),
            0b01 => Ok(Profile::C1),
            0b10 => Ok(Profile::C2),
            0b11 => Ok(Profile::C3),
            _ => Err(LcError::BadProfile(b)),
        }
    }

    /// Every profile, in wire order.
    pub const ALL: [Profile; 4] = [Profile::M, Profile::C1, Profile::C2, Profile::C3];
}

/// Per-layer ECC level (§8.2). Higher is more redundancy.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub struct EccLevel {
    /// Luma layer level, `0..=3`.
    pub luma: u8,
    /// Chroma layer level, `0..=3`.
    pub chroma: u8,
}

impl EccLevel {
    /// Validate both levels are in `0..=3`.
    pub const fn new(luma: u8, chroma: u8) -> Result<EccLevel, LcError> {
        if luma > 3 {
            return Err(LcError::BadEccLevel(luma));
        }
        if chroma > 3 {
            return Err(LcError::BadEccLevel(chroma));
        }
        Ok(EccLevel { luma, chroma })
    }

    /// Spec default: level 1 on both layers.
    pub const DEFAULT: EccLevel = EccLevel { luma: 1, chroma: 1 };
}

impl Default for EccLevel {
    fn default() -> Self {
        EccLevel::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sides_match_spec() {
        assert_eq!(Version::new(1).unwrap().side(), 21);
        assert_eq!(Version::new(2).unwrap().side(), 25);
        assert_eq!(Version::new(32).unwrap().side(), 145);
        assert!(Version::new(0).is_err());
        assert!(Version::new(33).is_err());
    }

    #[test]
    fn side_roundtrip() {
        for v in Version::all() {
            assert_eq!(Version::from_side(v.side()), Some(v));
        }
        assert_eq!(Version::from_side(22), None);
    }

    #[test]
    fn profile_bits_roundtrip() {
        for p in Profile::ALL {
            assert_eq!(Profile::from_bits(p.to_bits()).unwrap(), p);
        }
    }
}
