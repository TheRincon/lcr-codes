//! Soft demodulation (§12.4).
//!
//! > Sample each module with an averaging kernel (§12.6), apply the §12.3 correction,
//! > then compute **log-likelihood ratios, not hard decisions**. […] Feeding soft
//! > information to the LDPC decoder is worth roughly 2 dB — a larger gain than most
//! > palette redesigns would buy. — §12.4
//!
//! Two channels, measured differently, because they *are* different (§1.4):
//!
//! * **Luma** — corrected `L*` against the two level centroids, at full module pitch.
//! * **Chroma** — corrected `a*b*` averaged over the chroma cell, against the hue
//!   centroids, Gray-labelled so a confusion between neighbouring hues costs one bit.
//!
//! The hue decision reads `a*b*` **only**. That is what §4.1's iso-luminant hue axis buys:
//! a brightness error — auto-brightness drift, PWM banding, a glare gradient the §12.3 fit
//! did not fully cancel — cannot move a module across a hue boundary, so a luma error
//! stays a luma error instead of becoming two.
//!
//! ## Noise is measured, not assumed
//!
//! An LLR is only as good as the σ it is scaled by. Both σs are measured from the
//! symbol's own known-colour modules after correction — the finders and timing patterns
//! for luma, the palette anchors and scattered references for chroma — using a MAD
//! estimator, so one blown-out reference cannot inflate the scale and flatten every LLR
//! in the symbol. A capture with clean colour produces confident LLRs and converges in a
//! few iterations; a marginal one produces cautious LLRs and lets the LDPC decoder work.

use crate::illum::IlluminationModel;
use crate::image::ImageView;
use crate::math::{mad_sigma, median};
use crate::sample::{sample_chroma_cell, sample_module};
use crate::transform::Transform;
use alloc::vec;
use alloc::vec::Vec;
use lc_core::border;
use lc_core::color::{self, Lab, Linear};
use lc_core::grid::ModuleRole;
use lc_core::layout::Layout;
use lc_core::mask;
use lc_core::modulate::chroma_bit_llrs;
use lc_core::palette::Palette;

/// Largest LLR magnitude handed to the LDPC decoder.
///
/// Min-sum propagates the smallest incoming magnitude, so one over-confident bit does not
/// wreck a codeword the way it would in sum-product — but an unbounded LLR from a module
/// that happens to sit exactly on a centroid still makes the decoder unable to revise a
/// wrong decision. Clamping keeps every bit revisable.
pub const LLR_CLAMP: f32 = 20.0;

/// Floor on measured noise, in `L*` units. Prevents a synthetic capture with literally
/// zero noise from producing infinite LLRs.
const MIN_SIGMA_L: f32 = 0.75;
/// Floor on measured chroma noise, in `a*b*` units.
const MIN_SIGMA_AB: f32 = 0.75;

/// How far a reference's *nominal* `L*` may sit from its level centroid before it is
/// excluded from the luma statistics.
///
/// §4.2 caps intra-level spread at 12, so every genuine palette colour is within 6 of its
/// level mean and this admits all of them with room to spare. What it excludes is the
/// §6.4 border's white and black cells: they are excellent references for the
/// *illumination* fit — they pin both ends of the transfer function — but they are not
/// palette luma levels, and letting `L* = 100` vote on the "light" centroid drags it up
/// by 20 and inflates the measured noise by an order of magnitude. On a pristine render
/// that alone took the reported SNR from 27 dB to 7.
const MAX_LEVEL_DEVIATION: f32 = 12.0;

/// Measured decision statistics for one capture.
#[derive(Copy, Clone, Debug)]
pub struct ChannelStats {
    /// Observed `L*` centroid of the dark level.
    pub l0: f32,
    /// Observed `L*` centroid of the light level.
    pub l1: f32,
    /// Luma noise scale, in `L*`.
    pub sigma_l: f32,
    /// Chroma noise scale, in `a*b*`.
    pub sigma_ab: f32,
    /// Known-colour modules the estimate is based on.
    pub samples: u32,
}

