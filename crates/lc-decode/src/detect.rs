//! Finder and alignment detection (§12.1).
//!
//! > *Do all of this before touching colour.* The finders are luma-only precisely so this
//! > stage cannot be blocked by a white-balance failure. — §12.1
//!
//! Everything in this module works on the binarised luma image and never looks at a
//! colour channel. That is a correctness property, not an optimisation: a symbol captured
//! under a strong colour cast, or through a monochrome sensor, must still reach the point
//! where the format region can be read and the fallback path chosen.
//!
//! The 1:1:3:1:1 run-ratio scan and the concentric bullseye are adopted from QR
//! deliberately (Appendix B) — they are long out of patent, they are the best-tested
//! detection primitive in the field, and reusing them means an implementer can port a
//! working QR detector rather than write one.

use crate::image::Binary;
use crate::transform::Transform;
use alloc::vec::Vec;
use lc_core::Version;

/// A located finder pattern.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Finder {
    /// Centre x, in pixels.
    pub x: f32,
    /// Centre y, in pixels.
    pub y: f32,
    /// Estimated module pitch at this finder, in pixels.
    pub module: f32,
    /// How many independent scans agreed on this centre.
    pub confirmations: u32,
}

/// The output of detection: three ordered finders and an optional alignment marker.
#[derive(Clone, Debug)]
pub struct Detection {
    /// Top-left, top-right, bottom-left, in that order.
    pub finders: [Finder; 3],
    /// Alignment marker centre in pixels, when one was found.
    pub alignment: Option<(f32, f32)>,
    /// Version implied by the geometry. Confirmed or corrected by the format region.
    pub version: Version,
    /// Module space → pixel space.
    pub transform: Transform,
}

/// Ratio tolerance for a run-length match, as a fraction of the module estimate.
const TOLERANCE: f32 = 0.55;

/// Does a five-run sequence match 1:1:3:1:1?
///
/// The module estimate comes from the four **outer** runs, and the centre is then tested
/// against it. Normalising by `total / 7` instead — the obvious formulation — lets a
/// centre run of the wrong length drag the estimate towards itself, and a uniform
/// `1:1:1:1:1` sequence then satisfies the ratio test outright: five equal runs of `r`
/// give `module = 5r/7`, and the centre check `|r − 3·(5r/7)| = 8r/7` clears a limit of
/// `3·(5r/7)·TOLERANCE = 15r·TOLERANCE/7` for any tolerance above 8/15 ≈ 0.533. At 0.55
/// that margin was 3%, so a module-pitch checkerboard — which is exactly what masking an
/// all-zero payload with mask 0 produces — registered as a finder on every scan line.
fn matches_finder(runs: [u32; 5]) -> Option<f32> {
    let total: u32 = runs.iter().sum();
    if total < 7 {
        return None;
    }
    // Outer runs are the 1s of 1:1:3:1:1, so their mean is the module estimate.
    let outer = (runs[0] + runs[1] + runs[3] + runs[4]) as f32 / 4.0;
    if outer <= 0.0 {
        return None;
    }
    let tol = outer * TOLERANCE;
    for r in [runs[0], runs[1], runs[3], runs[4]] {
        if (r as f32 - outer).abs() > tol {
            return None;
        }
    }
    // The centre is three modules wide, so it gets three modules' worth of tolerance.
    if (runs[2] as f32 - 3.0 * outer).abs() > 3.0 * tol {
        return None;
    }
    Some(total as f32 / 7.0)
}

/// Does a three-run sequence match 1:1:1?
fn matches_alignment(runs: [u32; 3], module: f32) -> bool {
    let tol = module * 0.7;
    runs.iter().all(|r| (*r as f32 - module).abs() <= tol)
}

