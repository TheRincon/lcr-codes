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
//!
//! ## Luma is read twice: globally, and against its own neighbourhood (D22)
//!
//! The global read above is only as good as the §12.3 surface, and a specular highlight
//! is sharper than any low-order surface can follow. Inside its halo the corrected sample
//! matches no palette colour, [`ChannelStats::reliability`] rightly erases it, and under a
//! large highlight a third of the luma layer goes that way — more than the code can fill.
//!
//! But glare is *additive* in linear light, so it shifts both levels by the same amount
//! and leaves the gap between them alone. [`add_local_luma`] fits that directly: around every
//! module, an additive plane plus a level step over its 7×7 neighbourhood of payload
//! modules, and an LLR from where the module sits against the local step. The two reads
//! are added. Measured in D22: luma convergence under a 0.6-strength highlight across half
//! the frame goes from 0/16 to 16/16, on `C1` and `C3` alike.

use crate::illum::IlluminationModel;
use crate::image::ImageView;
use crate::math::{mad_sigma, median, solve};
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

/// Fewest chromatic references a luma level needs before its own chroma noise scale is
/// measured, rather than the pooled one used (D27).
const MIN_LEVEL_REFS: usize = 8;

/// Half-width, in modules, of the neighbourhood [`local_luma`] fits over.
///
/// 7×7 is ~49 samples for a four-parameter fit: enough that both levels are well
/// represented after masking, small enough that a highlight's gradient across it is close
/// to planar.
const LOCAL_RADIUS: i32 = 3;
/// Fewest payload neighbours a local fit is attempted from; below this the module keeps
/// only its global LLR.
const LOCAL_MIN: usize = 12;
/// Largest magnitude a local LLR may contribute. Below [`LLR_CLAMP`], so the global read
/// stays the stronger voice wherever it is confident.
const LOCAL_CLAMP: f32 = 8.0;
/// Global LLR magnitude at and above which the local read is not computed.
///
/// The local fit is ~2 µs a module, a third of a v32 decode if run everywhere, and it can
/// only matter where the global read is unsure: a module the global read already holds at
/// 12 or more is one [`LOCAL_CLAMP`] could not overturn. On a capture that decodes anyway
/// almost every module is there, so the cost follows the damage.
const LOCAL_SKIP: f32 = 12.0;
/// Most refit passes [`local_fit`] makes before settling for the classification it has.
const LOCAL_PASSES: usize = 8;