impl ChannelStats {
    /// Fallback used when a symbol offers too few known-colour modules to measure from —
    /// which in practice means detection succeeded but the capture is badly degraded.
    pub fn nominal(palette: &Palette) -> ChannelStats {
        ChannelStats {
            l0: palette.level_l[0],
            l1: palette.level_l[1],
            sigma_l: 6.0,
            sigma_ab: 6.0,
            samples: 0,
        }
    }

    /// Luma separation in units of its own noise — the honest per-module SNR, and the
    /// number that drives §12.4's user guidance.
    pub fn snr_db(&self) -> f32 {
        let separation = (self.l1 - self.l0).abs() / 2.0;
        20.0 * (separation / self.sigma_l.max(1e-3)).log10()
    }

    /// Log-likelihood ratio of one luma sample. Positive means bit 0, which is `L0`
    /// (dark) — see the bit-to-level convention in [`lc_core::modulate`].
    #[inline]
    pub fn luma_llr(&self, l: f32) -> f32 {
        // ((l − l1)² − (l − l0)²) / 2σ²  =  (l1 − l0)(l0 + l1 − 2l) / 2σ²
        let v = (self.l1 - self.l0) * (self.l0 + self.l1 - 2.0 * l)
            / (2.0 * self.sigma_l * self.sigma_l);
        v.clamp(-LLR_CLAMP, LLR_CLAMP)
    }

    /// How much to believe a sample at all, in `0.0..=1.0`.
    ///
    /// Every payload module renders as *exactly one palette colour*, so a corrected
    /// sample that is far from **every** palette entry did not come from a module the
    /// model understands — it came from a specular highlight, a finger, or a crease. Its
    /// LLR is not merely noisy, it is confidently wrong, which is the one thing an LDPC
    /// decoder cannot absorb: soft-decision decoding tolerates uncertainty far better
    /// than it tolerates certainty about the wrong value.
    ///
    /// Attenuating those LLRs converts local damage from *errors* into something much
    /// closer to *erasures*, which is the difference between the code needing twice the
    /// redundancy and needing none. §12.3's robust refit already keeps such samples out of
    /// the illumination fit; this is the same idea applied one stage later, to the modules
    /// the fit cannot rescue.
    ///
    /// Distance is measured in units of the channel's own measured noise, so this
    /// tightens automatically on a clean capture and loosens on a marginal one.
    pub fn reliability(&self, sample: Lab, palette: &Palette) -> f32 {
        let mut best = f32::INFINITY;
        for c in &palette.lab {
            let dl = (sample.l - c.l) / self.sigma_l;
            let da = (sample.a - c.a) / self.sigma_ab;
            let db = (sample.b - c.b) / self.sigma_ab;
            best = best.min(dl * dl + da * da + db * db);
        }
        // Flat out to ~3σ, then falls away fast.
        1.0 / (1.0 + (best.sqrt() / 3.0).powi(4))
    }

    /// Log-likelihood of a chroma sample having been rendered in each hue state.
    ///
    /// Unnormalised — [`chroma_bit_llrs`] takes differences, so the missing constant
    /// cancels.
    pub fn hue_log_likelihoods(&self, sample: Lab, palette: &Palette, out: &mut Vec<f32>) {
        out.clear();
        let denom = 2.0 * self.sigma_ab * self.sigma_ab;
        for h in 0..palette.hues {
            let [a, b] = palette.hue_ab[h];
            let d2 = (sample.a - a).powi(2) + (sample.b - b).powi(2);
            out.push(-d2 / denom);
        }
    }
}

/// A known-colour module or border cell, located in module space.
///
/// Both the illumination fit (§12.3) and the noise estimate above are built from these;
/// collecting them once and sharing is what keeps the decode to a single sampling pass
/// over the fixed patterns.
pub fn collect_references(
    img: &ImageView,
    t: &Transform,
    layout: &Layout,
    palette: &Palette,
    reference_border: bool,
) -> Vec<crate::illum::Reference> {
    let mut out = Vec::new();
    let mut push = |x: f32, y: f32, observed: Linear, nominal: Linear| {
        out.push(crate::illum::Reference {
            x,
            y,
            observed,
            nominal,
        });
    };

    for y in 0..layout.side {
        for x in 0..layout.side {
            let Some(nominal) = layout.known_color(x, y, palette) else {
                continue;
            };
            // Finder cores are large uniform blocks; sampling every module of them would
            // swamp the fit with a handful of positions. Take the whole separator and
            // timing patterns, but thin the finder interiors.
            if *layout.roles.at(x, y) == ModuleRole::Finder && (x + y) % 2 == 1 {
                continue;
            }
            push(
                x as f32 + 0.5,
                y as f32 + 0.5,
                sample_module(img, t, x as i32, y as i32),
                color::srgb8_to_linear(nominal),
            );
        }
    }

    if reference_border {
        for (bx, by) in border::cells(layout.side) {
            let Some(nominal) = border::cell_color(bx, by, layout.side, palette) else {
                continue;
            };
            push(
                bx as f32 + 0.5,
                by as f32 + 0.5,
                sample_module(img, t, bx, by),
                color::srgb8_to_linear(nominal),
            );
        }
    }
    out
}

