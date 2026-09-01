//! Palettes (§4) — the Cartesian product of a luma axis and a hue axis.
//!
//! ```text
//! index(l, h) = l · k + h      l ∈ {0=L0, 1=L1},  h ∈ 0..k
//! ```
//!
//! The ordering matters on the wire: the low `log2(k)` bits of a palette index are the
//! chroma symbol and the high bit is the luma bit, so [`Palette::index`] is literally the
//! §8.6 colourisation step.
//!
//! ## Why the hue axis is iso-luminant
//!
//! §4.1's constraint — hues at approximately constant `L*` within a luma level — is the
//! whole reason the two layers are independent. Tints and shades of one hue would vary
//! along precisely the axis that glare, auto-brightness and PWM banding corrupt, so a
//! luma error would become a hue error and the "monochrome fallback" property (§1.1 goal
//! 3) would evaporate. [`Palette::validate`] enforces this numerically against §4.2 and
//! is asserted for every built-in palette in the tests.
//!
//! ## Pinned: both colour palettes are derived, not transcribed
//!
//! **The §4.3 palette in spec v0.4 does not satisfy §4.2.** Measured here (and asserted
//! in [`tests::spec_4_3_palette_is_recorded_as_non_conforming`]):
//!
//! | Bound | Requirement | §4.3 palette |
//! |---|---|---|
//! | Inter-level luma gap | ≥ 40 | 42.1 ✅ |
//! | Intra-level luma spread | ≤ 12 | **13.6 ❌** |
//! | Hue separation (k=2) | ≥ 150° | **94.9° / 122.5° ❌** |
//!
//! There is also a structural problem the numeric bounds do not capture: the §4.3 hue
//! states are not the *same* hues at the two luma levels. H0 is 301.7° at L0 but 214.4°
//! at L1; H1 is 36.5° at L0 but 92.0° at L1. §12.4 demodulates chroma against a hue
//! centroid shared by both levels, so a palette whose hue states move by 80–90° between
//! levels forces the decoder to know the luma bit before it can read the hue bit — which
//! is exactly the layer coupling §4.1 exists to prevent.
//!
//! Both palettes are therefore *derived* from the §4.2 constraints rather than
//! transcribed, by the search in `xtask palette`:
//!
//! * Hue states are **iso-luminant and shared across levels** — one hue angle per state,
//!   used at both `L*` levels. Intra-level spread falls out at ≈ 0.3 rather than 13.6.
//! * A **single common `C*`** across all colours. Unequal chroma would bias the §12.4
//!   hue centroid toward whichever level is more saturated; equal chroma makes the
//!   centroid representative of both.
//! * `C*` is then the largest value the sRGB gamut permits at every (`L*`, hue) corner.
//!
//! The four-colour result — green at 136° against magenta at 316° — has a property worth
//! keeping through recalibration: the discriminating signal is G against R+B, the axis on
//! which a Bayer sensor has *twice* the photosite density. §1.4 argues chroma is the
//! low-bandwidth channel; putting the hue axis on green–magenta spends what bandwidth
//! there is where the sensor actually has it.
//!
//! §4.4's "~90° apart" is not achievable for `C3`: four iso-luminant hues at exactly 90°
//! spacing cap out at `C* = 32.4` in sRGB, below the `C* ≥ 35` floor. §4.2 only requires
//! `≥ 300°/k = 75°`, and spending the 60° of slack on the directions where sRGB is
//! roomiest reaches `C* = 40`. The pinned `C3` hues are consequently **uneven**:
//! 72°/149°/280°/356°.
//!
//! [`derive_c4`] and [`derive_c3`] reproduce the constants, and the tests fail if the
//! two ever drift apart. Recalibrating against measured panels means changing the `L*`,
//! `C*` and hue inputs — not the structure.

use crate::color::{self, Lab, Linear, Srgb8};
use crate::version::Profile;
use alloc::vec::Vec;

/// CIE D65 white point in XYZ, normalised to `Y = 1`.
pub const D65: [f32; 3] = [0.950_47, 1.0, 1.088_83];