/// Scan one line of the binary image, calling `hit` with the centre coordinate and module
/// estimate of every 1:1:3:1:1 match.
///
/// A five-state run counter over `dark, light, dark, light, dark`. When a dark run ends
/// while five states are filled, the window is tested; either way the counts shift back
/// by two so an overlapping match starting inside this one is still found. Lines are
/// treated as bounded by light, so a symbol touching the frame edge still matches — the
/// quiet zone may simply be off-screen.
fn scan_line(len: u32, read: impl Fn(u32) -> bool, mut hit: impl FnMut(f32, f32)) {
    let mut runs = [0u32; 5];
    let mut state = 0usize; // even = counting dark, odd = counting light

    // Shift the window back by two states, keeping the trailing light run.
    fn shift(runs: &mut [u32; 5]) {
        runs[0] = runs[2];
        runs[1] = runs[3];
        runs[2] = runs[4];
        runs[3] = 1;
        runs[4] = 0;
    }

    for i in 0..=len {
        // One past the end counts as light, closing any open dark run.
        let dark = i < len && read(i);
        if dark {
            if state % 2 == 1 {
                state += 1;
            }
            runs[state] += 1;
        } else if state % 2 == 1 {
            runs[state] += 1;
        } else if state == 4 {
            if let Some(module) = matches_finder(runs) {
                let centre = i as f32 - runs[4] as f32 - runs[3] as f32 - runs[2] as f32 / 2.0;
                hit(centre, module);
            }
            shift(&mut runs);
            state = 3;
        } else {
            state += 1;
            runs[state] += 1;
        }
    }
}

/// Count how far a run of `want` darkness extends from `(x, y)` in direction `step`.
fn run_length(bin: &Binary, x: u32, y: u32, step: i32, want: bool, limit: u32) -> u32 {
    let mut n = 0;
    let mut cy = y as i32;
    while n < limit {
        cy += step;
        if cy < 0 || cy as u32 >= bin.height || bin.dark(x, cy as u32) != want {
            break;
        }
        n += 1;
    }
    n
}

/// Vertical re-scan through a candidate centre, refining `y` and confirming the pattern.
///
/// A horizontal 1:1:3:1:1 hit is cheap and common — payload noise produces them
/// constantly. Requiring the same ratio vertically through the same point is what makes
/// the candidate list short enough for the triple search that follows.
fn confirm_vertical(bin: &Binary, cx: u32, cy: u32, module: f32) -> Option<f32> {
    if cx >= bin.width || cy >= bin.height || !bin.dark(cx, cy) {
        return None;
    }
    let limit = (module * 5.0) as u32 + 4;

    let up = run_length(bin, cx, cy, -1, true, limit);
    let down = run_length(bin, cx, cy, 1, true, limit);
    let core = up + down + 1;
    let top = cy - up;
    let bottom = cy + down;

    let light_up = run_length(bin, cx, top, -1, false, limit);
    let dark_up = run_length(bin, cx, top - light_up.min(top), -1, true, limit);
    let light_dn = run_length(bin, cx, bottom, 1, false, limit);
    let dark_dn = run_length(bin, cx, bottom + light_dn, 1, true, limit);

    matches_finder([dark_up, light_up, core, light_dn, dark_dn])?;
    Some(top as f32 + core as f32 / 2.0)
}

/// Horizontal re-scan through a confirmed centre, refining `x`.
fn confirm_horizontal(bin: &Binary, cx: u32, cy: u32, module: f32) -> Option<f32> {
    if cx >= bin.width || cy >= bin.height || !bin.dark(cx, cy) {
        return None;
    }
    let limit = (module * 5.0) as u32 + 4;
    let row = |x: u32| bin.dark(x, cy);

    let mut left = 0u32;
    while left < limit && cx > left && row(cx - left - 1) {
        left += 1;
    }
    let mut right = 0u32;
    while right < limit && cx + right + 1 < bin.width && row(cx + right + 1) {
        right += 1;
    }
    let core = left + right + 1;
    let (l, r) = (cx - left, cx + right);

    let mut light_l = 0u32;
    while light_l < limit && l > light_l && !row(l - light_l - 1) {
        light_l += 1;
    }
    let mut dark_l = 0u32;
    while dark_l < limit && l > light_l + dark_l && row(l - light_l - dark_l - 1) {
        dark_l += 1;
    }
    let mut light_r = 0u32;
    while light_r < limit && r + light_r + 1 < bin.width && !row(r + light_r + 1) {
        light_r += 1;
    }
    let mut dark_r = 0u32;
    while dark_r < limit && r + light_r + dark_r + 1 < bin.width && row(r + light_r + dark_r + 1) {
        dark_r += 1;
    }

    matches_finder([dark_l, light_l, core, light_r, dark_r])?;
    Some(l as f32 + core as f32 / 2.0)
}