/// Measure luma centroids and both noise scales from known-colour modules.
///
/// `illum` may be [`IlluminationModel::identity`], which is what the format-region read of
/// §12.2 uses: it runs *before* the illumination fit, because §12.3 needs the profile
/// (and therefore the palette) that the format region carries.
pub fn measure(
    refs: &[crate::illum::Reference],
    illum: &IlluminationModel,
    palette: &Palette,
) -> ChannelStats {
    if refs.len() < 8 {
        return ChannelStats::nominal(palette);
    }
    let mid = (palette.level_l[0] + palette.level_l[1]) / 2.0;
    let (mut dark, mut light) = (Vec::new(), Vec::new());
    let mut ab_err = Vec::new();

    for r in refs {
        let corrected = color::linear_to_lab(illum.correct(r.x, r.y, r.observed));
        let nominal = color::linear_to_lab(r.nominal);
        let level = usize::from(nominal.l >= mid);
        if (nominal.l - palette.level_l[level]).abs() <= MAX_LEVEL_DEVIATION {
            if level == 0 {
                dark.push(corrected.l);
            } else {
                light.push(corrected.l);
            }
        }
        // Only chromatic references say anything about chroma noise: white and black
        // border cells sit at the origin of a*b* and would report a hue error that is
        // really just a lightness error.
        if nominal.chroma() > 20.0 {
            ab_err.push(corrected.ab_distance(nominal));
        }
    }
    if dark.len() < 3 || light.len() < 3 {
        return ChannelStats::nominal(palette);
    }

    // Median, not mean. A specular highlight over one corner of the symbol lifts every
    // reference beneath it, and a mean centroid moves with them — far enough, at the
    // glare levels §3 calls the dominant screen impairment, to invert the sign of the
    // luma LLR for modules that were sampled correctly elsewhere. The median is unmoved
    // until half the references are affected, which is well past the point where the
    // symbol is unreadable for other reasons.
    let l0 = median(&mut dark.clone());
    let l1 = median(&mut light.clone());
    // Spread *within* each level is the luma noise; the gap between them is signal.
    let mut spread: Vec<f32> = dark.iter().map(|v| v - l0).collect();
    spread.extend(light.iter().map(|v| v - l1));

    ChannelStats {
        l0,
        l1,
        sigma_l: mad_sigma(&spread).max(MIN_SIGMA_L),
        sigma_ab: if ab_err.is_empty() {
            MIN_SIGMA_AB
        } else {
            mad_sigma(&ab_err).max(MIN_SIGMA_AB)
        },
        samples: refs.len() as u32,
    }
}

/// Per-layer LLRs in transmission order, ready to deinterleave.
#[derive(Clone, Debug)]
pub struct Demodulation {
    /// One LLR per payload module, in §8.4 walk order.
    pub luma: Vec<f32>,
    /// `log2(k)` LLRs per chroma-carrying cell, in cell order. Empty for profile `M` and
    /// for a monochrome capture.
    pub chroma: Vec<f32>,
    /// The statistics the LLRs were scaled by.
    pub stats: ChannelStats,
}

