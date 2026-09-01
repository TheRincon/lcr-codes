//! Illumination model fit (§12.3).
//!
//! Three stages, in order, and the order matters:
//!
//! 1. A **per-channel spatial surface**, bilinear below v10 and biquadratic at v10 and
//!    above, describing what the channel did to each colour *as a function of position*:
//!    a glare gradient, a shadow across half the symbol, vignetting.
//! 2. A **global 3×4 affine** mapping the de-shaded observation to nominal linear RGB.
//!    This absorbs white balance, exposure, and the display's colour mode — the
//!    cross-channel effects that are constant across the symbol.
//! 3. **Robust rejection and refit.** Samples whose residual exceeds a MAD-based
//!    threshold are dropped and both stages are refitted.
//!
//! > Step 3 matters more than it looks. One reference sitting under a specular highlight
//! > can drag a global fit badly, and on a glossy screen that is a routine occurrence
//! > rather than an edge case. — §12.3
//!
//! This is the design bet of §1.4 made concrete: colour errors are *correlated across
//! space*, so a small number of scattered known-colour references (§6.3, ~1.6% of area)
//! plus a low-order surface cancels most of the error, where per-module redundancy would
//! cost far more and buy less.
//!
//! ## Deviation from §12.3: the surface is multiplicative as well as additive
//!
//! §12.3 step 2 says to fit the surface "over the residuals", which reads as an additive
//! correction. **An additive surface cannot cancel a shading gradient**, and shading is
//! the dominant spatial impairment the step exists for. A gradient scales every channel
//! by `s(x, y)`, so the residual it leaves behind after stage 1 is
//! `(1 − s(x, y)) · nominal` — proportional to the colour of the module, and therefore
//! different for every palette entry at the same position. No single additive surface
//! fits all of them; fitting one against a mixture of palette colours produces their
//! average and cancels none of them. Measured on a 50% linear falloff, an additive-only
//! surface leaves a residual of 0.23 in linear light, which is larger than the gap
//! between palette entries.
//!
//! The model is **forward** — it describes what the channel did, and correction inverts
//! it:
//!
//! ```text
//! fit:      observed_c  ≈  gain_c(x, y) · nominal_c  +  flare_c(x, y)
//! correct:  nominal     =  affine · [ (observed_c − flare_c(x, y)) / gain_c(x, y) ]
//! ```
//!
//! Two things about that arrangement are load bearing.
//!
//! **Forward, then inverted — not fitted inverse.** A linear attenuation `s(x)` *is* a
//! bilinear surface, so a forward fit recovers it with no residual at all. Its inverse
//! `1/s(x)` is a rational function that a bilinear surface can only approximate, leaving
//! ~0.05 of linear-light error on a 50% falloff — comparable to the gap between palette
//! entries. Same basis, same cost; the difference is between cancelling a gradient and
//! merely reducing it.
//!
//! **Spatial before global.** The spatial surface is per-channel and scalar, so it cannot
//! represent channel mixing; the affine is global, so it cannot represent position. Fit
//! the affine first and it absorbs the *average* shading, leaving the surface a target
//! that depends on all three nominal channels and that a per-channel fit cannot express —
//! which is how the same components in the other order still leave 0.05. Fitting the
//! spatial part first leaves the affine a spatially uniform problem, which is exactly
//! what it can solve.
//!
//! Each stage is one linear least-squares solve. The gain half of stage 1 handles
//! shading, glare falloff and vignetting; the flare half handles veiling glare and
//! black-level lift, which genuinely are additive. Both degrade gracefully when
//! references are scarce: the flare half is only fitted when there are enough live
//! references to support it, and below that the model reduces to gain-only and then to a
//! plain global affine.

use crate::math::{least_squares, mad_sigma};
use alloc::vec;
use alloc::vec::Vec;
use lc_core::color::Linear;

/// One known-colour observation.
#[derive(Copy, Clone, Debug)]
pub struct Reference {
    /// Module-space x. May be negative — border references live outside the grid.
    pub x: f32,
    /// Module-space y.
    pub y: f32,
    /// What the camera saw, in linear light.
    pub observed: Linear,
    /// What should have been there, in linear light.
    pub nominal: Linear,
}