/// Locate every plausible finder pattern.
pub fn find_finders(bin: &Binary) -> Vec<Finder> {
    let mut found: Vec<Finder> = Vec::new();
    // Skipping lines costs nothing: a finder is at least 7 modules across, so even a
    // coarse stride crosses every one of them several times. Two is safe down to the
    // smallest module pitch §13.1 permits.
    for row in (0..bin.height).step_by(2) {
        scan_line(
            bin.width,
            |x| bin.dark(x, row),
            |cx, module| {
                let Some(cy) = confirm_vertical(bin, cx as u32, row, module) else {
                    return;
                };
                let Some(cx) = confirm_horizontal(bin, cx as u32, cy as u32, module) else {
                    return;
                };
                merge(&mut found, cx, cy, module);
            },
        );
    }
    // And again down the columns. A run-length scan is not rotation invariant: under
    // strong skew a finder's rows can be broken while its columns are clean, or the other
    // way round, and a symbol captured at 45° has neither axis aligned to its modules.
    // Scanning both axes roughly doubles the confirmation count for every real finder and
    // recovers the ones a single axis loses entirely.
    for col in (0..bin.width).step_by(2) {
        scan_line(
            bin.height,
            |y| bin.dark(col, y),
            |cy, module| {
                let Some(cx) = confirm_horizontal(bin, col, cy as u32, module) else {
                    return;
                };
                let Some(cy) = confirm_vertical(bin, cx as u32, cy as u32, module) else {
                    return;
                };
                merge(&mut found, cx, cy, module);
            },
        );
    }
    found.retain(|f| f.confirmations >= 2);
    // Most-confirmed first, ties kept in discovery order. A hand-written insertion sort rather
    // than `sort_by_key`: identical order, and the stable sort that call instantiated for
    // `Finder` was 3.4 KB of scanner.
    for i in 1..found.len() {
        let mut j = i;
        while j > 0 && found[j - 1].confirmations < found[j].confirmations {
            found.swap(j - 1, j);
            j -= 1;
        }
    }
    found
}

/// Fold a new observation into the candidate list, or add it.
fn merge(found: &mut Vec<Finder>, x: f32, y: f32, module: f32) {
    for f in found.iter_mut() {
        if (f.x - x).abs() <= f.module && (f.y - y).abs() <= f.module {
            let n = f.confirmations as f32;
            f.x = (f.x * n + x) / (n + 1.0);
            f.y = (f.y * n + y) / (n + 1.0);
            f.module = (f.module * n + module) / (n + 1.0);
            f.confirmations += 1;
            return;
        }
    }
    found.push(Finder {
        x,
        y,
        module,
        confirmations: 1,
    });
}

/// Order three finders as top-left, top-right, bottom-left.
///
/// The top-left finder is the one opposite the longest side — the corner of the right
/// angle. Which of the remaining two is top-right then follows from the sign of the cross
/// product in image coordinates, where `y` grows downward.
pub fn order_finders(a: Finder, b: Finder, c: Finder) -> [Finder; 3] {
    let d = |p: Finder, q: Finder| (p.x - q.x).powi(2) + (p.y - q.y).powi(2);
    let (ab, bc, ca) = (d(a, b), d(b, c), d(c, a));
    // The corner is the point not on the longest side.
    let (corner, p, q) = if ab >= bc && ab >= ca {
        (c, a, b)
    } else if bc >= ab && bc >= ca {
        (a, b, c)
    } else {
        (b, c, a)
    };
    let cross = (p.x - corner.x) * (q.y - corner.y) - (p.y - corner.y) * (q.x - corner.x);
    if cross > 0.0 {
        [corner, p, q]
    } else {
        [corner, q, p]
    }
}

/// Version implied by the distance between the top-left and top-right finders.
///
/// Their centres are `side − 7` modules apart, so the module pitch measured at the
/// finders converts the pixel distance straight into a side length. Rounded to the
/// nearest legal `side`, and always cross-checked against the format region (§12.2)
/// before anything is decoded — this is a starting point, not an answer.
pub fn estimate_version(finders: &[Finder; 3]) -> Option<Version> {
    let [tl, tr, bl] = *finders;
    if tl.module <= 0.0 || tr.module <= 0.0 || bl.module <= 0.0 {
        return None;
    }
    // Measure each edge against the pitch of *its own two finders*, not a global mean.
    // Under perspective the near and far edges of a symbol have genuinely different
    // module pitches, and averaging all three biases whichever edge is shortest.
    let edge = |a: Finder, b: Finder| {
        let d = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        d / ((a.module + b.module) / 2.0)
    };
    let side = (edge(tl, tr) + edge(tl, bl)) / 2.0 + 7.0;
    let v = ((side - 21.0) / 4.0).round() as i32 + 1;
    Version::new(v.clamp(1, 32) as u8).ok()
}