/// Demodulate both layers (§12.4).
///
/// `with_chroma` is false for a monochrome capture or a `M`-profile symbol. The luma
/// layer is produced either way — §1.1 goal 3 requires that a monochrome sensor still
/// yields a complete monochrome code.
pub fn demodulate(
    img: &ImageView,
    t: &Transform,
    layout: &Layout,
    palette: &Palette,
    illum: &IlluminationModel,
    stats: &ChannelStats,
    with_chroma: bool,
) -> Demodulation {
    let mut luma = Vec::with_capacity(layout.payload.len());
    for &(x, y) in &layout.payload {
        let raw = sample_module(img, t, x as i32, y as i32);
        let lab = color::linear_to_lab(illum.correct(x as f32 + 0.5, y as f32 + 0.5, raw));
        let weight = if with_chroma {
            stats.reliability(lab, palette)
        } else {
            1.0
        };
        luma.push(stats.luma_llr(lab.l) * weight);
    }

    let bpc = layout.profile.chroma_bits_per_cell();
    let mut chroma = Vec::new();
    if with_chroma && bpc > 0 {
        chroma.reserve(layout.chroma_cells.len() * bpc);
        let size = layout.profile.chroma_cell();
        let mut hue_ll = Vec::with_capacity(palette.hues);
        let mut bits = Vec::with_capacity(bpc);
        for &(cx, cy) in &layout.chroma_cells {
            let raw = sample_chroma_cell(img, t, cx, cy, size);
            let centre_x = cx as f32 * size as f32 + size as f32 / 2.0;
            let centre_y = cy as f32 * size as f32 + size as f32 / 2.0;
            let lab = color::linear_to_lab(illum.correct(centre_x, centre_y, raw));
            let weight = stats.reliability(lab, palette);
            stats.hue_log_likelihoods(lab, palette, &mut hue_ll);
            chroma_bit_llrs(&hue_ll, cx, cy, palette.hues, &mut bits);
            chroma.extend(
                bits.iter()
                    .map(|v| (v * weight).clamp(-LLR_CLAMP, LLR_CLAMP)),
            );
        }
    }

    Demodulation {
        luma,
        chroma,
        stats: *stats,
    }
}

/// Remove the §8.5 luma mask from a demodulated layer.
///
/// A mask is a bit flip, so removing it from an LLR is a sign flip — no re-sampling and
/// no hard decision in between, which is the whole reason the mask is confined to the
/// luma layer.
pub fn unmask_luma(llrs: &mut [f32], layout: &Layout, mask_id: u8) {
    for (i, &(x, y)) in layout.payload.iter().enumerate() {
        if mask::luma_mask(mask_id, x, y) {
            llrs[i] = -llrs[i];
        }
    }
}

/// Sample the two format-region copies as LLRs, for the soft BCH decode of §12.2.
///
/// Runs before the illumination fit — §12.3 needs the palette, which the format region is
/// what tells you. The format region is luma-only precisely so this is possible.
pub fn format_llrs(
    img: &ImageView,
    t: &Transform,
    layout: &Layout,
    illum: &IlluminationModel,
    stats: &ChannelStats,
) -> [[f32; 30]; 2] {
    core::array::from_fn(|copy| {
        let mut out = [0f32; 30];
        for (bit, &(x, y)) in layout.format_cells[copy].iter().enumerate() {
            let raw = sample_module(img, t, x as i32, y as i32);
            let lab = color::linear_to_lab(illum.correct(x as f32 + 0.5, y as f32 + 0.5, raw));
            out[bit] = stats.luma_llr(lab.l);
        }
        out
    })
}