/// Monochrome palette (§4.6).
pub const M_COLORS: [Srgb8; 2] = [Srgb8::from_hex(0x000000), Srgb8::from_hex(0xFFFFFF)];

/// The palette printed in spec §4.3.
///
/// **Not conforming to §4.2** — retained only so the deviation stays visible and
/// testable, and so calibration work can compare against the original intent. Nothing in
/// the encode or decode path uses it. See the module docs for the measured violations.
pub const SPEC_4_3_COLORS: [Srgb8; 4] = [
    Srgb8::from_hex(0x141E82), // L0 H0 deep blue
    Srgb8::from_hex(0x961914), // L0 H1 deep red
    Srgb8::from_hex(0x3CC8DC), // L1 H0 cyan
    Srgb8::from_hex(0xF0CD28), // L1 H1 yellow
];

/// Hue angles (degrees in `a*b*`) of the four-colour palette.
pub const C4_HUES: [f32; 2] = [136.0, 316.0];
/// `L*` of the dark level in the four-colour palette.
pub const C4_L0: f32 = 32.0;
/// `L*` of the light level in the four-colour palette.
pub const C4_L1: f32 = 78.0;
/// Common `C*` of the four-colour palette.
pub const C4_CHROMA: f32 = 55.0;

/// Four-colour screen palette, used by `C1` and `C2`. Derived by [`derive_c4`].
///
/// | Index | Luma | Hue | Name | sRGB | `L*` | `C*` | `h` |
/// |---|---|---|---|---|---|---|---|
/// | 0 | L0 | H0 | Deep green   | `#025901` | 32.2 | 55.1 | 136.0° |
/// | 1 | L0 | H1 | Deep magenta | `#693388` | 32.1 | 54.7 | 316.1° |
/// | 2 | L1 | H0 | Light green  | `#8BD377` | 78.1 | 55.3 | 135.9° |
/// | 3 | L1 | H1 | Light magenta| `#E3ACFF` | 78.0 | 50.9 | 316.1° |
pub const C4_COLORS: [Srgb8; 4] = [
    Srgb8::from_hex(0x025901),
    Srgb8::from_hex(0x693388),
    Srgb8::from_hex(0x8BD377),
    Srgb8::from_hex(0xE3ACFF),
];

/// Hue angles (degrees in `a*b*`) of the eight-colour palette. Uneven by design; see the
/// module docs.
pub const C8_HUES: [f32; 4] = [72.0, 149.0, 280.0, 356.0];
/// `L*` of the dark level in the eight-colour palette.
pub const C8_L0: f32 = 30.0;
/// `L*` of the light level in the eight-colour palette.
pub const C8_L1: f32 = 74.0;
/// Common `C*` of the eight-colour palette.
pub const C8_CHROMA: f32 = 40.0;

/// Eight-colour screen palette (§4.4), derived by [`derive_c3`] and pinned here.
pub const C8_COLORS: [Srgb8; 8] = [
    Srgb8::from_hex(0x653E04),
    Srgb8::from_hex(0x005225),
    Srgb8::from_hex(0x0C4784),
    Srgb8::from_hex(0x7B274C),
    Srgb8::from_hex(0xE1AC70),
    Srgb8::from_hex(0x7DC68F),
    Srgb8::from_hex(0x95B5FE),
    Srgb8::from_hex(0xF99ABC),
];

/// Colour used for the reference border's white cell (§6.4).
pub const BORDER_WHITE: Srgb8 = Srgb8::from_hex(0xFFFFFF);
/// Colour used for the reference border's black cell (§6.4).
pub const BORDER_BLACK: Srgb8 = Srgb8::from_hex(0x000000);

fn derive<const N: usize>(l0: f32, l1: f32, chroma: f32, hues: &[f32]) -> [Srgb8; N] {
    let k = hues.len();
    let mut out = [Srgb8::new(0, 0, 0); N];
    for (l_idx, &l) in [l0, l1].iter().enumerate() {
        for (h_idx, &h) in hues.iter().enumerate() {
            let (c, got) = color::lch_to_srgb8_gamut_clipped(l, chroma, h);
            debug_assert!(
                (got - chroma).abs() < 0.5,
                "C* {chroma} unreachable at L*={l} h={h}; gamut allows {got}"
            );
            out[l_idx * k + h_idx] = c;
        }
    }
    out
}