/// Search for the alignment marker near a predicted position.
///
/// Returns the refined centre, or `None` if no 1:1:1 bullseye is found in the window.
/// Failing to find it is routine — v1 has no alignment marker at all, and a specular blob
/// can swallow one on any version — so the caller must be able to proceed without it.
pub fn find_alignment(bin: &Binary, predicted: (f32, f32), module: f32) -> Option<(f32, f32)> {
    // Five modules, not three. The first prediction comes from an *affine* fit, and the
    // whole reason to want the marker is that an affine fit is wrong under perspective —
    // so the search window has to be wide enough to cover the error it is there to
    // correct. [`refine_alignment`] then re-searches from the homography, where the
    // prediction is much better.
    let radius = (module * 5.0).max(6.0) as i32;
    let (px, py) = (predicted.0 as i32, predicted.1 as i32);
    let mut best: Option<((f32, f32), f32)> = None;

    for dy in -radius..=radius {
        let y = py + dy;
        if y < 0 || y as u32 >= bin.height {
            continue;
        }
        let y = y as u32;
        // Walk the row looking for light-dark-light with all three runs ≈ one module.
        let x0 = (px - radius).max(0) as u32;
        let x1 = ((px + radius) as u32).min(bin.width - 1);
        let mut x = x0;
        while x < x1 {
            if bin.dark(x, y) {
                x += 1;
                continue;
            }
            let mut light1 = 0u32;
            while x < x1 && !bin.dark(x, y) {
                x += 1;
                light1 += 1;
            }
            let dark_start = x;
            let mut dark = 0u32;
            while x < x1 && bin.dark(x, y) {
                x += 1;
                dark += 1;
            }
            let mut light2 = 0u32;
            let mut probe = x;
            while probe < x1 && !bin.dark(probe, y) {
                probe += 1;
                light2 += 1;
            }
            if dark > 0 && matches_alignment([light1, dark, light2], module) {
                let cx = dark_start as f32 + dark as f32 / 2.0;
                if let Some(cy) = confirm_alignment_vertical(bin, cx as u32, y, module) {
                    let err = (cx - predicted.0).powi(2) + (cy - predicted.1).powi(2);
                    if best.is_none_or(|(_, e)| err < e) {
                        best = Some(((cx, cy), err));
                    }
                }
            }
        }
    }
    best.map(|(c, _)| c)
}

fn confirm_alignment_vertical(bin: &Binary, cx: u32, cy: u32, module: f32) -> Option<f32> {
    if !bin.dark(cx, cy) {
        return None;
    }
    let limit = (module * 3.0) as u32 + 3;
    let mut up = 0u32;
    let mut y = cy;
    while y > 0 && bin.dark(cx, y - 1) && up < limit {
        y -= 1;
        up += 1;
    }
    let top = y;
    let mut down = 0u32;
    let mut y = cy;
    while y + 1 < bin.height && bin.dark(cx, y + 1) && down < limit {
        y += 1;
        down += 1;
    }
    let dark = up + down + 1;
    let mut light_up = 0u32;
    let mut y = top;
    while y > 0 && !bin.dark(cx, y - 1) && light_up < limit {
        y -= 1;
        light_up += 1;
    }
    let mut light_dn = 0u32;
    let mut y = top + dark - 1;
    while y + 1 < bin.height && !bin.dark(cx, y + 1) && light_dn < limit {
        y += 1;
        light_dn += 1;
    }
    if matches_alignment([light_up, dark, light_dn], module) {
        Some(top as f32 + dark as f32 / 2.0)
    } else {
        None
    }
}