/// A grid of luma LLRs covering the whole symbol, for diagnostics and hard-decision
/// fallbacks.
pub fn luma_grid(
    img: &ImageView,
    t: &Transform,
    layout: &Layout,
    illum: &IlluminationModel,
    stats: &ChannelStats,
) -> Vec<f32> {
    let n = layout.side as usize;
    let mut out = vec![0f32; n * n];
    for y in 0..layout.side {
        for x in 0..layout.side {
            let raw = sample_module(img, t, x as i32, y as i32);
            let lab = color::linear_to_lab(illum.correct(x as f32 + 0.5, y as f32 + 0.5, raw));
            out[y as usize * n + x as usize] = stats.luma_llr(lab.l);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lc_core::Profile;

    fn palette() -> Palette {
        Palette::for_profile(Profile::C1)
    }

    fn stats() -> ChannelStats {
        ChannelStats {
            l0: 32.0,
            l1: 78.0,
            sigma_l: 4.0,
            sigma_ab: 5.0,
            samples: 100,
        }
    }

    #[test]
    fn luma_llr_sign_follows_the_bit_convention() {
        let s = stats();
        // Bit 0 is L0 (dark): a dark sample must give a positive LLR.
        assert!(s.luma_llr(30.0) > 0.0);
        assert!(s.luma_llr(80.0) < 0.0);
        // Exactly between the centroids is maximally ambiguous.
        assert!(s.luma_llr(55.0).abs() < 1e-4);
    }

    #[test]
    fn luma_llr_is_monotone_and_clamped() {
        let s = stats();
        let mut prev = f32::INFINITY;
        for l in 0..=100 {
            let v = s.luma_llr(l as f32);
            assert!(v <= prev + 1e-6, "not monotone at L*={l}");
            assert!(v.abs() <= LLR_CLAMP + 1e-6);
            prev = v;
        }
        assert_eq!(s.luma_llr(-500.0), LLR_CLAMP);
        assert_eq!(s.luma_llr(500.0), -LLR_CLAMP);
    }

    #[test]
    fn noisier_captures_produce_less_confident_llrs() {
        let clean = ChannelStats {
            sigma_l: 2.0,
            ..stats()
        };
        let noisy = ChannelStats {
            sigma_l: 12.0,
            ..stats()
        };
        assert!(clean.luma_llr(32.0).abs() > noisy.luma_llr(32.0).abs());
    }

    #[test]
    fn snr_tracks_separation_over_noise() {
        let a = ChannelStats {
            sigma_l: 4.0,
            ..stats()
        };
        let b = ChannelStats {
            sigma_l: 16.0,
            ..stats()
        };
        assert!(a.snr_db() > b.snr_db());
        // 46 L* of separation against σ=4 is 23/4 ≈ 15 dB.
        assert!((a.snr_db() - 15.2).abs() < 0.5, "{}", a.snr_db());
    }

    #[test]
    fn hue_likelihood_peaks_at_the_right_centroid() {
        let p = palette();
        let s = stats();
        let mut out = Vec::new();
        for h in 0..p.hues {
            let [a, b] = p.hue_ab[h];
            s.hue_log_likelihoods(Lab::new(50.0, a, b), &p, &mut out);
            let best = out
                .iter()
                .enumerate()
                .max_by(|x, y| x.1.partial_cmp(y.1).unwrap())
                .map(|(i, _)| i)
                .unwrap();
            assert_eq!(best, h, "hue {h} likelihoods {out:?}");
        }
    }

    /// The §4.1 property, tested directly: a large lightness error must not move the hue
    /// decision at all, because the decision never looks at `L*`.
    #[test]
    fn hue_decision_is_independent_of_lightness() {
        let p = palette();
        let s = stats();
        let [a, b] = p.hue_ab[1];
        let mut dim = Vec::new();
        let mut bright = Vec::new();
        s.hue_log_likelihoods(Lab::new(5.0, a, b), &p, &mut dim);
        s.hue_log_likelihoods(Lab::new(95.0, a, b), &p, &mut bright);
        assert_eq!(dim, bright);
    }

    #[test]
    fn unmasking_is_a_sign_flip_and_an_involution() {
        let layout = Layout::new(lc_core::Version::new(3).unwrap(), Profile::C1);
        let original: Vec<f32> = (0..layout.payload.len())
            .map(|i| (i % 7) as f32 - 3.0)
            .collect();
        let mut llrs = original.clone();
        unmask_luma(&mut llrs, &layout, 5);
        assert_ne!(llrs, original);
        for (a, b) in llrs.iter().zip(&original) {
            assert_eq!(a.abs(), b.abs(), "unmasking changed a magnitude");
        }
        unmask_luma(&mut llrs, &layout, 5);
        assert_eq!(llrs, original);
    }

    /// The §6.4 border's white and black cells must not vote on the luma centroids.
    #[test]
    fn border_white_and_black_are_excluded_from_luma_statistics() {
        use crate::illum::Reference;
        let p = palette();
        let identity = IlluminationModel::identity(29);
        let at = |i: usize, c: lc_core::color::Srgb8| Reference {
            x: i as f32,
            y: 0.0,
            observed: color::srgb8_to_linear(c),
            nominal: color::srgb8_to_linear(c),
        };

        // Six palette references, three of each level.
        let mut refs: Vec<Reference> = (0..12).map(|i| at(i, p.colors[i % 4])).collect();
        let clean = measure(&refs, &identity, &p);
        assert!((clean.l0 - p.level_l[0]).abs() < 1.0, "{}", clean.l0);
        assert!((clean.l1 - p.level_l[1]).abs() < 1.0, "{}", clean.l1);

        // Add border white and black; the statistics must not move.
        refs.push(at(20, lc_core::palette::BORDER_WHITE));
        refs.push(at(21, lc_core::palette::BORDER_BLACK));
        let with_border = measure(&refs, &identity, &p);
        assert!(
            (with_border.l0 - clean.l0).abs() < 0.5,
            "{} vs {}",
            with_border.l0,
            clean.l0
        );
        assert!(
            (with_border.l1 - clean.l1).abs() < 0.5,
            "{} vs {}",
            with_border.l1,
            clean.l1
        );
        assert!(
            with_border.snr_db() > 20.0,
            "SNR collapsed to {}",
            with_border.snr_db()
        );
    }

    /// A glare patch over part of the symbol must not move the luma centroids.
    #[test]
    fn luma_centroids_resist_a_glare_patch() {
        use crate::illum::Reference;
        let p = palette();
        let identity = IlluminationModel::identity(29);
        let make = |i: usize, observed: lc_core::color::Linear, nominal_idx: usize| Reference {
            x: i as f32,
            y: 0.0,
            observed,
            nominal: p.linear[nominal_idx],
        };
        // Thirty clean references, then ten blown to white — a third of the symbol.
        let mut refs: Vec<Reference> = (0..30).map(|i| make(i, p.linear[i % 4], i % 4)).collect();
        let clean = measure(&refs, &identity, &p);
        for i in 0..10 {
            refs.push(make(
                30 + i,
                lc_core::color::Linear::new(1.0, 1.0, 1.0),
                i % 2,
            ));
        }
        let glared = measure(&refs, &identity, &p);
        assert!(
            (glared.l0 - clean.l0).abs() < 1.0,
            "{} vs {}",
            glared.l0,
            clean.l0
        );
        assert!(
            (glared.l1 - clean.l1).abs() < 1.0,
            "{} vs {}",
            glared.l1,
            clean.l1
        );
    }

    #[test]
    fn reliability_trusts_palette_colours_and_doubts_glare() {
        let p = palette();
        let s = stats();
        // An exact palette colour is fully trusted.
        for c in &p.lab {
            assert!(s.reliability(*c, &p) > 0.95, "{c:?} distrusted");
        }
        // A blown-out neutral highlight matches nothing.
        assert!(s.reliability(Lab::new(100.0, 0.0, 0.0), &p) < 0.15);
        // Halfway between two palette entries is doubted but not discarded.
        let mid = Lab::new(
            (p.lab[0].l + p.lab[2].l) / 2.0,
            (p.lab[0].a + p.lab[2].a) / 2.0,
            (p.lab[0].b + p.lab[2].b) / 2.0,
        );
        let r = s.reliability(mid, &p);
        assert!((0.0..0.9).contains(&r), "midpoint reliability {r}");
    }

    #[test]
    fn reliability_scales_with_measured_noise() {
        let p = palette();
        let off = Lab::new(p.lab[0].l + 12.0, p.lab[0].a, p.lab[0].b);
        let clean = ChannelStats {
            sigma_l: 2.0,
            sigma_ab: 2.0,
            ..stats()
        };
        let noisy = ChannelStats {
            sigma_l: 10.0,
            sigma_ab: 10.0,
            ..stats()
        };
        // The same deviation is damning on a clean capture and unremarkable on a noisy one.
        assert!(clean.reliability(off, &p) < noisy.reliability(off, &p));
    }

    #[test]
    fn nominal_stats_are_used_when_references_are_scarce() {
        let p = palette();
        let s = measure(&[], &IlluminationModel::identity(29), &p);
        assert_eq!(s.samples, 0);
        assert_eq!(s.l0, p.level_l[0]);
        assert_eq!(s.l1, p.level_l[1]);
    }
}