/// A fitted correction from observed linear RGB to nominal linear RGB.
#[derive(Clone, Debug)]
pub struct IlluminationModel {
    /// Row-major 3×4: `nominal_c = Σ affine[c][k] · [r, g, b, 1]`.
    affine: [[f64; 4]; 3],
    /// Per-channel forward gain surface coefficients, `terms` long.
    gain: [Vec<f64>; 3],
    /// Per-channel forward flare surface coefficients, `terms` long.
    flare: [Vec<f64>; 3],
    /// Surface basis size: 4 (bilinear) or 9 (biquadratic).
    terms: usize,
    /// Symbol side, for coordinate normalisation.
    side: f32,
    /// RMS residual after fitting, in linear-light units.
    pub residual_rms: f32,
    /// References discarded by stage 3.
    pub rejected: u32,
    /// References that survived.
    pub kept: u32,
}

/// Invert the forward spatial model at one position.
///
/// The gain clamp keeps a surface extrapolated well outside the reference hull — a corner
/// of a symbol whose references all sit inland — from dividing by ~0 and handing the LDPC
/// decoder an infinite LLR it would then trust completely.
fn deshade(gain: &[Vec<f64>; 3], flare: &[Vec<f64>; 3], basis: &[f64], observed: Linear) -> [f64; 3] {
    let obs = [observed.r as f64, observed.g as f64, observed.b as f64];
    core::array::from_fn(|c| {
        let mut g = 0.0;
        let mut f = 0.0;
        for (k, bk) in basis.iter().enumerate() {
            g += gain[c][k] * bk;
            f += flare[c][k] * bk;
        }
        let g = if g.abs() < 0.05 { 0.05f64.copysign(if g == 0.0 { 1.0 } else { g }) } else { g };
        (obs[c] - f) / g
    })
}

/// A surface that evaluates to 1 everywhere: the multiplicative identity.
fn unit_gain(terms: usize) -> Vec<f64> {
    let mut v = vec![0.0; terms];
    v[0] = 1.0;
    v
}

/// Version at and above which the residual surface becomes biquadratic (§12.3).
pub const BIQUADRATIC_FROM_VERSION: u8 = 10;

impl IlluminationModel {
    /// An identity model, for when there are too few references to fit anything.
    ///
    /// Returning identity rather than failing is deliberate: a monochrome capture has no
    /// usable colour references at all, and §1.1 goal 3 requires the luma layer to decode
    /// anyway.
    pub fn identity(side: u16) -> IlluminationModel {
        IlluminationModel {
            affine: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
            gain: [unit_gain(4), unit_gain(4), unit_gain(4)],
            flare: [vec![0.0; 4], vec![0.0; 4], vec![0.0; 4]],
            terms: 4,
            side: side as f32,
            residual_rms: 0.0,
            rejected: 0,
            kept: 0,
        }
    }

    /// Fit the model from known-colour references.
    pub fn fit(refs: &[Reference], side: u16, version: u8) -> IlluminationModel {
        let terms = if version >= BIQUADRATIC_FROM_VERSION { 9 } else { 4 };
        let mut model = IlluminationModel::identity(side);
        model.terms = terms;
        model.gain = [unit_gain(terms), unit_gain(terms), unit_gain(terms)];
        model.flare = [vec![0.0; terms], vec![0.0; terms], vec![0.0; terms]];

        // Stage 1 needs 4 references for a 4-parameter fit per channel; ask for a few
        // more so the solution is not purely determined by noise.
        if refs.len() < 8 {
            return model;
        }

        let mut active: Vec<bool> = vec![true; refs.len()];
        // Two passes: fit, reject, refit. A third pass buys nothing measurable.
        for pass in 0..2 {
            let Some(fitted) = model.fit_once(refs, &active, terms) else { return model };
            model.affine = fitted.0;
            model.gain = fitted.1;
            model.flare = fitted.2;

            let residuals: Vec<f32> = refs
                .iter()
                .map(|r| {
                    let c = model.correct(r.x, r.y, r.observed);
                    ((c.r - r.nominal.r).powi(2)
                        + (c.g - r.nominal.g).powi(2)
                        + (c.b - r.nominal.b).powi(2))
                    .sqrt()
                })
                .collect();

            let kept: Vec<f32> =
                residuals.iter().zip(&active).filter(|(_, a)| **a).map(|(r, _)| *r).collect();
            model.residual_rms =
                (kept.iter().map(|r| r * r).sum::<f32>() / kept.len().max(1) as f32).sqrt();

            if pass == 0 {
                let sigma = mad_sigma(&kept).max(1e-4);
                let mut rejected = 0;
                for (i, r) in residuals.iter().enumerate() {
                    // 3σ, and never reject so much that the fit becomes underdetermined.
                    if *r > 3.0 * sigma && rejected < refs.len() / 4 {
                        active[i] = false;
                        rejected += 1;
                    }
                }
                model.rejected = rejected as u32;
            }
        }
        model.kept = active.iter().filter(|a| **a).count() as u32;
        model
    }