/// Locate the alignment marker by matching its 5×5 pattern **through the current
/// transform**, rather than by run-scanning image rows.
///
/// This is the rotation- and perspective-invariant way to do it, and the reason it
/// matters is that the marker is only needed *because* the capture is projective. A
/// row-scan asks "is there a light-dark-light run of about one module along this image
/// row", which is a question about the sensor's axes, not the symbol's; at 30° of rotation
/// a horizontal line crosses a 5×5 bullseye diagonally and the run lengths stop matching.
/// Measured, the run-scan version lost the marker whenever tilt and rotation were combined,
/// and with it the homography — so tilt-with-rotation failed at magnitudes that either
/// alone survived comfortably.
///
/// Sampling the 25 module centres through the transform asks the right question instead:
/// "does the symbol look like a bullseye *here*", in the symbol's own coordinates. The
/// search is coarse-to-fine over sub-module offsets, and returns `None` unless at least
/// 23 of the 25 modules agree — a glare blob over the marker should lose it, not
/// hallucinate it somewhere else.
pub fn match_alignment(bin: &Binary, t: &Transform, version: Version) -> Option<(f32, f32)> {
    if version.get() < 2 {
        return None;
    }
    let side = version.side() as f32;
    // Marker centre in module coordinates; the 5×5 spans ±2 modules about it.
    let (cx, cy) = (side - 6.5, side - 6.5);

    // Majority of five taps per module rather than a single pixel. One pixel makes the
    // score noisy enough that a chance pattern in the payload can outscore the real
    // marker, and a wrong lock corrupts the homography for the whole symbol — worse than
    // no marker at all, because the affine fallback is at least unbiased.
    let module_is_dark = |mx: f32, my: f32| -> bool {
        let mut dark = 0i32;
        let mut seen = 0i32;
        for (ox, oy) in [
            (0.0, 0.0),
            (-0.25, 0.0),
            (0.25, 0.0),
            (0.0, -0.25),
            (0.0, 0.25),
        ] {
            let (px, py) = t.apply(mx + ox, my + oy);
            if px < 0.0 || py < 0.0 || px >= bin.width as f32 || py >= bin.height as f32 {
                continue;
            }
            seen += 1;
            dark += bin.dark(px as u32, py as u32) as i32;
        }
        seen > 0 && dark * 2 > seen
    };

    let score_at = |ox: f32, oy: f32| -> u32 {
        let mut hits = 0;
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let want_dark = dx.abs().max(dy.abs()) != 1;
                if module_is_dark(cx + ox + dx as f32, cy + oy + dy as f32) == want_dark {
                    hits += 1;
                }
            }
        }
        hits
    };

    // Coarse sweep over ±3 modules, then refine. Ties break toward the prediction: the
    // transform that seeded this search is approximately right, so the nearer of two
    // equally good matches is the real marker.
    let mut best = (0u32, f32::INFINITY, 0.0f32, 0.0f32);
    let mut step = 0.5f32;
    let mut range = 3.0f32;
    let (mut bx, mut by) = (0.0f32, 0.0f32);
    for _ in 0..3 {
        let n = (range / step).round() as i32;
        for iy in -n..=n {
            for ix in -n..=n {
                let (ox, oy) = (bx + ix as f32 * step, by + iy as f32 * step);
                let s = score_at(ox, oy);
                let d = ox * ox + oy * oy;
                if s > best.0 || (s == best.0 && d < best.1) {
                    best = (s, d, ox, oy);
                }
            }
        }
        bx = best.2;
        by = best.3;
        range = step;
        step /= 4.0;
    }

    // 23 of 25, not 21. A 5×5 bullseye is a common enough shape that payload noise
    // produces near-misses; the marker itself scores 25 on any capture clean enough for
    // the homography to be worth having.
    if best.0 < 23 {
        return None;
    }
    Some(t.apply(cx + best.2, cy + best.3))
}

/// Locate the alignment marker, template match first and run-scan as a fallback.
///
/// The two methods fail in different places, and measurably so. The template match
/// (§[`match_alignment`]) is rotation invariant and finds the marker at 45° of rotation
/// where the run-scan finds nothing at all; the run-scan ([`find_alignment`]) keeps
/// working at extreme tilt, where foreshortening squeezes the marker's modules until the
/// template's multi-tap sampling straddles their neighbours. Neither dominates, both are
/// cheap, and a wrong answer from either is caught by the format region's
/// self-consistency check (§12.2) — so try both.
pub fn locate_alignment(
    bin: &Binary,
    t: &Transform,
    version: Version,
    module: f32,
) -> Option<(f32, f32)> {
    if version.get() < 2 {
        return None;
    }
    match_alignment(bin, t, version).or_else(|| {
        let side = version.side() as f32;
        find_alignment(bin, t.apply(side - 6.5, side - 6.5), module)
    })
}

