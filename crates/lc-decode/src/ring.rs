//! Finder-less detection from the §6.4 ring (D17).
//!
//! The classic path finds a symbol by its three 1:1:3:1:1 finders. A finder-less symbol has
//! none, and everything the finders, alignment marker, timing patterns and format region
//! did is recovered from the ring instead:
//!
//! 1. **Where.** The ring is the outermost structure of a symbol, and its four corner
//!    positions are forced dark in the position sequence. So the convex hull of a symbol's
//!    ink has the ring's outer corners as its corners, and those four points fit a
//!    homography. Ink is found by thresholding greyscale against local dark and light levels
//!    and grouped into connected regions, each tried on its own — see [`InkMap`].
//! 2. **Which version, which way up, mirrored or not.** Sampling the ring's luma under each
//!    version's geometry and correlating it against that version's sequence picks all three
//!    at once: only the right version lines the samples up with its own sequence, and the
//!    best-matching cyclic shift and direction give rotation and mirror.
//! 3. **The format.** Once geometry is known, illumination is fitted from the ring's
//!    fixed-colour reference positions, and the format codeword is read softly from the
//!    colour choices at the rest — two interleaved BCH copies.
//!
//! ## What this does not do, and it matters
//!
//! A region is assumed to be one symbol. Two symbols with their quiet zones touching, or a
//! symbol overlapping printed text or another dark object, merge into one region whose hull
//! is not a ring. That fails safely — a hull that is not a ring does not correlate — but it
//! fails. Real scenes are the first thing a capture sweep should test here.

use crate::demod;
use crate::illum::{IlluminationModel, Reference};
use crate::image::ImageView;
use crate::math::solve;
use crate::sample::sample_module;
use crate::transform::Transform;
use alloc::vec::Vec;
use lc_core::border::{self, FORMAT_COPIES, FORMAT_COPY_BITS};
use lc_core::color;
use lc_core::palette::Palette;
use lc_core::{FormatInfo, LcError, Profile, Version, bch};

/// Fraction of ring positions that must match the sequence to accept a detection.
///
/// A clean render matches every position. A wrong version or a hull that is not a ring
/// lands near one half, which is what chance gives a balanced binary sequence.
const MIN_AGREEMENT: f32 = 0.85;

/// How far the best version must beat the runner-up. Guards against a geometry so degraded
/// that two versions look alike, where committing to either would be a guess.
const MIN_LEAD: f32 = 0.08;

/// Agreement accepted when the lead is at least [`STRONG_LEAD`] (D25).
const RELAXED_AGREEMENT: f32 = 0.75;

/// A lead over the runner-up version that chance does not produce (D25).
const STRONG_LEAD: f32 = 0.2;

/// Below this many pixels per module a ring position cannot be sampled at all.
const MIN_PITCH_PX: f32 = 2.0;

/// Shortest dark run, in pixels, that counts as structure rather than sensor speckle.
///
/// The hull is built from the *outermost* dark pixels, so a single speckle survives
/// denoising anywhere in the frame and becomes a hull vertex: at noise σ = 0.10 every one of
/// twelve frames put its quad on the frame corners. A ring cell is two modules deep, so its
/// dark runs are at least `2 · MIN_PITCH_PX` long and this filter never removes one.
const MIN_RUN_PX: u32 = 4;

/// Cell size, in pixels, of the grid [`InkMap`] estimates dark and light levels on.
const LEVEL_CELL: u32 = 16;

/// Radius, in level cells, of the neighbourhood a cell's dark and light levels are pooled over.
const LEVEL_RADIUS: usize = 3;

/// How many cells a threshold is carried out from where contrast was seen, into flat areas.
///
/// Covers a module wider than the pooling neighbourhood — a flat dark interior at large pitch
/// — without letting one symbol's threshold reach across an arbitrarily large blank frame.
const LEVEL_FILL: usize = 8;

/// Least dark-to-light contrast, in luma levels, for a neighbourhood to hold ink.
///
/// **Set above a light hue against white (~67 levels).** The adaptive binariser accepts a
/// 24-level range, which sensor noise alone exceeds in flat white quiet zone at σ = 0.10, so
/// every such block thresholds at its own mean and the quiet zone binarises as ~50% speckle.
/// That is what put all twelve noisy frames' hulls on the frame corners.
const MIN_INK_CONTRAST: u8 = 80;

/// Where ink starts, as a fraction of the local dark-to-light range below the light level.
///
/// **Anything clearly darker than paper is ink, light hues included.** Splitting at the
/// midpoint instead left light hues as background, so every light cluster in the payload cut
/// a symbol into pieces — nine regions for one clean v3 at 12 px/module — and two symbols in
/// one frame only ever reached the all-ink fallback hull, which spans both. The lightest
/// palette colours sit near luma 200 against white at 255; 0.2 of a ~210-level range puts the
/// cut near 213, about 3σ below noisy white at the σ = 0.10 test limit, and [`MIN_RUN_PX`]
/// then needs four such pixels in a row.
const PAPER_MARGIN: f32 = 0.2;

/// Cell size, in pixels, of the grid ink regions are labelled on.
const REGION_CELL: u32 = 8;

/// Ink pixels a region cell needs to count, so a stray pixel cannot join or start a region.
const MIN_CELL_INK: u32 = 4;

/// Region cells a region needs before its hull is worth correlating.
const MIN_REGION_CELLS: usize = 16;

/// Most regions tried per frame, largest first.
const MAX_REGIONS: usize = 4;

/// Half-width, in ring positions, of the window each position's luma threshold comes from.
const LUMA_WINDOW: usize = 16;

/// Soft-decision clamp for format-bit LLRs, so one confident misread cannot outvote a copy.
const LLR_CLAMP: f32 = 16.0;

/// A located finder-less symbol.
#[derive(Clone, Debug)]
pub struct RingFix {
    /// Version, read from which sequence the ring matched.
    pub version: Version,
    /// Module space → pixel space, with orientation and mirror resolved.
    pub transform: Transform,
    /// Fraction of ring positions agreeing with the sequence.
    pub agreement: f32,
}

/// Locate a finder-less symbol from its ring, or `None` if the frame holds no ring.
///
/// `gray` is the frame's greyscale plane, `width × height`. The ring detector reads greyscale
/// throughout, never the adaptive binary — see [`ring_lumas`] and [`MIN_INK_CONTRAST`].
pub fn detect_ring(gray: &[u8], width: u32, height: u32, max_version: Version) -> Option<RingFix> {
    trace(gray, width, height, max_version).fix
}