/// Derive the four-colour palette from [`C4_L0`], [`C4_L1`], [`C4_CHROMA`], [`C4_HUES`].
pub fn derive_c4() -> [Srgb8; 4] {
    derive(C4_L0, C4_L1, C4_CHROMA, &C4_HUES)
}

/// Derive the eight-colour palette from [`C8_L0`], [`C8_L1`], [`C8_CHROMA`], [`C8_HUES`].
pub fn derive_c3() -> [Srgb8; 8] {
    derive(C8_L0, C8_L1, C8_CHROMA, &C8_HUES)
}

/// A profile's colour set, with the derived quantities the decoder needs.
#[derive(Clone, Debug)]
pub struct Palette {
    /// Number of hue states `k`.
    pub hues: usize,
    /// Colours in `index(l, h) = l·k + h` order.
    pub colors: Vec<Srgb8>,
    /// Linear-light values, parallel to `colors`.
    pub linear: Vec<Linear>,
    /// CIELAB values, parallel to `colors`.
    pub lab: Vec<Lab>,
    /// Mean `L*` of each luma level, `[L0, L1]` — the two luma centroids of §12.4.
    pub level_l: [f32; 2],
    /// Mean `a*b*` of each hue state, averaged across luma levels — the hue centroids
    /// of §12.4.
    pub hue_ab: Vec<[f32; 2]>,
}

impl Palette {
    /// The palette for a profile.
    pub fn for_profile(p: Profile) -> Palette {
        match p {
            Profile::M => Palette::new(1, &M_COLORS),
            Profile::C1 | Profile::C2 => Palette::new(2, &C4_COLORS),
            Profile::C3 => Palette::new(4, &C8_COLORS),
        }
    }

    /// Build from an explicit colour list. `colors.len()` must be `2 · hues`.
    pub fn new(hues: usize, colors: &[Srgb8]) -> Palette {
        assert_eq!(colors.len(), 2 * hues, "palette must be 2 luma levels × k hues");
        let linear: Vec<Linear> = colors.iter().map(|&c| color::srgb8_to_linear(c)).collect();
        let lab: Vec<Lab> = linear.iter().map(|&c| color::linear_to_lab(c)).collect();

        let mut level_l = [0f32; 2];
        for l in 0..2 {
            let sum: f32 = (0..hues).map(|h| lab[l * hues + h].l).sum();
            level_l[l] = sum / hues as f32;
        }
        let hue_ab = (0..hues)
            .map(|h| {
                let a = (lab[h].a + lab[hues + h].a) / 2.0;
                let b = (lab[h].b + lab[hues + h].b) / 2.0;
                [a, b]
            })
            .collect();

        Palette { hues, colors: colors.to_vec(), linear, lab, hue_ab, level_l }
    }

    /// Number of colours, `2 · k`.
    #[inline]
    pub fn len(&self) -> usize {
        self.colors.len()
    }

    /// Always false — a palette has at least two colours.
    #[inline]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// `index(l, h)`. `l` is the luma bit, `h` the hue symbol.
    #[inline]
    pub fn index(&self, l: bool, h: usize) -> usize {
        (l as usize) * self.hues + (h % self.hues)
    }

    /// The colour for a luma bit and hue symbol — the §8.6 colourisation step.
    #[inline]
    pub fn color(&self, l: bool, h: usize) -> Srgb8 {
        self.colors[self.index(l, h)]
    }

