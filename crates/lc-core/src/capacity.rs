//! Payload capacity (§6.1, spec open decision #8).
//!
//! §6.1's table is marked `[TBD]`, hand-estimated, and "order-of-magnitude only", with an
//! instruction to replace it with values computed by the reference implementation. This
//! module is that computation: it walks the real [`Layout`], builds the real [`Code`] for
//! each layer, and subtracts the real framing overhead.
//!
//! The numbers come out **lower than the spec's estimates**, by roughly 20–25% at the
//! large end. Three things the estimates did not account for:
//!
//! * Shortening. A layer length is set by geometry and never lands on `Z·n_b`, so the
//!   code is shortened and the effective rate sits below the table value (§8.2).
//! * The partial-chroma-cell rule, which costs `C2` and `C3` about 8% of chroma capacity
//!   (see [`crate::layout`]).
//! * Palette anchors and the 60-module format region, which at low versions are a large
//!   fraction of a small symbol.

use crate::ecc::ldpc::{CHROMA_RATE_INDEX, Code, LUMA_RATE_INDEX};
use crate::error::LcError;
use crate::frame::{COMPACT_MAX, Frame};
use crate::layout::Layout;
use crate::version::{EccLevel, Profile, Version};

/// What one symbol can carry.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct SymbolCapacity {
    /// Information bits recovered from the luma layer.
    pub luma_info_bits: usize,
    /// Information bits recovered from the chroma layer.
    pub chroma_info_bits: usize,
    /// Largest framed body, in bytes.
    pub body_bytes: usize,
}

impl SymbolCapacity {
    /// Total information bits across both layers, before framing.
    #[inline]
    pub const fn info_bits(&self) -> usize {
        self.luma_info_bits + self.chroma_info_bits
    }
}

/// Compute the capacity of a version/profile/ECC combination.
pub fn capacity(version: Version, profile: Profile, ecc: EccLevel) -> SymbolCapacity {
    let layout = Layout::new(version, profile);
    capacity_for(&layout, ecc)
}

/// Compute capacity from an already-built layout, avoiding the rebuild in hot loops.
pub fn capacity_for(layout: &Layout, ecc: EccLevel) -> SymbolCapacity {
    let cap = layout.capacity();
    let luma_info_bits = Code::new(LUMA_RATE_INDEX[ecc.luma as usize & 3], cap.luma_bits)
        .map(|c| c.info_bits())
        .unwrap_or(0);
    let chroma_info_bits = if cap.chroma_bits == 0 {
        0
    } else {
        Code::new(CHROMA_RATE_INDEX[ecc.chroma as usize & 3], cap.chroma_bits)
            .map(|c| c.info_bits())
            .unwrap_or(0)
    };

    let total = luma_info_bits + chroma_info_bits;
    SymbolCapacity { luma_info_bits, chroma_info_bits, body_bytes: body_bytes_for(total) }
}

/// Largest body that fits in `info_bits`, accounting for the §9.1 frame that has to go
/// around it.
///
/// The compact/extended threshold makes this non-monotonic in the crossing region: a
/// symbol that holds exactly 255 bytes compact may not hold 256 extended, because the
/// larger frame costs 22 more bits. The compact answer is preferred wherever it applies,
/// which is also what §9.1 requires of encoders.
pub fn body_bytes_for(info_bits: usize) -> usize {
    let compact = info_bits.saturating_sub(Frame::overhead_bits(0)) / 8;
    if compact <= COMPACT_MAX {
        return compact;
    }
    let extended = info_bits.saturating_sub(Frame::overhead_bits(COMPACT_MAX + 1)) / 8;
    extended.max(COMPACT_MAX).min(crate::frame::EXTENDED_MAX)
}

