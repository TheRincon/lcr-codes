//! Version, profile and ECC level — the three axes of the format region (§6.1, §5, §8.2).

use crate::error::LcrError;

/// Symbol size class. `side(v) = 21 + 4·(v − 1)`, `v ∈ 1..=32` (§6.1).
///
/// The full range is available to any payload; the spec's v5 floor applies to *signed*
/// packets only and is advisory, enforced (if at all) by `lcr-encode`.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Version(u8);

impl Version {
    /// Smallest symbol: 21×21.
    pub const MIN: Version = Version(1);
    /// Largest symbol: 145×145.
    pub const MAX: Version = Version(64);

    /// The largest version a **classic**, finder-bearing symbol can signal.
    ///
    /// §7's format word spends five bits on `version − 1`, so 32 is a hard ceiling there.
    /// The §6.4 ring carries no version field at all — D21 moved it into the de Bruijn
    /// position sequence, which a decoder reads from the geometry — so ringed symbols are
    /// bounded only by [`Version::MAX`]. Versions above this are therefore **ring-only**,
    /// and [`crate::FormatInfo::to_codeword`] refuses them rather than silently truncating
    /// into the profile bits.
    pub const MAX_CLASSIC: Version = Version(32);

    /// Construct from the wire value, validating the `1..=32` range.
    pub const fn new(v: u8) -> Result<Version, LcrError> {
        if v >= 1 && v <= Version::MAX.0 {
            Ok(Version(v))
        } else {
            Err(LcrError::BadVersion(v as u32))
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
        (1u8..=Version::MAX.0).map(Version::new_unchecked)
    }

    /// Every version a **classic**, finder-bearing symbol can take.
    ///
    /// Distinct from [`Version::all`] because §7's five version bits cap the classic
    /// format word at [`Version::MAX_CLASSIC`], and the classic layout's alignment grid
    /// is only defined that far. Anything iterating over classic geometry or the classic
    /// format word wants this; the ring (§6.4) wants [`Version::all`].
    pub fn all_classic() -> impl Iterator<Item = Version> {
        (1u8..=Version::MAX_CLASSIC.0).map(Version::new_unchecked)
    }

    /// The smallest version whose `side` is at least `side`, if one exists.
    pub fn from_side(side: u16) -> Option<Version> {
        if !(21..=Version::MAX.side()).contains(&side) || !(side - 21).is_multiple_of(4) {
            return None;
        }
        Some(Version(((side - 21) / 4 + 1) as u8))
    }
}

/// Colour / subsampling configuration (§5).
///
/// Measured head to head at v4 (33×33), each profile loaded to its own capacity, by
/// `cargo run -p lcr-decode --release --example profiles`. `m50` is the impairment
/// magnitude at which decoding falls through 50%; higher is better.
///
/// | | capacity | noise | blur | glare |
/// |---|---:|---:|---:|---:|
/// | `M` | 66 B | — | — | — |
/// | `C1` | 116 B | 0.128 | 4.71 | 0.600 |
/// | `C2` | 75 B | 0.128 | **5.75** | 0.600 |
/// | `C3` | **165 B** | 0.096 | 4.94 | 0.510 |
///
/// There is no profile that is best at everything, so the choice is a real one: `C1`
/// balances, `C2` buys blur tolerance with density, `C3` buys density with margin.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum Profile {
    /// Monochrome fallback. 2 colours, 1.00 bits/module.
    ///
    /// Slightly *behind* a QR code of the same size — the colour layer is where LCR Code's
    /// density advantage lives, and this profile does not have one. Its purpose is
    /// surviving a monochrome sensor or a capture too poor to trust colour at all.
    M,
    /// 4 colours, 1×1 chroma cell, 2.00 bits/module. **Default for screen.**
    ///
    /// The balanced choice and the right default: best-in-class on every impairment
    /// except blur, at 1.87× a QR version 4 at level M.
    C1,
    /// 4 colours, 2×2 chroma cell, 1.227 bits/module. **Default for print.**
    ///
    /// Keeps `C1`'s widely separated hues and spends the 2×2 cell purely on averaging
    /// four samples, which buys ~22% more blur tolerance than `C1` for ~35% less capacity.
    /// Print is also where halftone chroma noise lives, an impairment the §15 harness does
    /// not yet model — so `C2`'s case on paper rests on more than the table above.
    C2,
    /// 8 colours, 1×1 chroma cell, 3.00 bits/module. **Optional, and the least robust.**
    ///
    /// The density option: 165 B at v4 against `C1`'s 116, which is 1.42× `C1` and 2.66×
    /// a QR version 4 at level M. Nothing else in the format comes close.
    ///
    /// **It pays for that in margin, and you should opt in knowingly.** Four hue states
    /// instead of two sit 75° apart rather than 170°, at `C* ≈ 40` rather than 55, so the
    /// chroma decision is measurably harder: ~25% less noise tolerance and ~15% less
    /// glare tolerance than `C1`. Blur is a wash. Use it where the capture conditions are
    /// genuinely good — a bright screen at close range, §13.1 honoured — and where the
    /// payload will not otherwise fit. Use `C1` everywhere else.
    ///
    /// §5 makes this profile optional and requires only `M`, `C1` and `C2`, so a decoder
    /// is permitted to reject it. Do not select it automatically on a caller's behalf.
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
            Profile::M | Profile::C1 | Profile::C3 => 1,
            Profile::C2 => 2,
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
    pub const fn from_bits(b: u8) -> Result<Profile, LcrError> {
        match b {
            0b00 => Ok(Profile::M),
            0b01 => Ok(Profile::C1),
            0b10 => Ok(Profile::C2),
            0b11 => Ok(Profile::C3),
            _ => Err(LcrError::BadProfile(b)),
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
    pub const fn new(luma: u8, chroma: u8) -> Result<EccLevel, LcrError> {
        if luma > 3 {
            return Err(LcrError::BadEccLevel(luma));
        }
        if chroma > 3 {
            return Err(LcrError::BadEccLevel(chroma));
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
        assert_eq!(Version::new(64).unwrap().side(), 273);
        assert!(Version::new(0).is_err());
        assert!(Version::new(65).is_err());
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
