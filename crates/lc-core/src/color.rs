//! Colour space conversions used by the palette definition and by soft demodulation
//! (§4.2, §12.3, §12.4).
//!
//! Deliberately small and dependency-free: sRGB ⇄ linear ⇄ CIEXYZ(D65) ⇄ CIELAB, plus
//! polar CIELCh for palette derivation. Everything is `f32`; the precision that matters
//! is in the *illumination fit*, not here.
//!
//! Two things are worth knowing before reading further:
//!
//! * **The decoder works in linear light, not sRGB.** Glare, shading and the display's
//!   own transfer function are all multiplicative in linear light and become an ugly
//!   non-linearity after gamma encoding. The §12.3 affine correction is therefore fitted
//!   on [`Linear`] values, and only the final per-module decision is taken in
//!   [`Lab`], where "distance" matches perceptual separation and §4.2's ΔE budget.
//! * **CIELAB's `a*b*` plane is the chroma channel.** Hue decisions use `a*b*` only, so
//!   that a brightness error — which is exactly what auto-brightness, PWM banding and
//!   glare produce — cannot move a module across a hue boundary (§4.1).

use crate::palette::D65;

/// Non-linear sRGB, 0..=255 per channel. What a camera hands you.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Hash)]
pub struct Srgb8 {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Srgb8 {
    /// Construct from components.
    pub const fn new(r: u8, g: u8, b: u8) -> Srgb8 {
        Srgb8 { r, g, b }
    }

    /// Parse `#RRGGBB` or `RRGGBB`.
    pub const fn from_hex(hex: u32) -> Srgb8 {
        Srgb8 {
            r: (hex >> 16) as u8,
            g: (hex >> 8) as u8,
            b: hex as u8,
        }
    }

    /// Pack to `0xRRGGBB`.
    pub const fn to_hex(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }

    /// Rec.709 luma of the *gamma-encoded* values, as quoted in the §4.3 palette table.
    ///
    /// This is not a perceptual quantity and is not used for decisions; it exists so the
    /// palette tests can be checked against the numbers printed in the spec.
    pub fn rec709_luma(self) -> f32 {
        0.2126 * self.r as f32 + 0.7152 * self.g as f32 + 0.0722 * self.b as f32
    }
}

/// Linear-light RGB, nominally 0.0..=1.0 but unclamped: the illumination fit produces
/// out-of-range values and clamping early destroys the residual it needs to see.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct Linear {
    /// Red.
    pub r: f32,
    /// Green.
    pub g: f32,
    /// Blue.
    pub b: f32,
}

impl Linear {
    /// Construct from components.
    pub const fn new(r: f32, g: f32, b: f32) -> Linear {
        Linear { r, g, b }
    }

    /// Component-wise clamp into `0.0..=1.0`.
    pub fn clamped(self) -> Linear {
        Linear {
            r: self.r.clamp(0.0, 1.0),
            g: self.g.clamp(0.0, 1.0),
            b: self.b.clamp(0.0, 1.0),
        }
    }

    /// True when every component already lies in `0.0..=1.0` (within `1e-4`).
    pub fn in_gamut(self) -> bool {
        let ok = |v: f32| v >= -1e-4 && v <= 1.0 + 1e-4;
        ok(self.r) && ok(self.g) && ok(self.b)
    }

    /// Relative luminance `Y`.
    pub fn luminance(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }
}

/// CIELAB under D65.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct Lab {
    /// Lightness, 0..=100.
    pub l: f32,
    /// Green–red axis.
    pub a: f32,
    /// Blue–yellow axis.
    pub b: f32,
}

impl Lab {
    /// Construct from components.
    pub const fn new(l: f32, a: f32, b: f32) -> Lab {
        Lab { l, a, b }
    }

    /// Chroma `C* = √(a*² + b*²)`.
    pub fn chroma(self) -> f32 {
        (self.a * self.a + self.b * self.b).sqrt()
    }

    /// Hue angle in degrees, `0..360`.
    pub fn hue_deg(self) -> f32 {
        let h = self.b.atan2(self.a).to_degrees();
        if h < 0.0 { h + 360.0 } else { h }
    }

    /// Euclidean distance in the `a*b*` plane only — the chroma-channel metric used by
    /// hue demodulation (§12.4).
    pub fn ab_distance(self, other: Lab) -> f32 {
        let da = self.a - other.a;
        let db = self.b - other.b;
        (da * da + db * db).sqrt()
    }
}