    /// The colour a luma-only module renders as (§6.2).
    ///
    /// ## Pinned: hue state H0
    ///
    /// §6.2 says finders, timing and the format region are "rendered in `L0`/`L1` only",
    /// but a four-colour palette has no neutral pair to render them *in*. Using black and
    /// white would put the fixed patterns outside the palette and give the decoder luma
    /// centroids from the finders that do not match the payload's. Hue state **H0** is
    /// used instead: the fixed patterns then sit at exactly the luma levels the payload
    /// does, and the 1:1:3:1:1 finder ratio is preserved in luma regardless.
    #[inline]
    pub fn luma_only(&self, l: bool) -> Srgb8 {
        self.color(l, 0)
    }

    /// Nearest palette index to an observed CIELAB sample, by full ΔE.
    ///
    /// Only used for diagnostics and for the hard-decision path in tests; the real
    /// decoder produces LLRs (§12.4) rather than nearest-neighbour decisions.
    pub fn nearest(&self, sample: Lab) -> usize {
        let mut best = (0usize, f32::INFINITY);
        for (i, &c) in self.lab.iter().enumerate() {
            let d = color::delta_e00(sample, c);
            if d < best.1 {
                best = (i, d);
            }
        }
        best.0
    }

    /// Check the §4.2 separation requirements.
    ///
    /// Returns `Ok(())` or a human-readable description of the first violated bound.
    /// `M` is exempt from the hue rules: it has no hue axis.
    pub fn validate(&self) -> Result<PaletteMetrics, &'static str> {
        let k = self.hues;
        let dark_max = (0..k).map(|h| self.lab[h].l).fold(f32::MIN, f32::max);
        let light_min = (0..k).map(|h| self.lab[k + h].l).fold(f32::MAX, f32::min);
        let inter_level_gap = light_min - dark_max;

        let spread = |l: usize| {
            let vals = (0..k).map(|h| self.lab[l * k + h].l);
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            for v in vals {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            hi - lo
        };
        let intra_level_spread = spread(0).max(spread(1));

        let mut min_hue_sep = f32::INFINITY;
        let mut min_chroma = f32::INFINITY;
        if k > 1 {
            for l in 0..2 {
                for h1 in 0..k {
                    min_chroma = min_chroma.min(self.lab[l * k + h1].chroma());
                    for h2 in (h1 + 1)..k {
                        let a = self.lab[l * k + h1].hue_deg();
                        let b = self.lab[l * k + h2].hue_deg();
                        let d = (a - b).abs();
                        min_hue_sep = min_hue_sep.min(d.min(360.0 - d));
                    }
                }
            }
        } else {
            min_hue_sep = f32::INFINITY;
            min_chroma = f32::INFINITY;
        }

        let m = PaletteMetrics { inter_level_gap, intra_level_spread, min_hue_sep, min_chroma };

        if inter_level_gap < 40.0 {
            return Err("§4.2 inter-level luma gap below 40");
        }
        if intra_level_spread > 12.0 {
            return Err("§4.2 intra-level luma spread above 12");
        }
        if k > 1 {
            if min_hue_sep < 300.0 / k as f32 {
                return Err("§4.2 hue separation below 300°/k");
            }
            if min_chroma < 35.0 {
                return Err("§4.2 chroma below C* = 35");
            }
        }
        Ok(m)
    }
}

/// The §4.2 quantities, returned by [`Palette::validate`] so calibration work can see
/// the margin rather than just a pass/fail.
#[derive(Copy, Clone, Debug)]
pub struct PaletteMetrics {
    /// `min(L*[L1]) − max(L*[L0])`. Requirement: `≥ 40`.
    pub inter_level_gap: f32,
    /// Largest `L*` spread within a luma level. Requirement: `≤ 12`.
    pub intra_level_spread: f32,
    /// Smallest same-level hue separation in degrees. Requirement: `≥ 300°/k`.
    pub min_hue_sep: f32,
    /// Smallest `C*`. Requirement: `≥ 35`.
    pub min_chroma: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    fn show(cs: &[Srgb8]) -> alloc::string::String {
        cs.iter().map(|c| format!("{:#08X} ", c.to_hex())).collect()
    }