/// Re-find the alignment marker using the homography's own prediction, and rebuild.
///
/// One fixed-point iteration. The affine prediction that seeds the first search is, under
/// strong perspective, several modules off at the far corner — which is exactly where the
/// marker is. Once a homography exists its prediction is far better, so re-searching from
/// there either confirms the same marker or finds the right one. Cheap, and it is the
/// difference between tolerating the 10–20° off-axis capture §13.2 recommends and
/// tolerating rather more.
pub fn refine_alignment(
    bin: &Binary,
    finders: &[Finder; 3],
    version: Version,
    transform: Transform,
    module: f32,
) -> (Transform, Option<(f32, f32)>) {
    if version.get() < 2 {
        return (transform, None);
    }
    match locate_alignment(bin, &transform, version, module) {
        Some(a) => match build_transform(finders, Some(a), version) {
            Some(t) => (t, Some(a)),
            None => (transform, Some(a)),
        },
        None => (transform, None),
    }
}

/// Build the module→pixel transform for a candidate version.
///
/// Uses the alignment marker as the fourth correspondence when it is available, and falls
/// back to an affine fit from the three finders when it is not.
pub fn build_transform(
    finders: &[Finder; 3],
    alignment: Option<(f32, f32)>,
    version: Version,
) -> Option<Transform> {
    let side = version.side() as f32;
    let [tl, tr, bl] = *finders;
    let src3 = [(3.5, 3.5), (side - 3.5, 3.5), (3.5, side - 3.5)];
    let dst3 = [(tl.x, tl.y), (tr.x, tr.y), (bl.x, bl.y)];
    match alignment {
        Some(a) if version.get() >= 2 => {
            // The v≥2 marker sits at (side−7, side−7); its centre module is offset by 0.5.
            let src = [src3[0], src3[1], src3[2], (side - 6.5, side - 6.5)];
            let dst = [dst3[0], dst3[1], dst3[2], a];
            Transform::from_quad(src, dst).or_else(|| Transform::from_triangle(src3, dst3))
        }
        _ => Transform::from_triangle(src3, dst3),
    }
}

/// Full detection: locate finders, order them, estimate the version, find the alignment
/// marker, and fit the transform.
pub fn detect(bin: &Binary) -> Option<Detection> {
    detect_candidates(bin).into_iter().next()
}

/// Every plausible symbol in the frame, best-scoring first.
///
/// [`detect`] returns only the winner, which is wrong whenever the frame holds **more than
/// one code**. The geometry cannot settle it alone: §6.2 adopts QR's 1:1:3:1:1 finder
/// deliberately (Appendix B), so an LC symbol beside a QR — a §17.2 dual-render, say —
/// presents six finder-like patterns at one module pitch, and a triple straddling the two
/// symbols can score better on right-angleness than either real triple. The same happens
/// with two LC symbols in one frame, which is an ordinary thing to point a camera at.
///
/// So candidate selection is not the place to decide. The **format region is**: it is BCH
/// protected, so a wrong triple almost never yields a format word that decodes, and the
/// caller can walk this list until one does. See `prepare` in `lib.rs`.
pub fn detect_candidates(bin: &Binary) -> Vec<Detection> {
    candidates_from(bin, find_finders(bin))
}