/// Everything [`detect_ring`] looked at, for diagnosing why it did or did not find a ring.
///
/// Detection is *built on* this rather than duplicated beside it, so a trace shows exactly
/// what detection decided from — a diagnostic that reimplemented the steps could disagree
/// with the thing it claims to explain.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct RingTrace {
    /// Ink regions found in the frame, before any size filter.
    pub regions: usize,
    /// Vertices on the convex hull of the attempt reported here.
    pub hull_len: usize,
    /// Hull quadrilateral, clockwise in image space, if one was found.
    pub quad: Option<[(f32, f32); 4]>,
    /// `(version, agreement, corner, mirrored)` for every version tried, in version order.
    pub scores: Vec<(Version, f32, usize, bool)>,
    /// The accepted detection, if the best score cleared both thresholds.
    pub fix: Option<RingFix>,
    /// The detection under the relaxed rule for a defocused ring — used only when no
    /// attempt anywhere in the frame passes the ordinary one (D25).
    pub fallback: Option<RingFix>,
}

/// Run detection and keep the working. See [`RingTrace`].
///
/// Tries each ink region on its own, largest first, and reports the first attempt that is
/// accepted — or, if none is, the one that came closest. When the frame holds more than one
/// region, all ink together is tried last: a symbol at a large pitch can fall apart into a
/// main region and a detached corner, and only the union has the right hull.
#[doc(hidden)]
pub fn trace(gray: &[u8], width: u32, height: u32, max_version: Version) -> RingTrace {
    let ink = InkMap::new(gray, width, height);
    let (regions, total) = ink.regions();
    let mut labels: Vec<Option<u32>> = regions
        .iter()
        .take(MAX_REGIONS)
        .map(|r| Some(r.label))
        .collect();
    if total > 1 {
        labels.push(None);
    }

    let mut best: Option<RingTrace> = None;
    for label in labels {
        let bbox = match label {
            Some(l) => regions
                .iter()
                .find(|r| r.label == l)
                .map_or((0, 0, width, height), |r| r.bbox),
            None => (0, 0, width, height),
        };
        let points = boundary_points(&ink, label, bbox);
        let mut t = attempt(gray, width, height, &points, max_version);
        t.regions = total;
        if t.fix.is_some() {
            return t;
        }
        let top = |t: &RingTrace| t.scores.iter().map(|s| s.1).fold(0.0f32, f32::max);
        if best.as_ref().is_none_or(|b| top(&t) > top(b)) {
            best = Some(t);
        }
    }
    // Nothing passed the ordinary rule: the best attempt's relaxed fix, if it has one.
    if let Some(b) = best.as_mut()
        && b.fix.is_none()
    {
        b.fix = b.fallback.take();
    }
    best.unwrap_or(RingTrace {
        regions: total,
        hull_len: 0,
        quad: None,
        scores: Vec::new(),
        fix: None,
        fallback: None,
    })
}

/// One hull, correlated against every version.
fn attempt(
    gray: &[u8],
    width: u32,
    height: u32,
    points: &[(f32, f32)],
    max_version: Version,
) -> RingTrace {
    // Lens distortion first (D23): if the ring's edges bow, straighten the boundary before
    // anything fits lines or corners to it, and carry the coefficient into every transform.
    let lens = estimate_lens(points, (width as f32 / 2.0, height as f32 / 2.0));
    let straightened;
    let points = if lens.2 == 0.0 {
        points
    } else {
        straightened = undistort(points, lens);
        &straightened[..]
    };
    let hull = convex_hull(points);
    let hull_len = hull.len();
    let Some(hull_quad) = quad(&hull) else {
        return RingTrace {
            regions: 0,
            hull_len,
            quad: None,
            scores: Vec::new(),
            fix: None,
            fallback: None,
        };
    };

    // Two candidate quads (D23). The refined hull is what every symbol decoded with before,
    // and on an undamaged ring its corners are the more precise — under blur, light and dark
    // ring cells smear the boundary differently, and the edge fit's consensus line settles on
    // one of them, which cost `C3` half its blur-radius-4 decodes when it was used everywhere.
    // So the edge fit only leads when the two *disagree*, which is what a damaged corner
    // looks like; otherwise the refined hull is used alone.
    let refined = refine_quad(points, hull_quad);
    let edges = edge_quad(points, hull_quad).filter(|e| !same_quad(e, &refined, CORNER_AGREE_PX));
    let mut tried: Option<RingTrace> = None;
    for q in [edges, Some(refined)].into_iter().flatten() {
        let t = correlate_quad(gray, width, height, q, lens, hull_len, max_version);
        if t.fix.is_some() {
            return t;
        }
        let top = |t: &RingTrace| t.scores.iter().map(|s| s.1).fold(0.0f32, f32::max);
        if tried.as_ref().is_none_or(|b| top(&t) > top(b)) {
            tried = Some(t);
        }
    }
    tried.expect("the refined quad is always tried")
}

/// Whether two quads agree to within `tol` pixels at every corner.
fn same_quad(a: &[(f32, f32); 4], b: &[(f32, f32); 4], tol: f32) -> bool {
    a.iter()
        .zip(b)
        .all(|(p, q)| (p.0 - q.0).abs() <= tol && (p.1 - q.1).abs() <= tol)
}

/// One candidate quad, correlated against every version.
fn correlate_quad(
    gray: &[u8],
    width: u32,
    height: u32,
    q: [(f32, f32); 4],
    lens: Lens,
    hull_len: usize,
    max_version: Version,
) -> RingTrace {
    // Approximate outer side length in pixels: mean diagonal over √2.
    let diag = |a: (f32, f32), b: (f32, f32)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let span = (diag(q[0], q[2]) + diag(q[1], q[3])) / 2.0 / core::f32::consts::SQRT_2;

    let mut scores = Vec::new();
    for version in Version::all() {
        if version > max_version {
            break;
        }
        let side = version.side();
        // Versions only get denser, so once positions are too small to sample, stop.
        if span / (side as f32 + 8.0) < MIN_PITCH_PX {
            break;
        }
        let Some(t) = ring_transform(&q, side, lens) else {
            continue;
        };
        let bits = classify(&ring_lumas(gray, width, height, &t, side));
        let (score, r, rev) = correlate(&bits, side);
        scores.push((version, score, r, rev));
    }

    let fix = accept(&q, &scores, lens, false);
    let fallback = if fix.is_none() {
        accept(&q, &scores, lens, true)
    } else {
        None
    };
    RingTrace {
        regions: 0,
        hull_len,
        quad: Some(q),
        scores,
        fix,
        fallback,
    }
}