    #[test]
    fn every_shipped_palette_meets_spec_4_2() {
        for prof in Profile::ALL {
            let p = Palette::for_profile(prof);
            match p.validate() {
                Ok(m) => {
                    assert!(m.inter_level_gap >= 40.0, "{prof:?} {m:?}");
                    assert!(m.intra_level_spread <= 12.0, "{prof:?} {m:?}");
                    if p.hues > 1 {
                        assert!(m.min_hue_sep >= 300.0 / p.hues as f32, "{prof:?} {m:?}");
                        assert!(m.min_chroma >= 35.0, "{prof:?} {m:?}");
                    }
                }
                Err(e) => panic!("{prof:?} palette violates {e}"),
            }
        }
    }

    #[test]
    fn palettes_match_their_derivations() {
        assert_eq!(derive_c4(), C4_COLORS, "C4_COLORS drifted; expected {}", show(&derive_c4()));
        assert_eq!(derive_c3(), C8_COLORS, "C8_COLORS drifted; expected {}", show(&derive_c3()));
    }

    /// Locks in *why* the shipped palette departs from the printed spec, so the
    /// deviation cannot be quietly reverted by someone transcribing §4.3.
    #[test]
    fn spec_4_3_palette_is_recorded_as_non_conforming() {
        let p = Palette::new(2, &SPEC_4_3_COLORS);
        assert_eq!(p.validate().err(), Some("§4.2 intra-level luma spread above 12"));

        // The structural failure the scalar bounds miss: a hue state must mean the same
        // hue at both luma levels, or chroma demodulation cannot be independent of luma.
        let drift = |h: usize| {
            let d = (p.lab[h].hue_deg() - p.lab[2 + h].hue_deg()).abs();
            d.min(360.0 - d)
        };
        assert!(drift(0) > 80.0, "H0 drift {}", drift(0));
        assert!(drift(1) > 50.0, "H1 drift {}", drift(1));

        // The shipped palette holds each hue state fixed across levels.
        let q = Palette::for_profile(Profile::C1);
        for h in 0..2 {
            let d = (q.lab[h].hue_deg() - q.lab[2 + h].hue_deg()).abs();
            assert!(d.min(360.0 - d) < 3.0, "shipped H{h} drifts {d}");
        }
    }

    #[test]
    fn mono_palette_is_exempt_from_hue_rules() {
        let p = Palette::for_profile(Profile::M);
        assert_eq!(p.hues, 1);
        assert!(p.validate().is_ok());
    }

    #[test]
    fn index_ordering_is_luma_major() {
        let p = Palette::for_profile(Profile::C1);
        assert_eq!(p.color(false, 0), C4_COLORS[0]); // L0 H0
        assert_eq!(p.color(false, 1), C4_COLORS[1]); // L0 H1
        assert_eq!(p.color(true, 0), C4_COLORS[2]);  // L1 H0
        assert_eq!(p.color(true, 1), C4_COLORS[3]);  // L1 H1

        let q = Palette::for_profile(Profile::C3);
        for l in 0..2 {
            for h in 0..4 {
                assert_eq!(q.index(l == 1, h), l * 4 + h);
            }
        }
    }

    /// The monochrome fallback of §1.1 goal 3: thresholding a colour symbol at the
    /// midpoint between the two level centroids must recover the luma bit for every
    /// colour, with margin.
    #[test]
    fn luma_layer_survives_a_monochrome_sensor() {
        for prof in [Profile::C1, Profile::C2, Profile::C3] {
            let p = Palette::for_profile(prof);
            let mid = (p.level_l[0] + p.level_l[1]) / 2.0;
            for i in 0..p.len() {
                let is_light = i >= p.hues;
                assert_eq!(p.lab[i].l > mid, is_light, "{prof:?} colour {i}");
                assert!((p.lab[i].l - mid).abs() > 15.0, "{prof:?} colour {i} too close to mid");
            }
        }
    }

    #[test]
    fn nearest_recovers_every_palette_entry() {
        for prof in Profile::ALL {
            let p = Palette::for_profile(prof);
            for i in 0..p.len() {
                assert_eq!(p.nearest(p.lab[i]), i, "{prof:?} index {i}");
            }
        }
    }
}