/// Build candidate detections from an already-located set of finders.
///
/// Split out from [`detect_candidates`] so the finder search can run on a **downscaled**
/// image while everything after it — the alignment search, the transform — runs at full
/// resolution against `bin`. See `PYRAMID_LEVELS` in `lib.rs` for why that matters: the
/// run-ratio scan has an upper limit on module pitch, and reducing the image is what
/// brings an over-large symbol back into the band where it works.
pub fn candidates_from(bin: &Binary, candidates: Vec<Finder>) -> Vec<Detection> {
    if candidates.len() < 3 {
        return Vec::new();
    }

    // Keep the best few by insertion into a fixed array, rather than collecting every
    // triple and sorting. Not premature: the obvious `Vec` + `sort_by` + iterator-chain
    // version cost **10.4 KB** in the scanner and pushed it over §14.2's budget, which
    // `just wasm-size` duly failed on. A bounded insertion needs no sort, no growth and no
    // iterator machinery, and the array is three entries long.
    let mut best: [Option<(f32, [Finder; 3])>; MAX_CANDIDATES] = [None; MAX_CANDIDATES];
    let n = candidates.len().min(8);
    for i in 0..n {
        for j in (i + 1)..n {
            for k in (j + 1)..n {
                let (a, b, c) = (candidates[i], candidates[j], candidates[k]);
                // A false positive in payload noise almost never matches the real
                // finders' pitch.
                let mean = (a.module + b.module + c.module) / 3.0;
                let spread = [a, b, c]
                    .iter()
                    .map(|f| (f.module - mean).abs())
                    .fold(0.0, f32::max);
                if spread > mean * 0.5 {
                    continue;
                }
                let ordered = order_finders(a, b, c);
                let Some(score) = right_angle_score(&ordered) else {
                    continue;
                };
                insert_ranked(&mut best, score, ordered);
            }
        }
    }

    let mut out = Vec::new();
    for entry in best.iter() {
        let Some((_, finders)) = entry else { continue };
        if let Some(d) = build_detection(bin, *finders) {
            out.push(d);
        }
    }
    out
}

/// Cap on candidates carried forward.
///
/// Every one costs an alignment search here and, for `Decoder::decode`, a format read and
/// possibly a full LDPC pass — and that cost lands on frames that *fail*, which in a live
/// preview is every frame the user is still aiming. Six doubled the §15 degradation
/// suite's runtime, 33.5 s → 67.2 s; three recovers all of it and still fixes the
/// two-symbol case, because a dual-render's two real triples both score well and a
/// straddling triple rarely outranks both.
const MAX_CANDIDATES: usize = 3;

/// Insert into a descending-quality fixed array, dropping the worst. Lower score is better.
fn insert_ranked(
    best: &mut [Option<(f32, [Finder; 3])>; MAX_CANDIDATES],
    score: f32,
    f: [Finder; 3],
) {
    for idx in 0..MAX_CANDIDATES {
        if let Some((existing, _)) = best[idx]
            && score >= existing
        {
            continue;
        }
        let mut back = MAX_CANDIDATES - 1;
        while back > idx {
            best[back] = best[back - 1];
            back -= 1;
        }
        best[idx] = Some((score, f));
        return;
    }
}

fn build_detection(bin: &Binary, finders: [Finder; 3]) -> Option<Detection> {
    let version = estimate_version(&finders)?;
    let module = (finders[0].module + finders[1].module + finders[2].module) / 3.0;

    // Predict the alignment marker with an affine fit, then look for it there.
    let rough = build_transform(&finders, None, version)?;
    let alignment = locate_alignment(bin, &rough, version, module);
    let transform = build_transform(&finders, alignment, version)?;
    // One refinement pass: the affine seed is several modules off at the far corner under
    // strong perspective, which is exactly where the marker lives.
    let (transform, alignment) = refine_alignment(bin, &finders, version, transform, module);
    Some(Detection {
        finders,
        alignment,
        version,
        transform,
    })
}