/// Accept the best version if it clears [`MIN_AGREEMENT`] and leads the runner-up by
/// [`MIN_LEAD`], and resolve its orientation into a transform.
fn accept(
    q: &[(f32, f32); 4],
    scores: &[(Version, f32, usize, bool)],
    lens: Lens,
    relaxed: bool,
) -> Option<RingFix> {
    let (best_i, &(version, agreement, r, rev)) = scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.1.total_cmp(&b.1.1))?;
    let runner_up = scores
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != best_i)
        .map(|(_, s)| s.1)
        .fold(0.0f32, f32::max);
    let lead = agreement - runner_up;
    let ordinary = agreement >= MIN_AGREEMENT && lead >= MIN_LEAD;
    // Heavy defocus smears ring cells into their neighbours and agreement sags below
    // `MIN_AGREEMENT` while the right version still leads by a margin no other ink produces:
    // at blur radius 9–10 the true version read 0.84–0.93 against a runner-up near 0.6, where
    // anything that is not this ring reads near 0.5 on every version (D25).
    let defocused = relaxed && agreement >= RELAXED_AGREEMENT && lead >= STRONG_LEAD;
    if !ordinary && !defocused {
        return None;
    }

    // Reassign the hull corners so module top-left is where sequence position 0 was found.
    // Base corner `r` holds position 0. Going forwards, true top-right is the next corner
    // clockwise; mirrored, the sequence runs the other way round the hull.
    let order = if rev {
        [r, (r + 3) % 4, (r + 2) % 4, (r + 1) % 4]
    } else {
        [r, (r + 1) % 4, (r + 2) % 4, (r + 3) % 4]
    };
    let dst = [q[order[0]], q[order[1]], q[order[2]], q[order[3]]];
    Some(RingFix {
        version,
        transform: ring_transform(&dst, version.side(), lens)?,
        agreement,
    })
}

/// Read the format codeword from a located ring's colour choices.
///
/// Tries both palette families, because hues cannot be classified without knowing the
/// palette and the palette is what the format says. A wrong family fits illumination to the
/// wrong references and the BCH decode fails, so the right family identifies itself.
pub fn read_ring_format(
    img: &ImageView,
    t: &Transform,
    version: Version,
) -> Result<(FormatInfo, f32), LcError> {
    let side = version.side();
    let mut best: Option<(FormatInfo, f32)> = None;

    for family in [Profile::C1, Profile::C3] {
        let palette = Palette::for_profile(family);
        let hues = palette.hues;

        // §12.3 from reference positions only: their colours do not depend on the format,
        // which is what lets this run before the format is known.
        let refs: Vec<Reference> = border::cells(side)
            .filter_map(|(x, y)| {
                let nominal = border::reference_color(x, y, side, &palette)?;
                Some(Reference {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                    observed: sample_module(img, t, x, y),
                    nominal: color::srgb8_to_linear(nominal),
                })
            })
            .collect();
        let illum = IlluminationModel::fit(&refs, side, version.get());
        let stats = demod::measure(&refs, &illum, &palette);

        let mut ll = Vec::with_capacity(hues);
        let mut copies = [[0f32; FORMAT_COPY_BITS]; FORMAT_COPIES];
        for (copy, llrs) in copies.iter_mut().enumerate() {
            for (bit, out) in llrs.iter_mut().enumerate() {
                let (p, b) = border::format_slot(copy, bit, side, hues);
                // Format bits ride on data positions, whose inner cell carries payload
                // (D19), so this reads one cell rather than a stacked pair.
                let (x, y) = border::outer_cell(p, side);
                let raw = sample_module(img, t, x, y);
                let lab = color::linear_to_lab(illum.correct(x as f32 + 0.5, y as f32 + 0.5, raw));
                stats.hue_log_likelihoods(lab, &palette, &mut ll);
                *out = symbol_bit_llr(&ll, b, hues);
            }
        }

        for llrs in &copies {
            let (bits, margin) = bch::decode_soft(llrs);
            // `from_ring_bits`, not `from_bits`: a ringed symbol spends the version and
            // border bits on a wider profile field (D21), because the sequence correlation
            // above already established both. It also rejects a non-zero reserved field,
            // which is what now catches a word that survived the BCH decode as garbage.
            let Ok(info) = FormatInfo::from_ring_bits(bits, version) else {
                continue;
            };
            let consistent = info.profile != Profile::M
                // C1 and C2 share the 4-colour palette, C3 has the 8-colour one; comparing
                // families avoids building a second palette just to read its hue count.
                && (info.profile == Profile::C3) == (family == Profile::C3);
            if consistent && best.as_ref().is_none_or(|(_, m)| margin > *m) {
                best = Some((info, margin));
            }
        }
    }
    best.ok_or(LcError::FormatUndecodable)
}

// ---------------------------------------------------------------- geometry

/// Where a frame's ink is, and which connected region each piece of it belongs to.
///
/// Built for the hull, which is decided entirely by the *outermost* ink, so the one property
/// that matters is that background is never ink — and, for regions to mean symbols, that a
/// symbol is one solid piece of it, which [`PAPER_MARGIN`] gives. Two things keep background
/// out where the shared adaptive binary did not:
///
/// * **A contrast floor that noise cannot clear.** Levels are the 5th and 95th percentile
///   luma of each cell, pooled over a neighbourhood; a neighbourhood without
///   [`MIN_INK_CONTRAST`] between them holds no ink at all.
/// * **Regions.** Ink is labelled into connected regions (one region cell of dilation bridges
///   a light module), so a second symbol, or anything else dark in the frame, gets its own hull
///   instead of stretching this one.
struct InkMap<'a> {
    gray: &'a [u8],
    width: u32,
    /// Per level cell; a pixel is ink when darker than its cell's value. 0 holds no ink.
    threshold: Vec<u8>,
    level_w: u32,
    /// Per region cell: 0 if the cell is not ink, else its region's label.
    label: Vec<u32>,
    region_w: u32,
}

/// A connected ink region: its label, size in region cells, and pixel bounding box.
struct Region {
    label: u32,
    cells: usize,
    /// `(x0, y0, x1, y1)`, end-exclusive.
    bbox: (u32, u32, u32, u32),
}