    #[allow(clippy::type_complexity)]
    fn fit_once(
        &self,
        refs: &[Reference],
        active: &[bool],
        terms: usize,
    ) -> Option<([[f64; 4]; 3], [Vec<f64>; 3], [Vec<f64>; 3])> {
        let live: Vec<&Reference> =
            refs.iter().zip(active).filter(|(_, a)| **a).map(|(r, _)| r).collect();
        if live.len() < 8 {
            return None;
        }

        // Stage 1: forward gain and flare surfaces, per channel. The flare half is only
        // fitted when there are enough live references to support twice the basis; below
        // that it would absorb noise rather than veiling glare.
        let fit_flare = live.len() >= 6 * terms;
        let width = if fit_flare { 2 * terms } else { terms };

        let mut gain: [Vec<f64>; 3] = [unit_gain(terms), unit_gain(terms), unit_gain(terms)];
        let mut flare: [Vec<f64>; 3] = [vec![0.0; terms], vec![0.0; terms], vec![0.0; terms]];

        for c in 0..3 {
            let rows: Vec<(Vec<f64>, f64)> = live
                .iter()
                .map(|r| {
                    let observed = [r.observed.r, r.observed.g, r.observed.b][c] as f64;
                    let nominal = [r.nominal.r, r.nominal.g, r.nominal.b][c] as f64;
                    let b = self.basis(r.x, r.y, terms);
                    let mut design = Vec::with_capacity(width);
                    design.extend(b.iter().map(|v| v * nominal));
                    if fit_flare {
                        design.extend_from_slice(&b);
                    }
                    (design, observed)
                })
                .collect();
            if let Some(sol) = least_squares(&rows, width) {
                gain[c].copy_from_slice(&sol[..terms]);
                if fit_flare {
                    flare[c].copy_from_slice(&sol[terms..]);
                }
            }
        }

        // Stage 2: global affine over the de-shaded observations.
        let mut affine = [[0f64; 4]; 3];
        for (c, row) in affine.iter_mut().enumerate() {
            let rows: Vec<(Vec<f64>, f64)> = live
                .iter()
                .map(|r| {
                    let d = deshade(&gain, &flare, &self.basis(r.x, r.y, terms), r.observed);
                    let target = [r.nominal.r, r.nominal.g, r.nominal.b][c] as f64;
                    (vec![d[0], d[1], d[2], 1.0], target)
                })
                .collect();
            let sol = least_squares(&rows, 4)?;
            row.copy_from_slice(&sol);
        }
        Some((affine, gain, flare))
    }

    /// Polynomial basis at a module-space position, on normalised coordinates.
    ///
    /// Normalising to roughly `[-1, 1]` keeps the normal equations well conditioned; a
    /// biquadratic fit on raw coordinates up to 145 would have a condition number in the
    /// billions.
    fn basis(&self, x: f32, y: f32, terms: usize) -> Vec<f64> {
        let u = (2.0 * x as f64 / self.side as f64) - 1.0;
        let v = (2.0 * y as f64 / self.side as f64) - 1.0;
        if terms == 4 {
            vec![1.0, u, v, u * v]
        } else {
            vec![1.0, u, v, u * v, u * u, v * v, u * u * v, u * v * v, u * u * v * v]
        }
    }