/// Measured decision statistics for one capture.
#[derive(Copy, Clone, Debug)]
pub struct ChannelStats {
    /// Observed `L*` centroid of the dark level.
    pub l0: f32,
    /// Observed `L*` centroid of the light level.
    pub l1: f32,
    /// Luma noise scale, in `L*`.
    pub sigma_l: f32,
    /// Chroma noise scale, in `a*b*`, pooled over both levels. What the chroma SNR reports.
    pub sigma_ab: f32,
    /// Chroma noise scale at each luma level, dark then light; `0.0` where not measured, in
    /// which case [`ChannelStats::sigma_ab`] stands in (D27).
    ///
    /// A dark colour has much less chroma *signal* in linear light than a light one at the
    /// same `C*`, so the same sensor noise scatters its `a*b*` further. One pooled scale
    /// overstates confidence in dark hues and understates it in light ones; the decoder
    /// should know which modules it can believe.
    pub sigma_ab_level: [f32; 2],
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
            sigma_ab_level: [0.0; 2],
            samples: 0,
        }
    }

    /// Chroma noise scale for a sample at lightness `l`: the two levels' measured scales,
    /// interpolated by where `l` sits between the level centroids. Interpolated rather than
    /// switched, so a `C2` chroma cell — four modules averaged, of either level — gets a
    /// scale between the two instead of an arbitrary one.
    pub fn sigma_ab_at(&self, l: f32) -> f32 {
        let [d, li] = self.sigma_ab_level;
        if d <= 0.0 || li <= 0.0 {
            return self.sigma_ab;
        }
        let t = ((l - self.l0) / (self.l1 - self.l0).max(1e-3)).clamp(0.0, 1.0);
        d + (li - d) * t
    }

    /// Luma separation in units of its own noise — the honest per-module SNR, and the
    /// number that drives §12.4's user guidance.
    pub fn snr_db(&self) -> f32 {
        let separation = (self.l1 - self.l0).abs() / 2.0;
        20.0 * (separation / self.sigma_l.max(1e-3)).log10()
    }

    /// Chroma separation in units of its own noise — the chroma-layer counterpart of
    /// [`ChannelStats::snr_db`], and the number §8.2's unequal protection is a bet about.
    ///
    /// Both are `20·log10(amplitude / sigma)`, where amplitude is *half* the distance
    /// between the two nearest decision points. That is the same quantity as the SNR axis
    /// in `docs/ldpc.md`, which sends BPSK at ±1 through noise of standard deviation
    /// `sigma`, so a layer's SNR here can be read directly against the rate thresholds
    /// tabulated there — which is the whole point of measuring it.
    ///
    /// The separation is the **minimum** distance between hue centroids rather than the
    /// mean. Errors go to the nearest neighbour, so the closest pair sets the error rate;
    /// with `k = 2` the two coincide, but on `C3`'s four uneven hues (D2) they do not.
    ///
    /// Returns `f32::NEG_INFINITY` for profile `M`, which has no chroma layer to measure.
    pub fn chroma_snr_db(&self, palette: &Palette) -> f32 {
        if palette.hues < 2 {
            return f32::NEG_INFINITY;
        }
        let mut min_d = f32::INFINITY;
        for i in 0..palette.hues {
            for j in (i + 1)..palette.hues {
                let ([a1, b1], [a2, b2]) = (palette.hue_ab[i], palette.hue_ab[j]);
                min_d = min_d.min(((a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt());
            }
        }
        20.0 * ((min_d / 2.0) / self.sigma_ab.max(1e-3)).log10()
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
            // The scale for the palette entry being tested: a sample is unlikely under a dark
            // colour's noise, not its own lightness's.
            let s_ab = self.sigma_ab_at(c.l);
            let dl = (sample.l - c.l) / self.sigma_l;
            let da = (sample.a - c.a) / s_ab;
            let db = (sample.b - c.b) / s_ab;
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
        let s_ab = self.sigma_ab_at(sample.l);
        let denom = 2.0 * s_ab * s_ab;
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
    ring: Option<border::RingMode>,
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

    // `None` is a symbol without the §6.4 ring. With one, the mode says what its colour
    // choices carry — and so what colour every ring cell should be (D17).
    if let Some(mode) = ring {
        for (bx, by) in border::cells(layout.side) {
            let Some(nominal) = border::cell_color_in(bx, by, layout.side, palette, mode) else {
                continue;
            };
            // A data position's inner cell carries payload (D19), so its colour is not
            // known a priori. Reference positions stay 2-deep and still count twice.
            if layout.ring_cells > 0
                && let Some(p) = border::position(bx, by, layout.side)
                && !border::is_two_deep(p)
                && (bx, by) != border::outer_cell(p, layout.side)
            {
                continue;
            }
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
    let mut ab_level: [Vec<f32>; 2] = [Vec::new(), Vec::new()];

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
            let e = corrected.ab_distance(nominal);
            ab_err.push(e);
            ab_level[level].push(e);
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
        // Per level only where a level has enough chromatic references to measure from;
        // otherwise 0, and the pooled scale stands in.
        sigma_ab_level: ab_level.map(|e| {
            if e.len() >= MIN_LEVEL_REFS {
                mad_sigma(&e).max(MIN_SIGMA_AB)
            } else {
                0.0
            }
        }),
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
    /// §12.3-corrected colour of every luma position — grid payload, then ring payload
    /// cells — as sampled for the luma read. What D24's equaliser reads, so a retry or an
    /// accumulating preview never samples the frame twice (D25). Empty when not kept.
    pub colours: Vec<Linear>,
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
    // Every luma module's position and corrected linear luminance, in LLR order, for the
    // local read (D22).
    let mut at = Vec::with_capacity(layout.payload.len());
    for &(x, y) in &layout.payload {
        let raw = sample_module(img, t, x as i32, y as i32);
        let corrected = illum.correct(x as f32 + 0.5, y as f32 + 0.5, raw);
        at.push((x as i32, y as i32, corrected));
        let lab = color::linear_to_lab(corrected);
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
            chroma_bit_llrs(&hue_ll, Some((cx, cy)), palette.hues, &mut bits);
            chroma.extend(
                bits.iter()
                    .map(|v| (v * weight).clamp(-LLR_CLAMP, LLR_CLAMP)),
            );
        }
    }

    // The ring's payload cells (D19): ordinary payload modules, sampled through the same
    // kernel and appended to the tail of each layer, exactly where `modulate` put them.
    if layout.ring_cells > 0 {
        let ring_chroma = with_chroma && bpc > 0 && layout.profile.chroma_cell() == 1;
        let mut hue_ll = Vec::with_capacity(palette.hues);
        let mut bits = Vec::with_capacity(bpc);
        for (x, y) in border::payload_cells(layout.side) {
            let raw = sample_module(img, t, x, y);
            let corrected = illum.correct(x as f32 + 0.5, y as f32 + 0.5, raw);
            at.push((x, y, corrected));
            let lab = color::linear_to_lab(corrected);
            let weight = if with_chroma {
                stats.reliability(lab, palette)
            } else {
                1.0
            };
            luma.push(stats.luma_llr(lab.l) * weight);
            if ring_chroma {
                stats.hue_log_likelihoods(lab, palette, &mut hue_ll);
                chroma_bit_llrs(&hue_ll, None, palette.hues, &mut bits);
                chroma.extend(
                    bits.iter()
                        .map(|v| (v * weight).clamp(-LLR_CLAMP, LLR_CLAMP)),
                );
            }
        }
    }

    // D22: add the local read. Two observations of one bit, so the LLRs add.
    add_local_luma(&mut luma, &at, layout.side, palette, stats, with_chroma);

    Demodulation {
        luma,
        chroma,
        stats: *stats,
        colours: at.into_iter().map(|(_, _, c)| c).collect(),
    }
}

/// Luma LLRs read against each module's own neighbourhood rather than the global fit (D22).
///
/// `luma` holds the global LLRs and `at` the matching `(x, y, colour)` per luma module —
/// module coordinates, which may be negative for ring cells, and the §12.3-corrected linear
/// colour. Each local LLR is added to its global one in place, positive meaning dark as
/// [`ChannelStats::luma_llr`], wherever the global one is below [`LOCAL_SKIP`].
///
/// Around each module the model is
///
/// ```text
/// Y  =  p0 + p1·dx + p2·dy  +  p3·bit        bit 1 = light level
/// ```
///
/// fitted by least squares over its payload neighbours' luminance, re-classifying the
/// neighbours against the fit a few times. The plane is whatever additive light the global
/// surface missed — a highlight's halo, mostly — and `p3` is the level gap, which additive
/// light does not change. A flat two-level split was tried first and is not enough: across
/// a steep halo a dark module on the bright side outshines a light one on the dim side.
///
/// **Luminance alone cannot tell glare from an occluder**, so the fit is not trusted on its
/// own. A finger is dark, and would read as a confident run of dark modules — exactly the
/// confidently-wrong input [`ChannelStats::reliability`] exists to keep from the decoder.
/// So the fit's additive offset is subtracted from the module's full colour, and the
/// de-glared colour must look like a palette entry to the same [`ChannelStats::reliability`]
/// test the global read uses. A glared module de-glares back to itself; a finger, a crease
/// or a grey patch does not. `chroma` false — a monochrome capture or profile `M` — skips
/// the test, as the global read does, because there is no colour to check against.
///
/// **Clipped samples stay in the fit.** Clipping looked like an erasure and measured as
/// evidence: a light module clipped to white is still brighter than a dark one that did
/// not clip, so excluding clipped samples lost decodes it should have kept (D22). Where
/// *both* levels clip, the fitted step collapses and the `(p3 / gap)²` factor takes the LLR
/// to zero on its own.
pub fn add_local_luma(
    luma: &mut [f32],
    at: &[(i32, i32, Linear)],
    side: u16,
    palette: &Palette,
    stats: &ChannelStats,
    chroma: bool,
) {
    let level_y = |l: f32| color::lab_to_linear(Lab::new(l, 0.0, 0.0)).luminance();
    let dark_y = level_y(palette.level_l[0]);
    let gap = level_y(palette.level_l[1]) - dark_y;

    // Dense lookup over the grid plus the ring's band, NaN where nothing is.
    let m = LOCAL_RADIUS + border::GAP + 2;
    let dim = side as i32 + 2 * m;
    let mut grid = vec![f32::NAN; (dim * dim) as usize];
    let index = |x: i32, y: i32| ((y + m) * dim + (x + m)) as usize;
    for &(x, y, c) in at {
        if (-m..side as i32 + m).contains(&x) && (-m..side as i32 + m).contains(&y) {
            grid[index(x, y)] = c.luminance();
        }
    }

    let mut near: Vec<(f32, f32, f32)> = Vec::with_capacity(49);
    let mut bits: Vec<bool> = Vec::with_capacity(49);
    let mut dev: Vec<f32> = Vec::with_capacity(49);
    for (llr, &(x, y, c)) in luma.iter_mut().zip(at) {
        if llr.abs() >= LOCAL_SKIP {
            continue;
        }
        near.clear();
        for dy in -LOCAL_RADIUS..=LOCAL_RADIUS {
            for dx in -LOCAL_RADIUS..=LOCAL_RADIUS {
                let (u, v) = (x + dx, y + dy);
                if !(-m..dim - m).contains(&u) || !(-m..dim - m).contains(&v) {
                    continue;
                }
                let s = grid[index(u, v)];
                if !s.is_nan() {
                    near.push((dx as f32, dy as f32, s));
                }
            }
        }
        if near.len() < LOCAL_MIN {
            continue;
        }
        let Some((mut local, base, step)) =
            local_fit(&near, &mut bits, &mut dev, c.luminance(), gap)
        else {
            continue;
        };
        if chroma {
            // Undo the local fit — offset, then gain — and ask whether what is left is a
            // palette colour. The offset is treated as neutral light, which is what a
            // reflection off a screen mostly is.
            let gain = (step / gap).clamp(0.25, 4.0);
            let offset = base - gain * dark_y;
            let deglared = Linear::new(
                (c.r - offset) / gain,
                (c.g - offset) / gain,
                (c.b - offset) / gain,
            );
            local *= stats.reliability(color::linear_to_lab(deglared), palette);
        }
        *llr = (*llr + local).clamp(-LLR_CLAMP, LLR_CLAMP);
    }
}

/// One neighbourhood's fit and the LLR it gives the module at the origin.
fn local_fit(
    near: &[(f32, f32, f32)],
    bits: &mut Vec<bool>,
    dev: &mut Vec<f32>,
    y0: f32,
    gap: f32,
) -> Option<(f32, f32, f32)> {
    // Seed the classes with one Ridler–Calvard split of the raw values.
    let mean = near.iter().map(|n| n.2).sum::<f32>() / near.len() as f32;
    let (mut lo, mut hi, mut nl, mut nh) = (0.0, 0.0, 0usize, 0usize);
    for n in near {
        if n.2 < mean {
            lo += n.2;
            nl += 1;
        } else {
            hi += n.2;
            nh += 1;
        }
    }
    if nl == 0 || nh == 0 {
        return None;
    }
    let split = (lo / nl as f32 + hi / nh as f32) / 2.0;
    bits.clear();
    bits.extend(near.iter().map(|n| n.2 > split));

    let mut p = [0f64; 4];
    // Refit until the classification stops moving. Usually two passes; a steep halo with
    // an uneven level mix in the window can need more, and stopping early leaves the plane
    // biased by the misclassified neighbours — which the de-glared colour check then
    // (rightly) refuses.
    for _ in 0..LOCAL_PASSES {
        let mut ata = [0f64; 16];
        let mut atb = [0f64; 4];
        for (n, &bit) in near.iter().zip(bits.iter()) {
            let row = [1.0, n.0 as f64, n.1 as f64, f64::from(u8::from(bit))];
            for i in 0..4 {
                atb[i] += row[i] * n.2 as f64;
                for j in 0..4 {
                    ata[i * 4 + j] += row[i] * row[j];
                }
            }
        }
        let sol = solve(&mut ata, &mut atb, 4)?;
        p = [sol[0], sol[1], sol[2], sol[3]];
        if p[3] <= 0.0 {
            return None;
        }
        let mut moved = false;
        for (n, bit) in near.iter().zip(bits.iter_mut()) {
            let base = p[0] + p[1] * n.0 as f64 + p[2] * n.1 as f64;
            let now = n.2 as f64 - base > p[3] / 2.0;
            moved |= now != *bit;
            *bit = now;
        }
        if !moved {
            break;
        }
    }

    let (base, step) = (p[0] as f32, p[3] as f32);
    dev.clear();
    dev.extend(near.iter().zip(bits.iter()).map(|(n, &bit)| {
        let fit = base + p[1] as f32 * n.0 + p[2] as f32 * n.1 + if bit { step } else { 0.0 };
        n.2 - fit
    }));
    let sigma = mad_sigma(dev).max(0.02 * gap);
    // A step much smaller than nominal means the levels have merged — both clipped, or no
    // symbol here at all — and the read should fade out rather than amplify noise.
    let c = (step / gap).clamp(0.0, 1.0);
    let mid = base + step / 2.0;
    let llr = (step * (mid - y0) / (sigma * sigma) * c * c).clamp(-LOCAL_CLAMP, LOCAL_CLAMP);
    Some((llr, base, step))
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

/// The format words worth BCH-decoding from two copies: each alone, and their mean (D27).
///
/// Averaging the copies' LLRs before decoding rather than voting afterwards is worth up to
/// 3 dB when both are noisy — two independent observations of one word. Each copy is still
/// tried alone, because a copy that sits under a highlight is better dropped than averaged
/// in; the caller keeps whichever decodes with the largest margin. Averaged rather than
/// summed so the three margins are on one scale.
pub fn format_candidates<const N: usize>(copies: &[[f32; N]; 2]) -> [[f32; N]; 3] {
    let mean = core::array::from_fn(|i| (copies[0][i] + copies[1][i]) / 2.0);
    [copies[0], copies[1], mean]
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
            sigma_ab_level: [0.0; 2],
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

    /// Both SNRs must be on one scale, or comparing the layers is meaningless — and
    /// comparing them is the entire purpose of §8.2's unequal protection.
    #[test]
    fn both_layer_snrs_share_a_scale() {
        let p = Palette::for_profile(Profile::C1);
        let st = ChannelStats {
            sigma_l: 4.0,
            sigma_ab: 4.0,
            ..stats()
        };
        // Luma: 46 L* apart, so amplitude 23 against sigma 4 is ~15 dB.
        assert!((st.snr_db() - 15.2).abs() < 0.5, "{}", st.snr_db());
        // Chroma: the C4 hue centroids are ~110 apart in a*b*, so amplitude 55 against
        // the same sigma is ~22.8 dB — the palette hands chroma more separation than
        // luma, which is why the measured gap runs the opposite way to §8.2's premise.
        let c = st.chroma_snr_db(&p);
        assert!((c - 22.8).abs() < 0.6, "{c}");
        assert!(c > st.snr_db());

        // Halving the noise is +6 dB on either layer.
        let quiet = ChannelStats {
            sigma_ab: 2.0,
            ..st
        };
        assert!((quiet.chroma_snr_db(&p) - c - 6.02).abs() < 0.05);
    }

    /// Profile `M` has no chroma layer, so there is nothing to report.
    #[test]
    fn mono_has_no_chroma_snr() {
        let st = stats();
        assert_eq!(
            st.chroma_snr_db(&Palette::for_profile(Profile::M)),
            f32::NEG_INFINITY
        );
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

    /// Ground-truth levels (true = light), module positions and colours, global LLRs.
    type Field = (Vec<bool>, Vec<(i32, i32, Linear)>, Vec<f32>);

    /// An 11×11 field of `C1` payload modules in a fixed pseudo-random level pattern, with
    /// `f(x, y, colour)` applied, and global LLRs of zero — everything erased.
    fn local_field(f: impl Fn(i32, i32, Linear) -> Linear) -> Field {
        let p = palette();
        let (mut light, mut at) = (Vec::new(), Vec::new());
        for y in 0..11i32 {
            for x in 0..11i32 {
                let bit = (x * 7 + y * 13 + x * y) % 3 == 0;
                let c = p.linear[usize::from(bit) * p.hues + (x + y) as usize % p.hues];
                light.push(bit);
                at.push((x, y, f(x, y, c)));
            }
        }
        let llrs = vec![0.0; at.len()];
        (light, at, llrs)
    }

    /// D22's premise: additive light shifts both levels together, so the local read recovers
    /// every module the global read has given up on — here a steep 0.6-per-symbol ramp of
    /// white, enough to put dark modules on one side above light ones on the other.
    #[test]
    fn local_read_sees_through_additive_glare() {
        let ramp = |x: i32, _y: i32, c: Linear| {
            let g = 0.06 * x as f32;
            Linear::new(c.r + g, c.g + g, c.b + g)
        };
        let (light, at, mut llrs) = local_field(ramp);
        add_local_luma(&mut llrs, &at, 11, &palette(), &stats(), true);
        for (i, (&bit, &v)) in light.iter().zip(&llrs).enumerate() {
            // Positive means dark.
            assert!(
                (v > 1.0 && !bit) || (v < -1.0 && bit),
                "module {i}: light={bit} llr={v}"
            );
        }
    }

    /// Luminance alone would read a dark finger as a confident run of dark modules. The
    /// de-glared colour check must refuse it: a near-black neutral is no palette colour.
    #[test]
    fn local_read_does_not_vouch_for_a_dark_occluder() {
        let inside = |x: i32, y: i32| (3..8).contains(&x) && (3..8).contains(&y);
        let finger = |x: i32, y: i32, c: Linear| {
            if inside(x, y) {
                Linear::new(0.06, 0.045, 0.04)
            } else {
                c
            }
        };
        let (_, at, mut llrs) = local_field(finger);
        add_local_luma(&mut llrs, &at, 11, &palette(), &stats(), true);
        for (&(x, y, _), &v) in at.iter().zip(&llrs) {
            if inside(x, y) {
                assert!(v.abs() < 1.0, "occluded ({x},{y}) read with confidence {v}");
            }
        }
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