/// How far the finder triangle is from a right isosceles one. Lower is better.
fn right_angle_score(f: &[Finder; 3]) -> Option<f32> {
    let [tl, tr, bl] = *f;
    let v1 = (tr.x - tl.x, tr.y - tl.y);
    let v2 = (bl.x - tl.x, bl.y - tl.y);
    let n1 = (v1.0 * v1.0 + v1.1 * v1.1).sqrt();
    let n2 = (v2.0 * v2.0 + v2.1 * v2.1).sqrt();
    if n1 < 1.0 || n2 < 1.0 {
        return None;
    }
    let cos = (v1.0 * v2.0 + v1.1 * v2.1) / (n1 * n2);
    // Perspective can skew the right angle, but not past about 45°.
    if cos.abs() > 0.7 {
        return None;
    }
    let ratio = (n1 / n2).max(n2 / n1);
    if ratio > 1.6 {
        return None;
    }
    Some(cos.abs() + (ratio - 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ratio_accepts_ideal_and_scaled_finders() {
        assert!(matches_finder([1, 1, 3, 1, 1]).is_some());
        assert!(matches_finder([10, 10, 30, 10, 10]).is_some());
        assert_eq!(matches_finder([10, 10, 30, 10, 10]).unwrap(), 10.0);
        // Mild distortion still matches — capture is never exact.
        assert!(matches_finder([9, 11, 29, 10, 11]).is_some());
    }

    #[test]
    fn run_ratio_rejects_non_finders() {
        assert!(matches_finder([1, 1, 1, 1, 1]).is_none());
        assert!(matches_finder([1, 1, 9, 1, 1]).is_none());
        assert!(matches_finder([5, 1, 3, 1, 1]).is_none());
        assert!(matches_finder([0, 0, 0, 0, 0]).is_none());
    }

    /// A uniform run sequence is what a module-pitch checkerboard presents to the scan
    /// line, and it must not read as 1:1:3:1:1 at any scale.
    ///
    /// `[1, 1, 1, 1, 1]` above does not cover this: it is rejected by the `total < 7`
    /// size guard before the ratio test runs at all. Scaled up past that guard, the
    /// original `total / 7` normalisation accepted every one of these.
    #[test]
    fn run_ratio_rejects_a_uniform_checkerboard_at_every_scale() {
        for r in 2..40u32 {
            assert!(
                matches_finder([r, r, r, r, r]).is_none(),
                "uniform runs of {r} matched 1:1:3:1:1"
            );
        }
    }

    #[test]
    fn ordering_puts_the_right_angle_first() {
        let f = |x: f32, y: f32| Finder {
            x,
            y,
            module: 4.0,
            confirmations: 3,
        };
        // Unrotated.
        let [tl, tr, bl] = order_finders(f(100.0, 10.0), f(10.0, 100.0), f(10.0, 10.0));
        assert_eq!((tl.x, tl.y), (10.0, 10.0));
        assert_eq!((tr.x, tr.y), (100.0, 10.0));
        assert_eq!((bl.x, bl.y), (10.0, 100.0));
    }

    #[test]
    fn ordering_survives_rotation() {
        let f = |x: f32, y: f32| Finder {
            x,
            y,
            module: 4.0,
            confirmations: 3,
        };
        for turn in 0..4 {
            // Rotate the ideal layout by 90° `turn` times about the origin (y down).
            let rot = |x: f32, y: f32| {
                let (mut x, mut y) = (x, y);
                for _ in 0..turn {
                    let (nx, ny) = (-y, x);
                    x = nx;
                    y = ny;
                }
                (x + 500.0, y + 500.0)
            };
            let tl = rot(0.0, 0.0);
            let tr = rot(90.0, 0.0);
            let bl = rot(0.0, 90.0);
            let got = order_finders(f(tr.0, tr.1), f(bl.0, bl.1), f(tl.0, tl.1));
            assert!(
                (got[0].x - tl.0).abs() < 1e-3 && (got[0].y - tl.1).abs() < 1e-3,
                "turn {turn}"
            );
            assert!((got[1].x - tr.0).abs() < 1e-3, "turn {turn} TR");
            assert!((got[2].x - bl.0).abs() < 1e-3, "turn {turn} BL");
        }
    }

    #[test]
    fn version_estimate_inverts_the_geometry() {
        for v in Version::all() {
            let side = v.side() as f32;
            let module = 6.0f32;
            let f = |x: f32, y: f32| Finder {
                x,
                y,
                module,
                confirmations: 5,
            };
            let d = (side - 7.0) * module;
            let finders = [f(0.0, 0.0), f(d, 0.0), f(0.0, d)];
            assert_eq!(estimate_version(&finders), Some(v), "v{}", v.get());
        }
    }

    #[test]
    fn right_angle_score_rejects_implausible_triangles() {
        let f = |x: f32, y: f32| Finder {
            x,
            y,
            module: 4.0,
            confirmations: 3,
        };
        // Nearly collinear.
        assert!(right_angle_score(&[f(0.0, 0.0), f(100.0, 1.0), f(200.0, 2.0)]).is_none());
        // Wildly unequal sides.
        assert!(right_angle_score(&[f(0.0, 0.0), f(100.0, 0.0), f(0.0, 20.0)]).is_none());
        // Ideal.
        assert!(right_angle_score(&[f(0.0, 0.0), f(100.0, 0.0), f(0.0, 100.0)]).unwrap() < 0.01);
    }
}