    /// Apply the correction to an observed sample at a module-space position.
    pub fn correct(&self, x: f32, y: f32, observed: Linear) -> Linear {
        let b = self.basis(x, y, self.terms);
        let d = deshade(&self.gain, &self.flare, &b, observed);
        let mut out = [0f64; 3];
        for (c, slot) in out.iter_mut().enumerate() {
            let a = &self.affine[c];
            *slot = a[0] * d[0] + a[1] * d[1] + a[2] * d[2] + a[3];
        }
        Linear::new(out[0] as f32, out[1] as f32, out[2] as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lc_core::Profile;
    use lc_core::palette::Palette;

    /// Build a reference set on a grid, distorted by `f`.
    fn refs(side: u16, f: impl Fn(f32, f32, Linear) -> Linear) -> Vec<Reference> {
        let palette = Palette::for_profile(Profile::C1);
        let mut out = Vec::new();
        for i in 0..8u16 {
            for j in 0..8u16 {
                let (x, y) = (i as f32 * side as f32 / 8.0, j as f32 * side as f32 / 8.0);
                let nominal = palette.linear[((i + j) % 4) as usize];
                out.push(Reference { x, y, observed: f(x, y, nominal), nominal });
            }
        }
        out
    }

    fn max_error(model: &IlluminationModel, refs: &[Reference]) -> f32 {
        refs.iter()
            .map(|r| {
                let c = model.correct(r.x, r.y, r.observed);
                (c.r - r.nominal.r).abs().max((c.g - r.nominal.g).abs()).max((c.b - r.nominal.b).abs())
            })
            .fold(0.0, f32::max)
    }

    #[test]
    fn identity_model_leaves_samples_alone() {
        let m = IlluminationModel::identity(29);
        let c = Linear::new(0.2, 0.5, 0.8);
        let got = m.correct(10.0, 10.0, c);
        assert!((got.r - c.r).abs() < 1e-6 && (got.b - c.b).abs() < 1e-6);
    }

    #[test]
    fn cancels_a_global_channel_gain() {
        // A warm display mode: blue attenuated, red boosted.
        let set = refs(29, |_, _, c| Linear::new(c.r * 1.15, c.g * 0.98, c.b * 0.6));
        let m = IlluminationModel::fit(&set, 29, 5);
        assert!(max_error(&m, &set) < 0.01, "residual {}", max_error(&m, &set));
    }

    /// The case that an additive-only residual surface cannot fix; see the module docs.
    #[test]
    fn cancels_a_linear_shading_gradient() {
        let set = refs(29, |x, _, c| {
            let s = 1.0 - 0.5 * (x / 29.0);
            Linear::new(c.r * s, c.g * s, c.b * s)
        });
        let m = IlluminationModel::fit(&set, 29, 5);
        assert!(max_error(&m, &set) < 0.02, "residual {}", max_error(&m, &set));
    }

    /// Shading and veiling flare together — multiplicative and additive at once, which
    /// is what a glossy screen under room lighting actually does.
    #[test]
    fn cancels_shading_and_flare_together() {
        let set = refs(97, |x, y, c| {
            let s = 1.0 - 0.4 * (x / 97.0) - 0.2 * (y / 97.0);
            let flare = 0.05 * (1.0 - y / 97.0);
            Linear::new(c.r * s + flare, c.g * s + flare, c.b * s + flare)
        });
        let m = IlluminationModel::fit(&set, 97, 20);
        assert!(max_error(&m, &set) < 0.02, "residual {}", max_error(&m, &set));
    }

    #[test]
    fn biquadratic_beats_bilinear_on_a_curved_falloff() {
        let distort = |x: f32, y: f32, c: Linear| {
            let (u, v) = (x / 97.0 - 0.5, y / 97.0 - 0.5);
            let s = 1.0 - 0.6 * (u * u + v * v);
            Linear::new(c.r * s, c.g * s, c.b * s)
        };
        let set = refs(97, distort);
        let bilinear = IlluminationModel::fit(&set, 97, 5);
        let biquadratic = IlluminationModel::fit(&set, 97, 20);
        assert!(
            max_error(&biquadratic, &set) < max_error(&bilinear, &set),
            "biquadratic {} vs bilinear {}",
            max_error(&biquadratic, &set),
            max_error(&bilinear, &set)
        );
    }

    /// The §12.3 step 3 scenario, stated plainly: one blown-out reference must not be
    /// allowed to steer the fit.
    #[test]
    fn one_specular_reference_is_rejected() {
        let mut set = refs(29, |_, _, c| Linear::new(c.r * 0.9, c.g * 0.9, c.b * 0.9));
        let victim = 27;
        set[victim].observed = Linear::new(1.0, 1.0, 1.0); // blown to white

        let m = IlluminationModel::fit(&set, 29, 5);
        assert!(m.rejected >= 1, "no reference was rejected");

        let clean: Vec<Reference> =
            set.iter().enumerate().filter(|(i, _)| *i != victim).map(|(_, r)| *r).collect();
        assert!(max_error(&m, &clean) < 0.02, "outlier dragged the fit: {}", max_error(&m, &clean));
    }

    #[test]
    fn too_few_references_falls_back_to_identity() {
        let set = refs(29, |_, _, c| c);
        let m = IlluminationModel::fit(&set[..4], 29, 5);
        assert_eq!(m.kept, 0);
        let c = Linear::new(0.3, 0.4, 0.5);
        assert!((m.correct(1.0, 1.0, c).g - c.g).abs() < 1e-6);
    }

    #[test]
    fn handles_references_outside_the_grid() {
        // Border references (§6.4) have negative coordinates; the basis must cope.
        let m = IlluminationModel::fit(&refs(29, |_, _, c| c), 29, 5);
        let c = Linear::new(0.5, 0.5, 0.5);
        let out = m.correct(-4.0, -4.0, c);
        assert!(out.r.is_finite() && out.g.is_finite() && out.b.is_finite());
    }
}