/// The smallest version that can carry a `body_len`-byte body.
pub fn smallest_version(
    body_len: usize,
    profile: Profile,
    ecc: EccLevel,
) -> Result<Version, LcError> {
    Version::all()
        .find(|&v| capacity(v, profile, ecc).body_bytes >= body_len)
        .ok_or(LcError::TooLarge(body_len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn capacity_grows_monotonically_with_version() {
        for profile in Profile::ALL {
            for level in 0..4u8 {
                let ecc = EccLevel { luma: level, chroma: level };
                let caps: Vec<usize> =
                    Version::all().map(|v| capacity(v, profile, ecc).body_bytes).collect();
                for w in caps.windows(2) {
                    assert!(w[1] >= w[0], "{profile:?} level {level}: {caps:?}");
                }
            }
        }
    }

    #[test]
    fn stronger_ecc_never_increases_capacity() {
        for profile in Profile::ALL {
            for v in Version::all() {
                let mut prev = usize::MAX;
                for level in 0..4u8 {
                    let c = capacity(v, profile, EccLevel { luma: level, chroma: level }).info_bits();
                    assert!(c <= prev, "v{} {profile:?} level {level}", v.get());
                    prev = c;
                }
            }
        }
    }

    #[test]
    fn colour_profiles_beat_monochrome() {
        for v in Version::all() {
            let ecc = EccLevel::DEFAULT;
            let m = capacity(v, Profile::M, ecc).info_bits();
            for p in [Profile::C1, Profile::C2, Profile::C3] {
                assert!(capacity(v, p, ecc).info_bits() > m, "v{} {p:?}", v.get());
            }
        }
    }

    /// §1.1 goal 2: exceed monochrome density by 1.5×–2× per unit area. `C1` is the
    /// screen default and is what the goal is really about.
    #[test]
    fn c1_meets_the_density_goal_against_the_mono_fallback() {
        let ecc = EccLevel::DEFAULT;
        for v in [8u8, 16, 24, 32] {
            let v = Version::new(v).unwrap();
            let m = capacity(v, Profile::M, ecc).info_bits() as f32;
            let c1 = capacity(v, Profile::C1, ecc).info_bits() as f32;
            let ratio = c1 / m;
            assert!((1.5..=2.0).contains(&ratio), "v{} density ratio {ratio}", v.get());
        }
    }

    #[test]
    fn v1_carries_a_useful_payload() {
        let c = capacity(Version::new(1).unwrap(), Profile::C1, EccLevel::DEFAULT);
        assert!(c.body_bytes >= 12, "v1 C1 holds only {} bytes", c.body_bytes);
    }

    #[test]
    fn the_spec_example_object_fits_where_promised() {
        // §6.1: `{"name":"Fred","loc":"45.00,55.0"}` is ~28 bytes as CBOR and "fits at v2".
        let cbor = crate::json::parse(r#"{"name":"Fred","loc":"45.00,55.0"}"#).unwrap().to_vec();
        let v = smallest_version(cbor.len(), Profile::C1, EccLevel::DEFAULT).unwrap();
        assert!(v.get() <= 2, "needed v{} for {} bytes", v.get(), cbor.len());
    }

    /// §10.4 budgets ~107 bytes for a minimal signed packet and §6.1 says that does not
    /// fit below roughly v5.
    #[test]
    fn a_minimal_signed_packet_needs_about_v5() {
        let v = smallest_version(107, Profile::C1, EccLevel::DEFAULT).unwrap();
        assert!((4..=6).contains(&v.get()), "signed floor landed at v{}", v.get());
    }

    #[test]
    fn body_bytes_handles_the_compact_extended_crossing() {
        // Just enough for a 255-byte compact frame.
        let bits = 255 * 8 + Frame::overhead_bits(0);
        assert_eq!(body_bytes_for(bits), 255);
        // One more byte requires the extended frame, which costs 22 extra bits.
        assert_eq!(body_bytes_for(bits + 8), 255);
        assert_eq!(body_bytes_for(bits + 32), 256);
    }

    #[test]
    fn too_large_a_payload_is_reported_not_truncated() {
        assert!(matches!(
            smallest_version(100_000, Profile::C1, EccLevel::DEFAULT),
            Err(LcError::TooLarge(100_000))
        ));
    }

    #[test]
    fn every_combination_is_constructible() {
        for v in Version::all() {
            for p in Profile::ALL {
                for luma in 0..4u8 {
                    for chroma in 0..4u8 {
                        let c = capacity(v, p, EccLevel { luma, chroma });
                        assert!(c.luma_info_bits > 0, "v{} {p:?} {luma}/{chroma}", v.get());
                        if p != Profile::M {
                            assert!(c.chroma_info_bits > 0, "v{} {p:?} {luma}/{chroma}", v.get());
                        }
                    }
                }
            }
        }
    }
}