impl<'a> InkMap<'a> {
    fn new(gray: &'a [u8], width: u32, height: u32) -> Self {
        let (lw, lh) = (width.div_ceil(LEVEL_CELL), height.div_ceil(LEVEL_CELL));
        let (lwu, lhu) = (lw as usize, lh as usize);
        let mut lo = alloc::vec![0u8; lwu * lhu];
        let mut hi = alloc::vec![0u8; lwu * lhu];
        for cy in 0..lh {
            for cx in 0..lw {
                let mut hist = [0u32; 32];
                let mut count = 0u32;
                for y in cy * LEVEL_CELL..((cy + 1) * LEVEL_CELL).min(height) {
                    for x in cx * LEVEL_CELL..((cx + 1) * LEVEL_CELL).min(width) {
                        hist[(gray[(y * width + x) as usize] >> 3) as usize] += 1;
                        count += 1;
                    }
                }
                let cut = count / 20;
                let (mut acc, mut b) = (0, 0);
                while b < 31 {
                    acc += hist[b];
                    if acc > cut {
                        break;
                    }
                    b += 1;
                }
                let low = b;
                (acc, b) = (0, 31);
                while b > 0 {
                    acc += hist[b];
                    if acc > cut {
                        break;
                    }
                    b -= 1;
                }
                let i = (cy * lw + cx) as usize;
                (lo[i], hi[i]) = ((low * 8) as u8, (b * 8 + 7) as u8);
            }
        }

        // Pool over the neighbourhood: darkest dark and lightest light.
        let (lo, hi) = (pool(&lo, lwu, lhu, false), pool(&hi, lwu, lhu, true));
        let mut threshold: Vec<u8> = lo
            .iter()
            .zip(&hi)
            .map(|(&l, &h)| {
                if h.saturating_sub(l) >= MIN_INK_CONTRAST {
                    (h as f32 - PAPER_MARGIN * (h - l) as f32) as u8
                } else {
                    0
                }
            })
            .collect();

        // Carry thresholds into flat cells from their neighbours, a bounded distance.
        for _ in 0..LEVEL_FILL {
            let prev = threshold.clone();
            let mut changed = false;
            for y in 0..lhu {
                for x in 0..lwu {
                    if prev[y * lwu + x] != 0 {
                        continue;
                    }
                    let (mut sum, mut n) = (0u32, 0u32);
                    for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx >= 0 && ny >= 0 && (nx as usize) < lwu && (ny as usize) < lhu {
                            let v = prev[ny as usize * lwu + nx as usize];
                            if v != 0 {
                                (sum, n) = (sum + v as u32, n + 1);
                            }
                        }
                    }
                    if let Some(avg) = sum.checked_div(n) {
                        threshold[y * lwu + x] = avg as u8;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        let mut map = InkMap {
            gray,
            width,
            threshold,
            level_w: lw,
            label: Vec::new(),
            region_w: 0,
        };

        // Region cells: ink where enough pixels are.
        let (rw, rh) = (width.div_ceil(REGION_CELL), height.div_ceil(REGION_CELL));
        let (rwu, rhu) = (rw as usize, rh as usize);
        let mut ink_cell = alloc::vec![false; rwu * rhu];
        for cy in 0..rh {
            for cx in 0..rw {
                let mut n = 0u32;
                for y in cy * REGION_CELL..((cy + 1) * REGION_CELL).min(height) {
                    for x in cx * REGION_CELL..((cx + 1) * REGION_CELL).min(width) {
                        n += map.ink(x, y) as u32;
                    }
                }
                ink_cell[(cy * rw + cx) as usize] = n >= MIN_CELL_INK;
            }
        }
        // Label connected ink cells, bridging gaps of up to two cells: everything within
        // Chebyshev distance 3 is a neighbour. The same grouping as dilating by one cell and
        // taking 8-connected components, without building the dilated grid.
        let mut label = alloc::vec![0u32; rwu * rhu];
        let mut next = 0u32;
        let mut stack = Vec::new();
        for start in 0..rwu * rhu {
            if !ink_cell[start] || label[start] != 0 {
                continue;
            }
            next += 1;
            label[start] = next;
            stack.push(start);
            while let Some(i) = stack.pop() {
                let (x, y) = (i % rwu, i / rwu);
                for gy in y.saturating_sub(3)..=(y + 3).min(rhu - 1) {
                    for gx in x.saturating_sub(3)..=(x + 3).min(rwu - 1) {
                        let j = gy * rwu + gx;
                        if ink_cell[j] && label[j] == 0 {
                            label[j] = next;
                            stack.push(j);
                        }
                    }
                }
            }
        }
        map.label = label;
        map.region_w = rw;
        map
    }

    fn ink(&self, x: u32, y: u32) -> bool {
        let t = self.threshold[((y / LEVEL_CELL) * self.level_w + x / LEVEL_CELL) as usize];
        self.gray[(y * self.width + x) as usize] < t
    }

    /// Whether `(x, y)` is ink in region `label`, or in any region if `label` is `None`.
    fn member(&self, x: u32, y: u32, label: Option<u32>) -> bool {
        let l = self.label[((y / REGION_CELL) * self.region_w + x / REGION_CELL) as usize];
        l != 0 && label.is_none_or(|want| want == l) && self.ink(x, y)
    }

    /// The [`MAX_REGIONS`] largest regions of at least [`MIN_REGION_CELLS`], largest first,
    /// and how many regions there were before that filter.
    fn regions(&self) -> (Vec<Region>, usize) {
        let rw = self.region_w as usize;
        let mut all: Vec<Region> = Vec::new();
        for (i, &l) in self.label.iter().enumerate() {
            if l == 0 {
                continue;
            }
            let (x, y) = ((i % rw) as u32 * REGION_CELL, (i / rw) as u32 * REGION_CELL);
            let (x1, y1) = (x + REGION_CELL, y + REGION_CELL);
            match all.iter_mut().find(|r| r.label == l) {
                Some(r) => {
                    r.cells += 1;
                    r.bbox = (
                        r.bbox.0.min(x),
                        r.bbox.1.min(y),
                        r.bbox.2.max(x1),
                        r.bbox.3.max(y1),
                    );
                }
                None => all.push(Region {
                    label: l,
                    cells: 1,
                    bbox: (x, y, x1, y1),
                }),
            }
        }
        let total = all.len();
        let height = self.gray.len() as u32 / self.width;
        // Bounded insertion rather than a sort: only the first few are ever read, and a sort
        // instantiated a stable sort for `Region` — 4.4 KB of scanner.
        let mut top: Vec<Region> = Vec::new();
        for mut r in all.into_iter().filter(|r| r.cells >= MIN_REGION_CELLS) {
            r.bbox.2 = r.bbox.2.min(self.width);
            r.bbox.3 = r.bbox.3.min(height);
            let at = top
                .iter()
                .position(|t| r.cells > t.cells)
                .unwrap_or(top.len());
            if at < MAX_REGIONS {
                top.insert(at, r);
                top.truncate(MAX_REGIONS);
            }
        }
        (top, total)
    }
}

/// Separable min or max filter over a `w × h` grid, radius [`LEVEL_RADIUS`].
fn pool(v: &[u8], w: usize, h: usize, max: bool) -> Vec<u8> {
    let pick = |a: u8, b: u8| if max { a.max(b) } else { a.min(b) };
    let mut rows = v.to_vec();
    for y in 0..h {
        for x in 0..w {
            let mut m = v[y * w + x];
            for k in x.saturating_sub(LEVEL_RADIUS)..=(x + LEVEL_RADIUS).min(w - 1) {
                m = pick(m, v[y * w + k]);
            }
            rows[y * w + x] = m;
        }
    }
    let mut out = rows.clone();
    for y in 0..h {
        for x in 0..w {
            let mut m = rows[y * w + x];
            for k in y.saturating_sub(LEVEL_RADIUS)..=(y + LEVEL_RADIUS).min(h - 1) {
                m = pick(m, rows[k * w + x]);
            }
            out[y * w + x] = m;
        }
    }
    out
}

/// Outer extremes of the ink *runs* on every row and column of a bounding box.
///
/// Enough to find the convex hull without walking every ink pixel, because only the outer
/// boundary can be a hull vertex. Pixel *edges* rather than centres, so the hull lands on the
/// ring's outer edge, which is where module coordinates put it. Runs shorter than
/// [`MIN_RUN_PX`] are skipped, so isolated speckle cannot become a vertex.
fn boundary_points(
    ink: &InkMap,
    label: Option<u32>,
    bbox: (u32, u32, u32, u32),
) -> Vec<(f32, f32)> {
    let (x0, y0, x1, y1) = bbox;
    let (bw, bh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mask: Vec<bool> = (0..bw * bh)
        .map(|i| ink.member(x0 + (i % bw) as u32, y0 + (i / bw) as u32, label))
        .collect();
    let (fx, fy) = (x0 as f32, y0 as f32);
    let mut pts = Vec::new();
    for r in 0..bh {
        if let Some((a, b)) = run_extremes(&mask, r * bw, 1, bw) {
            pts.push((fx + a as f32, fy + r as f32 + 0.5));
            pts.push((fx + b as f32 + 1.0, fy + r as f32 + 0.5));
        }
    }
    for c in 0..bw {
        if let Some((a, b)) = run_extremes(&mask, c, bw, bh) {
            pts.push((fx + c as f32 + 0.5, fy + a as f32));
            pts.push((fx + c as f32 + 0.5, fy + b as f32 + 1.0));
        }
    }
    pts
}

/// First and last index along one line of `mask` that lie in a run of at least
/// [`MIN_RUN_PX`]. The line starts at `start` and advances by `step`, for `len` pixels.
fn run_extremes(mask: &[bool], start: usize, step: usize, len: usize) -> Option<(usize, usize)> {
    let (mut first, mut last, mut run) = (None, 0, 0);
    for k in 0..len {
        if mask[start + k * step] {
            run += 1;
            if run >= MIN_RUN_PX as usize {
                first.get_or_insert(k + 1 - run);
                last = k;
            }
        } else {
            run = 0;
        }
    }
    first.map(|f| (f, last))
}

/// Gift-wrapping convex hull.
///
/// Rather than a sorting hull: a ring's hull has a few dozen vertices at most, so wrapping is
/// cheap, and the sort a monotone chain needs cost 3.8 KB of scanner. Orientation does not
/// matter to [`quad`], which orders its own corners.
fn convex_hull(p: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let cross = |o: (f32, f32), a: (f32, f32), b: (f32, f32)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let d2 = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0) * (a.0 - b.0) + (a.1 - b.1) * (a.1 - b.1);
    let mut hull = Vec::new();
    let Some(start) =
        (0..p.len()).min_by(|&i, &j| p[i].0.total_cmp(&p[j].0).then(p[i].1.total_cmp(&p[j].1)))
    else {
        return hull;
    };
    let mut cur = start;
    // Bounded, so a degenerate input cannot loop: a hull never has more vertices than points.
    while hull.len() < p.len() {
        hull.push(p[cur]);
        let mut next = (cur + 1) % p.len();
        for i in 0..p.len() {
            let c = cross(p[cur], p[next], p[i]);
            // Take the most clockwise candidate; among collinear ones, the farthest, so no
            // edge-interior point becomes a vertex.
            if c < 0.0 || (c == 0.0 && d2(p[cur], p[i]) > d2(p[cur], p[next])) {
                next = i;
            }
        }
        cur = next;
        // Compare coordinates, not indices: a duplicate of the start point ends the walk too.
        if p[cur] == p[start] {
            break;
        }
    }
    hull
}

/// Four corners from a hull, clockwise in image space.
///
/// The farthest pair of hull points is a diagonal of a square under any rotation and under
/// moderate perspective; the other two corners are then the points farthest from that
/// diagonal on either side. Simpler than a maximum-area quadrilateral search and exact for
/// the shape it is looking for.
fn quad(hull: &[(f32, f32)]) -> Option<[(f32, f32); 4]> {
    if hull.len() < 4 {
        return None;
    }
    let d2 = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2);
    let (mut ia, mut ib, mut far) = (0, 0, -1.0f32);
    for i in 0..hull.len() {
        for j in i + 1..hull.len() {
            let d = d2(hull[i], hull[j]);
            if d > far {
                (ia, ib, far) = (i, j, d);
            }
        }
    }
    let (a, b) = (hull[ia], hull[ib]);
    let side_of = |p: (f32, f32)| (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
    let (mut c, mut cs, mut d, mut ds) = (a, 0.0f32, a, 0.0f32);
    for &p in hull {
        let s = side_of(p);
        if s > cs {
            (c, cs) = (p, s);
        }
        if s < ds {
            (d, ds) = (p, s);
        }
    }
    if cs <= 0.0 || ds >= 0.0 {
        return None;
    }
    // `a` and `b` are opposite corners and `c`, `d` lie either side of that diagonal, so
    // `a, c, b, d` already runs round the quadrilateral and only its direction needs fixing.
    // A shoelace sign rather than an angle sort: with y down a positive area is clockwise on
    // screen, and sorting by `atan2` instantiated a sort and pulled libm's arctangent into
    // the scanner.
    let q = [a, c, b, d];
    let area: f32 = (0..4)
        .map(|i| {
            let (p, r) = (q[i], q[(i + 1) % 4]);
            p.0 * r.1 - r.0 * p.1
        })
        .sum();
    Some(if area >= 0.0 { q } else { [a, d, b, c] })
}

/// Homography taking the ring's outer square, in module space, onto four image corners —
/// in the *undistorted* image when `lens` carries a coefficient, with the distortion then
/// applied on the way out (D23).
///
/// Module space puts cell `x` at `[x, x + 1)`, so the outer edges of the ring's outer layer
/// are at `-(QUIET + WIDTH)` and `side + QUIET + WIDTH`.
fn ring_transform(corners: &[(f32, f32); 4], side: u16, lens: Lens) -> Option<Transform> {
    let lo = -(border::GAP + border::WIDTH) as f32;
    let hi = side as f32 + (border::GAP + border::WIDTH) as f32;
    Transform::from_quad([(lo, lo), (hi, lo), (hi, hi), (lo, hi)], *corners).map(|t| {
        if lens.2 == 0.0 {
            t
        } else {
            t.with_radial(lens.0, lens.1, lens.2)
        }
    })
}

/// Radial distortion as `(cx, cy, κ)`, the model of [`Transform::with_radial`].
type Lens = (f32, f32, f32);

/// Least distortion worth modelling, as the fraction `κ·R²` by which it displaces the ring's
/// farthest corner. Below this a clean render's estimate is quantisation noise, and leaving
/// `κ = 0` keeps an undistorted capture on exactly the path it always took.
const LENS_MIN: f32 = 0.004;

/// Estimate radial distortion about `centre` from how a ring's edges bow (D23).
///
/// Under the [`Transform::with_radial`] model a straight edge at perpendicular distance `h`
/// from the centre, with chord length `L`, bows by
///
/// ```text
/// offset(t)  ≈  c0 + c1·t  +  κ · h · L² · t(1 − t)          t ∈ [0, 1] along the edge
/// ```
///
/// — outward for barrel, inward for pincushion — whatever the edge's direction or where it
/// sits in the frame. The line terms `c0 + c1·t` are fitted alongside the bow rather than
/// taken from the hull's corners, so a corner that is occluded, glared away or merely
/// imprecise moves the line and not the curvature. Each edge then gives its own `κ`.
///
/// **A lens bends all four edges; damage bends one.** The estimate is accepted only when the
/// four edges agree, which is what keeps a damaged corner from reading as distortion — the
/// first version measured the bow against the hull's chord, and a glared corner's chopped
/// chord came out as `κ` and cost `C3` a third of its decodes under strong corner glare.
/// Distances are taken on the distorted image, so the estimate is refined once on the
/// straightened points.
///
/// Returns `κ = 0` when the bow is below [`LENS_MIN`], the edges disagree, or any edge is
/// too thin to measure.
fn estimate_lens(points: &[(f32, f32)], centre: (f32, f32)) -> Lens {
    let none = (centre.0, centre.1, 0.0);
    let mut kappa = 0.0f32;
    let mut reach = 0.0f32;
    let mut sel: Vec<(f32, f32)> = Vec::new();
    for _ in 0..2 {
        let pts = if kappa == 0.0 {
            points.to_vec()
        } else {
            undistort(points, (centre.0, centre.1, kappa))
        };
        let Some(q) = quad(&convex_hull(&pts)) else {
            return none;
        };
        // Per edge: (κ, standard error of κ).
        let mut edges = [(0.0f32, 0.0f32); 4];
        for (e, slot) in edges.iter_mut().enumerate() {
            let (a, b) = (q[e], q[(e + 1) % 4]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 16.0 {
                return none;
            }
            let (ux, uy) = (dx / len, dy / len);
            // Outward normal: away from the distortion centre.
            let (mut nx, mut ny) = (uy, -ux);
            let h0 = (a.0 - centre.0) * nx + (a.1 - centre.1) * ny;
            if h0 < 0.0 {
                (nx, ny) = (-nx, -ny);
            }
            sel.clear();
            sel.extend(pts.iter().filter_map(|&(px, py)| {
                let t = ((px - a.0) * ux + (py - a.1) * uy) / len;
                let o = (px - a.0) * nx + (py - a.1) * ny;
                ((0.1..=0.9).contains(&t) && o.abs() <= 0.05 * len).then_some((t, o))
            }));
            if (sel.len() as f32) < 0.3 * len {
                return none;
            }
            // Fit o = c0 + c1·t + α·t(1−t), then once more without the points that sit off
            // that curve — what is left of a damaged corner inside the window.
            let Some((first, _)) = bow_fit(&sel, None) else {
                return none;
            };
            let Some((fit, se)) = bow_fit(&sel, Some(first)) else {
                return none;
            };
            // This edge's own κ and its standard error: α = κ·h·L².
            let w = h0.abs() * len * len;
            *slot = (fit[2] / w, (se / w).max(f32::MIN_POSITIVE));
            for &c in &[a, b] {
                reach = reach.max((c.0 - centre.0).powi(2) + (c.1 - centre.1).powi(2));
            }
        }
        // Inverse-variance mean: a blurred, wavy edge measures its bow less precisely and
        // counts for less, rather than vetoing the estimate.
        let (num, den) = edges.iter().fold((0.0f32, 0.0f32), |(n, d), &(k, se)| {
            (n + k / (se * se), d + 1.0 / (se * se))
        });
        if den == 0.0 || !den.is_finite() {
            return none;
        }
        let step = num / den;
        // Agreement: every edge within three of its own standard errors of the estimate, plus
        // a quarter of the *total* κ — a lens bends all four edges, damage bends one. Total,
        // not this pass's step: the refining pass measures a small residual, and slack
        // proportional to that rejected every real lens on its second look.
        let total = (kappa + step).abs();
        if edges
            .iter()
            .any(|&(k, se)| (k - step).abs() > 3.0 * se + 0.25 * total)
        {
            return none;
        }
        kappa += step;
    }
    if (kappa * reach).abs() < LENS_MIN {
        return none;
    }
    (centre.0, centre.1, kappa)
}

/// Distance, in pixels, beyond which a point is dropped from an edge's second bow fit.
const BOW_INLIER_PX: f32 = 2.0;

/// Least-squares `[c0, c1, α]` for `o = c0 + c1·t + α·t(1 − t)` over `(t, o)` samples, using
/// only the samples within [`BOW_INLIER_PX`] of `prev` when one is given — and the standard
/// error of `α`, from the fit's own residuals.
fn bow_fit(sel: &[(f32, f32)], prev: Option<[f32; 3]>) -> Option<([f32; 3], f32)> {
    let mut ata = [0f64; 9];
    let mut atb = [0f64; 3];
    let mut n = 0usize;
    for &(t, o) in sel {
        let g = t * (1.0 - t);
        if let Some([c0, c1, a]) = prev
            && (o - c0 - c1 * t - a * g).abs() > BOW_INLIER_PX
        {
            continue;
        }
        let row = [1.0, t as f64, g as f64];
        for i in 0..3 {
            atb[i] += row[i] * o as f64;
            for j in 0..3 {
                ata[i * 3 + j] += row[i] * row[j];
            }
        }
        n += 1;
    }
    if n < 8 {
        return None;
    }
    let normal = ata;
    let x = solve(&mut ata, &mut atb, 3)?;
    let fit = [x[0] as f32, x[1] as f32, x[2] as f32];
    // Var(α) = σ² · [(AᵀA)⁻¹]₂₂, the last diagonal entry found by solving against e₂.
    let mut a2 = normal;
    let inv = solve(&mut a2, &mut [0.0, 0.0, 1.0], 3)?;
    let mut rss = 0.0f32;
    for &(t, o) in sel {
        let r = o - fit[0] - fit[1] * t - fit[2] * t * (1.0 - t);
        if prev.is_none() || r.abs() <= BOW_INLIER_PX {
            rss += r * r;
        }
    }
    let var = rss / (n - 3) as f32 * inv[2].max(0.0) as f32;
    Some((fit, var.sqrt()))
}

/// Sensor points to where an undistorted camera would have put them, under `lens`.
fn undistort(points: &[(f32, f32)], lens: Lens) -> Vec<(f32, f32)> {
    let (cx, cy, k) = lens;
    points
        .iter()
        .map(|&(x, y)| {
            let (dx, dy) = (x - cx, y - cy);
            let s = 1.0 + k * (dx * dx + dy * dy);
            (cx + dx * s, cy + dy * s)
        })
        .collect()
}

/// Mean greyscale luma of each ring position, over both layers and a small cross of points.
///
/// **Read from the greyscale image, not the adaptive binary, and that choice was measured.**
/// The ring is an island of mid-luma colours in a sea of white quiet zone. A binarisation
/// block that is mostly white sets its threshold between a light hue (~188) and white (255),
/// so light-but-not-white ring cells binarise *dark*. On a clean render that cost 11–33% of
/// positions — true-version agreement 0.67 to 0.89 depending on pitch and phase, where a
/// clean image should read 1.0 — and rejected every clean render at 8, 20, 64 and 80
/// px/module. [`classify`] thresholds these against the ring's own two levels instead.
fn ring_lumas(gray: &[u8], w: u32, h: u32, t: &Transform, side: u16) -> Vec<f32> {
    (0..border::positions(side))
        .map(|p| {
            let (mut sum, mut n) = (0.0f32, 0.0f32);
            for (layer, (x, y)) in [border::outer_cell(p, side), border::inner_cell(p, side)]
                .into_iter()
                .enumerate()
            {
                // Two deep only where the ring still is: a data position's inner cell
                // carries payload now (D19), so its luma is not this position's.
                if layer == 1
                    && (!border::is_two_deep(p) || border::position(x, y, side) != Some(p))
                {
                    continue;
                }
                for (du, dv) in [(0.5, 0.5), (0.3, 0.5), (0.7, 0.5), (0.5, 0.3), (0.5, 0.7)] {
                    let (px, py) = t.apply(x as f32 + du, y as f32 + dv);
                    if px < 0.0 || py < 0.0 || px >= w as f32 || py >= h as f32 {
                        continue;
                    }
                    sum += gray[(py as u32 * w + px as u32) as usize] as f32;
                    n += 1.0;
                }
            }
            if n > 0.0 { sum / n } else { f32::NAN }
        })
        .collect()
}

/// Light or dark per position, by a threshold local to each position.
///
/// A two-means split over the [`LUMA_WINDOW`] positions either side. The sequence's runs
/// are at most four long, so every window holds both levels; and a *local* split rather
/// than one global threshold is what survives a shading gradient, which can put a light
/// cell in a dark corner below a dark cell in a bright one.
fn classify(lumas: &[f32]) -> Vec<bool> {
    let n = lumas.len();
    (0..n)
        .map(|p| {
            if !lumas[p].is_finite() {
                return false;
            }
            let mut win = [0.0f32; 2 * LUMA_WINDOW + 1];
            let mut len = 0;
            for k in 0..=2 * LUMA_WINDOW {
                let v = lumas[(p + n + k - LUMA_WINDOW) % n];
                if v.is_finite() {
                    win[len] = v;
                    len += 1;
                }
            }
            if len < 4 {
                return false;
            }
            let win = &win[..len];
            let mut thr = win.iter().sum::<f32>() / len as f32;
            for _ in 0..4 {
                let (mut lo, mut ln, mut hi, mut hn) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                for &v in win {
                    if v > thr {
                        (hi, hn) = (hi + v, hn + 1.0);
                    } else {
                        (lo, ln) = (lo + v, ln + 1.0);
                    }
                }
                if ln == 0.0 || hn == 0.0 {
                    break;
                }
                thr = (lo / ln + hi / hn) / 2.0;
            }
            lumas[p] > thr
        })
        .collect()
}

/// Refine hull corners by fitting a line to each edge and intersecting neighbouring lines.
///
/// Hull vertices sit on the pixel staircase and on blur-rounded corners, which put every
/// clean-render corner ~2 px off and blur r = 4 corners up to 4 px off — a third of a module
/// at 12 px/module, enough to fail the payload CRC with the format read perfectly. An edge
/// stays straight under blur and perspective where its corner does not, so the line through
/// an edge's middle locates the corner better than the corner itself does.
fn refine_quad(points: &[(f32, f32)], q: [(f32, f32); 4]) -> [(f32, f32); 4] {
    let mut lines = [((0.0f32, 0.0f32), (0.0f32, 0.0f32)); 4];
    for (e, line) in lines.iter_mut().enumerate() {
        let (a, b) = (q[e], q[(e + 1) % 4]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 16.0 {
            return q;
        }
        let (ux, uy) = (dx / len, dy / len);
        // Two passes: gather near the hull edge, then tighten around the fitted line, so a
        // stray point inside the first tolerance cannot tilt the final fit.
        let mut fit = (a, (ux, uy));
        for tol in [(len * 0.02).max(2.0), 1.5] {
            let ((ox, oy), (fx, fy)) = fit;
            let sel: Vec<(f32, f32)> = points
                .iter()
                .copied()
                .filter(|&(px, py)| {
                    let along = ((px - a.0) * ux + (py - a.1) * uy) / len;
                    let off = ((px - ox) * fy - (py - oy) * fx).abs();
                    (0.15..=0.85).contains(&along) && off <= tol
                })
                .collect();
            if sel.len() < 8 {
                return q;
            }
            fit = tls_line(&sel);
        }
        *line = fit;
    }
    let mut out = q;
    for (c, corner) in out.iter_mut().enumerate() {
        match intersect(lines[(c + 3) % 4], lines[c]) {
            // A "refinement" that moves a corner a long way is a bad fit, not a better corner.
            Some(p) if (p.0 - q[c].0).powi(2) + (p.1 - q[c].1).powi(2) <= 64.0 => *corner = p,
            _ => return q,
        }
    }
    out
}

/// Corner disagreement, in pixels, below which the edge fit and the refined hull are taken to
/// describe the same undamaged ring, and the refined hull's corners are used.
const CORNER_AGREE_PX: f32 = 3.0;

/// Fraction of an edge's length a point may sit off the hull's version of that edge and still
/// be a candidate for the true one. Covers a corner displaced by a sixth of the symbol.
const EDGE_SEARCH: f32 = 0.15;

/// Distance, in pixels, within which a boundary point agrees with a candidate edge line.
const EDGE_INLIER_PX: f32 = 1.5;

/// Half-width, in pixels, of the band the first refit around a consensus line gathers from.
const EDGE_REFIT_PX: f32 = 3.0;

/// Consensus trials per edge.
const EDGE_TRIALS: usize = 96;

/// Least fraction of an edge's length that must lie on one straight line for the edge to be
/// believed. A ring edge is straight end to end, so a clean one scores near 1.
const EDGE_SUPPORT: f32 = 0.35;

/// Four corners from four robustly fitted edges, ignoring where the hull put its corners (D23).
///
/// The hull's corners are its weakest part. Its four extreme points are taken as the ring's
/// corners, so anything that moves an extreme point moves a corner, and corners are where
/// damage lands. A patch over a corner is ink, and pulls that corner outward. A highlight
/// erases the ring's corner and chops it inward. [`refine_quad`] cannot repair either,
/// because it starts from the hull's corners and refuses to move one far.
///
/// An *edge* is long and straight, and damage near a corner touches only its end. So each
/// edge is fitted by consensus — many trial lines through pairs of boundary points near the
/// hull's version of the edge, keeping the one the most points agree with — and refitted by
/// total least squares to its agreeing points. Corners come from intersecting neighbouring
/// edges. A corner that is occluded, glared away or simply missing is reconstructed from the
/// two edges that meet there.
///
/// `None` when an edge has too little straight support or the result is not a sane convex
/// quad near the hull's, and the caller falls back to [`refine_quad`].
fn edge_quad(points: &[(f32, f32)], q: [(f32, f32); 4]) -> Option<[(f32, f32); 4]> {
    let mut lines = [((0.0f32, 0.0f32), (0.0f32, 0.0f32)); 4];
    let mut cand: Vec<(f32, f32)> = Vec::new();
    let mut inliers: Vec<(f32, f32)> = Vec::new();
    for (e, line) in lines.iter_mut().enumerate() {
        let (a, b) = (q[e], q[(e + 1) % 4]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 16.0 {
            return None;
        }
        let (ux, uy) = (dx / len, dy / len);
        cand.clear();
        cand.extend(points.iter().copied().filter(|&(px, py)| {
            let along = ((px - a.0) * ux + (py - a.1) * uy) / len;
            let off = ((px - a.0) * uy - (py - a.1) * ux).abs();
            (-0.05..=1.05).contains(&along) && off <= EDGE_SEARCH * len
        }));
        let n = cand.len();
        if n < 8 {
            return None;
        }

        // Deterministic trials: index pairs from two coprime strides, so a failure is
        // reproducible and no RNG reaches the scanner.
        let (mut best, mut best_count) = (None, 0usize);
        for k in 0..EDGE_TRIALS {
            let (i, j) = ((k * 7919 + 13) % n, (k * 104_729 + n / 2) % n);
            let (p, r) = (cand[i], cand[j]);
            let (lx, ly) = (r.0 - p.0, r.1 - p.1);
            let l = (lx * lx + ly * ly).sqrt();
            // Short baselines give wild directions; a trial must span a third of the edge.
            if l < len / 3.0 {
                continue;
            }
            let (fx, fy) = (lx / l, ly / l);
            let count = cand
                .iter()
                .filter(|&&(px, py)| ((px - p.0) * fy - (py - p.1) * fx).abs() <= EDGE_INLIER_PX)
                .count();
            if count > best_count {
                (best, best_count) = (Some((p, (fx, fy))), count);
            }
        }
        // Points per unit length: rows and columns both contribute an extreme per pixel, so
        // a fully supported axis-aligned edge offers about one point per pixel of length.
        let mut fit = best?;
        if (best_count as f32) < EDGE_SUPPORT * len {
            return None;
        }
        // Refit as `refine_quad` does, around the consensus line rather than the hull edge:
        // a wide band first, then a tight one, over the edge's middle only. The consensus
        // line itself is biased — under blur, light and dark ring cells smear the boundary
        // into two parallel populations a pixel or so apart, and the line with most inliers
        // runs through one of them. The wide pass centres it between both; the middle-only
        // window keeps whatever damaged a corner out of the fit.
        for tol in [EDGE_REFIT_PX, EDGE_INLIER_PX] {
            let ((ox, oy), (fx, fy)) = fit;
            inliers.clear();
            inliers.extend(cand.iter().copied().filter(|&(px, py)| {
                let along = ((px - a.0) * ux + (py - a.1) * uy) / len;
                (0.15..=0.85).contains(&along) && ((px - ox) * fy - (py - oy) * fx).abs() <= tol
            }));
            if inliers.len() < 8 {
                return None;
            }
            fit = tls_line(&inliers);
        }
        *line = fit;
    }

    let mut out = q;
    let span = ((q[0].0 - q[2].0).powi(2) + (q[0].1 - q[2].1).powi(2)).sqrt();
    for (c, corner) in out.iter_mut().enumerate() {
        let p = intersect(lines[(c + 3) % 4], lines[c])?;
        // A reconstructed corner may move a long way from a damaged hull corner, but not off
        // the scale of the symbol.
        if (p.0 - q[c].0).powi(2) + (p.1 - q[c].1).powi(2) > (0.25 * span).powi(2) {
            return None;
        }
        *corner = p;
    }
    Some(out)
}

/// Total-least-squares line through points: `(centroid, unit direction)`.
fn tls_line(pts: &[(f32, f32)]) -> ((f32, f32), (f32, f32)) {
    let n = pts.len() as f32;
    let mx = pts.iter().map(|p| p.0).sum::<f32>() / n;
    let my = pts.iter().map(|p| p.1).sum::<f32>() / n;
    let (mut sxx, mut syy, mut sxy) = (0.0f32, 0.0f32, 0.0f32);
    for &(x, y) in pts {
        let (dx, dy) = (x - mx, y - my);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    // Principal axis in closed form — the eigenvector of the scatter matrix's larger
    // eigenvalue — so this needs only `sqrt`, which is a wasm instruction, and no libm trig.
    let half = (sxx - syy) / 2.0;
    let lambda = (sxx + syy) / 2.0 + (half * half + sxy * sxy).sqrt();
    // Two equivalent eigenvector forms; take the larger for numerical stability.
    let (dx, dy) = if (lambda - syy).abs() >= (lambda - sxx).abs() {
        (lambda - syy, sxy)
    } else {
        (sxy, lambda - sxx)
    };
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return ((mx, my), (1.0, 0.0));
    }
    ((mx, my), (dx / len, dy / len))
}

/// Intersection of two `(point, direction)` lines, or `None` if they are near parallel.
fn intersect(l0: ((f32, f32), (f32, f32)), l1: ((f32, f32), (f32, f32))) -> Option<(f32, f32)> {
    let ((x0, y0), (u0, v0)) = l0;
    let ((x1, y1), (u1, v1)) = l1;
    let det = u0 * v1 - v0 * u1;
    if det.abs() < 1e-3 {
        return None;
    }
    let t = ((x1 - x0) * v1 - u1 * (y1 - y0)) / det;
    Some((x0 + t * u0, y0 + t * v0))
}

/// Best agreement between sampled ring luma and the version's sequence, over the four
/// corner-aligned rotations and both directions. Returns `(agreement, corner, mirrored)`.
///
/// Only corner-aligned shifts are tried because the hull already fixes the corners: the
/// uncertainty is which corner is top-left and which way round, not where along an edge.
fn correlate(bits: &[bool], side: u16) -> (f32, usize, bool) {
    let n = bits.len();
    let s = side as usize + 2 * (border::GAP + border::WIDTH) as usize;
    let mut best = (0.0f32, 0usize, false);
    for rev in [false, true] {
        for r in 0..4 {
            let k = r * (s - 1);
            let agree = (0..n)
                .filter(|&i| {
                    let at = if rev { (k + n - i) % n } else { (k + i) % n };
                    bits[at] == border::sequence_light(side, i)
                })
                .count();
            let score = agree as f32 / n as f32;
            if score > best.0 {
                best = (score, r, rev);
            }
        }
    }
    best
}

/// Max-log LLR for bit `b` of a hue's Gray-labelled symbol. Positive means bit 0.
fn symbol_bit_llr(ll: &[f32], b: usize, hues: usize) -> f32 {
    let (mut zero, mut one) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for (h, &v) in ll.iter().enumerate() {
        if (lc_core::symbol::hue_to_chroma_symbol(h, hues) >> b) & 1 == 0 {
            zero = zero.max(v);
        } else {
            one = one.max(v);
        }
    }
    (zero - one).clamp(-LLR_CLAMP, LLR_CLAMP)
}