/// sRGB transfer function, encoded → linear.
#[inline]
pub fn srgb_to_linear_f(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB transfer function, linear → encoded.
#[inline]
pub fn linear_to_srgb_f(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Decode an 8-bit sRGB triple to linear light.
pub fn srgb8_to_linear(c: Srgb8) -> Linear {
    Linear {
        r: srgb_to_linear_f(c.r as f32 / 255.0),
        g: srgb_to_linear_f(c.g as f32 / 255.0),
        b: srgb_to_linear_f(c.b as f32 / 255.0),
    }
}

/// Encode linear light to an 8-bit sRGB triple, clamping out-of-gamut values.
pub fn linear_to_srgb8(c: Linear) -> Srgb8 {
    let q = |v: f32| (linear_to_srgb_f(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
    Srgb8 {
        r: q(c.r),
        g: q(c.g),
        b: q(c.b),
    }
}

/// Linear sRGB → CIEXYZ (D65), sRGB primaries.
pub fn linear_to_xyz(c: Linear) -> [f32; 3] {
    [
        0.412_456_4 * c.r + 0.357_576_1 * c.g + 0.180_437_5 * c.b,
        0.212_672_9 * c.r + 0.715_152_2 * c.g + 0.072_175_0 * c.b,
        0.019_333_9 * c.r + 0.119_192_0 * c.g + 0.950_304_1 * c.b,
    ]
}

/// CIEXYZ (D65) → linear sRGB. May return out-of-gamut components.
pub fn xyz_to_linear(xyz: [f32; 3]) -> Linear {
    let [x, y, z] = xyz;
    Linear {
        r: 3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z,
        g: -0.969_266_0 * x + 1.876_010_8 * y + 0.041_556_0 * z,
        b: 0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z,
    }
}

#[inline]
fn lab_f(t: f32) -> f32 {
    const DELTA3: f32 = 216.0 / 24389.0;
    if t > DELTA3 {
        t.cbrt()
    } else {
        t * (841.0 / 108.0) + 4.0 / 29.0
    }
}

#[inline]
fn lab_f_inv(t: f32) -> f32 {
    const DELTA: f32 = 6.0 / 29.0;
    if t > DELTA {
        t * t * t
    } else {
        3.0 * DELTA * DELTA * (t - 4.0 / 29.0)
    }
}

/// CIEXYZ (D65) → CIELAB.
pub fn xyz_to_lab(xyz: [f32; 3]) -> Lab {
    let fx = lab_f(xyz[0] / D65[0]);
    let fy = lab_f(xyz[1] / D65[1]);
    let fz = lab_f(xyz[2] / D65[2]);
    Lab {
        l: 116.0 * fy - 16.0,
        a: 500.0 * (fx - fy),
        b: 200.0 * (fy - fz),
    }
}

/// CIELAB → CIEXYZ (D65).
pub fn lab_to_xyz(lab: Lab) -> [f32; 3] {
    let fy = (lab.l + 16.0) / 116.0;
    let fx = fy + lab.a / 500.0;
    let fz = fy - lab.b / 200.0;
    [
        lab_f_inv(fx) * D65[0],
        lab_f_inv(fy) * D65[1],
        lab_f_inv(fz) * D65[2],
    ]
}

/// Linear sRGB → CIELAB.
pub fn linear_to_lab(c: Linear) -> Lab {
    xyz_to_lab(linear_to_xyz(c))
}

/// CIELAB → linear sRGB (possibly out of gamut).
pub fn lab_to_linear(lab: Lab) -> Linear {
    xyz_to_linear(lab_to_xyz(lab))
}

/// 8-bit sRGB → CIELAB.
pub fn srgb8_to_lab(c: Srgb8) -> Lab {
    linear_to_lab(srgb8_to_linear(c))
}

/// Polar CIELAB (`L*`, `C*`, `h°`) → 8-bit sRGB, reducing chroma until the result is
/// inside the sRGB gamut.
///
/// This is how the §4.4 eight-colour palette is derived: hue angles and lightness are
/// chosen first — those are the properties §4.2 constrains — and chroma is then taken as
/// high as the display can actually reproduce. Binary search over `C*` converges in ~20
/// steps and is exact to `1e-3`.
pub fn lch_to_srgb8_gamut_clipped(l: f32, c: f32, h_deg: f32) -> (Srgb8, f32) {
    let at = |cc: f32| {
        let rad = h_deg.to_radians();
        Lab::new(l, cc * rad.cos(), cc * rad.sin())
    };
    if lab_to_linear(at(c)).in_gamut() {
        return (linear_to_srgb8(lab_to_linear(at(c))), c);
    }
    let (mut lo, mut hi) = (0.0f32, c);
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if lab_to_linear(at(mid)).in_gamut() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (linear_to_srgb8(lab_to_linear(at(lo)).clamped()), lo)
}

/// CIEDE2000 colour difference. Used only to check the §4.2 reproducibility bound.
pub fn delta_e00(a: Lab, b: Lab) -> f32 {
    let (l1, a1, b1) = (a.l, a.a, a.b);
    let (l2, a2, b2) = (b.l, b.a, b.b);
    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();
    let cbar = 0.5 * (c1 + c2);
    let c7 = cbar.powi(7);
    let g = 0.5 * (1.0 - (c7 / (c7 + 25f32.powi(7))).sqrt());
    let a1p = (1.0 + g) * a1;
    let a2p = (1.0 + g) * a2;
    let c1p = (a1p * a1p + b1 * b1).sqrt();
    let c2p = (a2p * a2p + b2 * b2).sqrt();
    let hp = |ap: f32, bp: f32| {
        if ap == 0.0 && bp == 0.0 {
            0.0
        } else {
            let h = bp.atan2(ap).to_degrees();
            if h < 0.0 { h + 360.0 } else { h }
        }
    };
    let h1p = hp(a1p, b1);
    let h2p = hp(a2p, b2);
    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let dhp = 2.0 * (c1p * c2p).sqrt() * (dhp.to_radians() / 2.0).sin();
    let lbar = 0.5 * (l1 + l2);
    let cbarp = 0.5 * (c1p + c2p);
    let hbarp = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        0.5 * (h1p + h2p)
    } else if h1p + h2p < 360.0 {
        0.5 * (h1p + h2p + 360.0)
    } else {
        0.5 * (h1p + h2p - 360.0)
    };
    let t = 1.0 - 0.17 * (hbarp - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hbarp).to_radians().cos()
        + 0.32 * (3.0 * hbarp + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hbarp - 63.0).to_radians().cos();
    let dtheta = 30.0 * (-(((hbarp - 275.0) / 25.0).powi(2))).exp();
    let cbarp7 = cbarp.powi(7);
    let rc = 2.0 * (cbarp7 / (cbarp7 + 25f32.powi(7))).sqrt();
    let lb = (lbar - 50.0).powi(2);
    let sl = 1.0 + 0.015 * lb / (20.0 + lb).sqrt();
    let sc = 1.0 + 0.045 * cbarp;
    let sh = 1.0 + 0.015 * cbarp * t;
    let rt = -(2.0 * dtheta).to_radians().sin() * rc;
    ((dlp / sl).powi(2) + (dcp / sc).powi(2) + (dhp / sh).powi(2) + rt * (dcp / sc) * (dhp / sh))
        .max(0.0)
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_transfer_roundtrips() {
        for v in 0..=255u8 {
            let f = v as f32 / 255.0;
            let back = linear_to_srgb_f(srgb_to_linear_f(f));
            assert!((back - f).abs() < 1e-5, "v={v}");
        }
    }

    #[test]
    fn srgb8_roundtrips_through_lab() {
        for &c in &[
            Srgb8::new(0, 0, 0),
            Srgb8::new(255, 255, 255),
            Srgb8::from_hex(0x141E82),
            Srgb8::from_hex(0x3CC8DC),
            Srgb8::from_hex(0xF0CD28),
            Srgb8::from_hex(0x961914),
        ] {
            let lab = srgb8_to_lab(c);
            let back = linear_to_srgb8(lab_to_linear(lab));
            assert_eq!(back, c, "{c:?} -> {lab:?}");
        }
    }

    #[test]
    fn white_is_l100_neutral() {
        let lab = srgb8_to_lab(Srgb8::new(255, 255, 255));
        assert!((lab.l - 100.0).abs() < 0.05, "{lab:?}");
        assert!(lab.chroma() < 0.05, "{lab:?}");
    }

    #[test]
    fn black_is_l0() {
        let lab = srgb8_to_lab(Srgb8::new(0, 0, 0));
        assert!(lab.l.abs() < 1e-3);
    }

    #[test]
    fn delta_e_of_identical_colors_is_zero() {
        let lab = srgb8_to_lab(Srgb8::from_hex(0x961914));
        assert!(delta_e00(lab, lab) < 1e-4);
    }

    #[test]
    fn delta_e_matches_sharma_vector() {
        // Sharma et al. CIEDE2000 test data, pair 1.
        let a = Lab::new(50.0000, 2.6772, -79.7751);
        let b = Lab::new(50.0000, 0.0000, -82.7485);
        assert!(
            (delta_e00(a, b) - 2.0425).abs() < 0.002,
            "{}",
            delta_e00(a, b)
        );
    }

    #[test]
    fn gamut_clipping_returns_displayable_colors() {
        // Ask for an impossible chroma; expect a reduced but in-gamut result.
        let (rgb, c) = lch_to_srgb8_gamut_clipped(32.0, 150.0, 205.0);
        assert!(c < 150.0);
        assert!(srgb8_to_linear(rgb).in_gamut());
        // And the hue must survive the clip.
        let got = srgb8_to_lab(rgb).hue_deg();
        assert!((got - 205.0).abs() < 4.0, "hue drifted to {got}");
    }
}
