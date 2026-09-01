//! Masking (§8.5).
//!
//! Two independent mechanisms, for two different failure modes.
//!
//! **Luma masking** is the familiar QR-style search: eight candidate patterns, XORed
//! into the payload modules, scored by a penalty function, best one signalled in the
//! format region. Its job is to stop a payload that happens to be mostly zeros from
//! rendering as a large flat region or, worse, as something the finder detector will
//! mistake for a finder.
//!
//! **Chroma masking** is unconditional and unsignalled: hue index rotated by
//! `(cx + cy) mod k`. There is no search and nothing to signal, which is the point —
//! adding chroma to the mask search would multiply the encoder's work by `k` and add
//! bits to a format region that §6.1 already flags as expensive at low versions. A fixed
//! rotation is enough to break up monochromatic regions, which is all chroma masking
//! needs to do.
//!
//! ## Pinned: chroma rotation is indexed by *cell*, not module
//!
//! §8.5 writes the rotation as `(x + y) mod k` without saying whether `x, y` are module
//! or chroma-cell coordinates. For `C1` they coincide. For `C2`/`C3`, where a cell is
//! 2×2, module coordinates would give `(2·cx + 2·cy) mod 2 = 0` for every cell — the
//! mask would do nothing at all on exactly the profiles that most need it. Cell
//! coordinates are therefore normative here.

use crate::grid::ModuleGrid;
use crate::layout::Layout;

/// Number of luma mask patterns (§8.5).
pub const MASK_COUNT: u8 = 8;

/// Evaluate luma mask `id` at module `(x, y)`. `true` means invert the luma bit.
///
/// The eight patterns are QR's, which is deliberate: they are known to decorrelate well
/// against the finder run-ratio, and reusing them means the penalty heuristics tuned for
/// QR over three decades carry across unchanged.
#[inline]
pub fn luma_mask(id: u8, x: u16, y: u16) -> bool {
    let (x, y) = (x as u32, y as u32);
    match id & 7 {
        0 => (y + x) % 2 == 0,
        1 => y % 2 == 0,
        2 => x % 3 == 0,
        3 => (y + x) % 3 == 0,
        4 => (y / 2 + x / 3) % 2 == 0,
        5 => (y * x) % 2 + (y * x) % 3 == 0,
        6 => ((y * x) % 2 + (y * x) % 3) % 2 == 0,
        _ => ((y + x) % 2 + (y * x) % 3) % 2 == 0,
    }
}

/// Rotation applied to a chroma symbol in cell `(cx, cy)` (§8.5).
#[inline]
pub const fn chroma_rotation(cx: u16, cy: u16, hues: usize) -> usize {
    (cx as usize + cy as usize) % hues
}

/// Apply the chroma mask to a hue symbol.
#[inline]
pub const fn chroma_mask_apply(hue: usize, cx: u16, cy: u16, hues: usize) -> usize {
    (hue + chroma_rotation(cx, cy, hues)) % hues
}

/// Remove the chroma mask from a hue symbol.
#[inline]
pub const fn chroma_mask_remove(hue: usize, cx: u16, cy: u16, hues: usize) -> usize {
    (hue + hues - chroma_rotation(cx, cy, hues)) % hues
}

/// Penalty weights, matching QR's N1..N4.
const N1: u32 = 3;
const N2: u32 = 3;
const N3: u32 = 40;
const N4: u32 = 10;

/// Score a fully-rendered luma grid; lower is better.
///
/// `luma[(x, y)]` is `true` for a light (L1) module. Fixed patterns are included in the
/// score because the detector sees them too — a payload run that continues a timing
/// pattern is exactly as confusing as one that does not.
pub fn penalty(luma: &ModuleGrid<bool>) -> u32 {
    let side = luma.side();
    let mut score = 0u32;

    // N1: runs of five or more same-coloured modules in a row or column.
    for line in 0..side {
        for horizontal in [true, false] {
            let mut run = 1u32;
            let mut prev = if horizontal { *luma.at(0, line) } else { *luma.at(line, 0) };
            for i in 1..side {
                let v = if horizontal { *luma.at(i, line) } else { *luma.at(line, i) };
                if v == prev {
                    run += 1;
                } else {
                    if run >= 5 {
                        score += N1 + (run - 5);
                    }
                    prev = v;
                    run = 1;
                }
            }
            if run >= 5 {
                score += N1 + (run - 5);
            }
        }
    }

    // N2: 2×2 blocks of one colour.
    for y in 0..side - 1 {
        for x in 0..side - 1 {
            let a = *luma.at(x, y);
            if a == *luma.at(x + 1, y) && a == *luma.at(x, y + 1) && a == *luma.at(x + 1, y + 1) {
                score += N2;
            }
        }
    }

    // N3: 1:1:3:1:1 finder-like sequences with four modules of quiet on either side.
    const PATTERN: [bool; 7] = [false, true, false, false, false, true, false];
    const QUIET: [bool; 4] = [true, true, true, true];
    for line in 0..side {
        for horizontal in [true, false] {
            let get = |i: u16| if horizontal { *luma.at(i, line) } else { *luma.at(line, i) };
            for start in 0..side {
                let matches = |offset: i32, pat: &[bool]| {
                    pat.iter().enumerate().all(|(j, &want)| {
                        let i = start as i32 + offset + j as i32;
                        // Outside the symbol counts as quiet zone, which is light.
                        let v = if i < 0 || i >= side as i32 { true } else { get(i as u16) };
                        v == want
                    })
                };
                if matches(0, &PATTERN) && (matches(-4, &QUIET) || matches(7, &QUIET)) {
                    score += N3;
                }
            }
        }
    }

    // N4: deviation from an even dark/light split, in 5% steps.
    let dark = luma.cells().iter().filter(|v| !**v).count();
    let total = luma.cells().len();
    let pct = (dark * 100) / total;
    let k = (pct as i32 - 50).abs() / 5;
    score += N4 * k as u32;

    score
}

/// Apply luma mask `id` to the payload modules of a rendered luma grid, in place.
pub fn apply_luma_mask(luma: &mut ModuleGrid<bool>, layout: &Layout, id: u8) {
    for &(x, y) in &layout.payload {
        if luma_mask(id, x, y) {
            let v = *luma.at(x, y);
            luma.set(x, y, !v);
        }
    }
}

/// Choose the lowest-penalty mask by rendering all eight.
///
/// `render` receives a mask id and returns the fully-rendered luma grid for it.
pub fn choose_mask(mut render: impl FnMut(u8) -> ModuleGrid<bool>) -> (u8, u32) {
    let mut best = (0u8, u32::MAX);
    for id in 0..MASK_COUNT {
        let s = penalty(&render(id));
        if s < best.1 {
            best = (id, s);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::{Profile, Version};

    #[test]
    fn masks_are_distinct() {
        for a in 0..MASK_COUNT {
            for b in (a + 1)..MASK_COUNT {
                let differs = (0..40u16)
                    .flat_map(|y| (0..40u16).map(move |x| (x, y)))
                    .any(|(x, y)| luma_mask(a, x, y) != luma_mask(b, x, y));
                assert!(differs, "masks {a} and {b} are identical");
            }
        }
    }

    #[test]
    fn masks_are_roughly_balanced() {
        for id in 0..MASK_COUNT {
            let set = (0..48u16)
                .flat_map(|y| (0..48u16).map(move |x| (x, y)))
                .filter(|&(x, y)| luma_mask(id, x, y))
                .count();
            let frac = set as f32 / (48.0 * 48.0);
            assert!((0.3..0.7).contains(&frac), "mask {id} sets {frac} of modules");
        }
    }

    #[test]
    fn masking_is_an_involution() {
        let l = Layout::new(Version::new(4).unwrap(), Profile::C1);
        let mut g = ModuleGrid::new(l.side, false);
        for (i, &(x, y)) in l.payload.iter().enumerate() {
            g.set(x, y, i % 3 == 0);
        }
        let orig = g.clone();
        apply_luma_mask(&mut g, &l, 5);
        assert_ne!(g, orig);
        apply_luma_mask(&mut g, &l, 5);
        assert_eq!(g, orig);
    }

    #[test]
    fn chroma_mask_roundtrips() {
        for hues in [2usize, 4] {
            for cy in 0..8u16 {
                for cx in 0..8u16 {
                    for h in 0..hues {
                        let m = chroma_mask_apply(h, cx, cy, hues);
                        assert!(m < hues);
                        assert_eq!(chroma_mask_remove(m, cx, cy, hues), h);
                    }
                }
            }
        }
    }

    /// The failure the pinned cell-coordinate rule exists to prevent.
    #[test]
    fn chroma_mask_is_not_a_no_op_on_two_by_two_profiles() {
        let hues = 2;
        let rotations: alloc::vec::Vec<usize> =
            (0..4u16).map(|cx| chroma_rotation(cx, 0, hues)).collect();
        assert_eq!(rotations, alloc::vec![0, 1, 0, 1]);
    }

    #[test]
    fn penalty_punishes_flat_and_striped_grids() {
        let flat = ModuleGrid::new(21u16, true);
        let mut checker = ModuleGrid::new(21u16, true);
        for y in 0..21u16 {
            for x in 0..21u16 {
                checker.set(x, y, (x + y) % 2 == 0);
            }
        }
        assert!(penalty(&flat) > penalty(&checker), "flat grid must score worse");
    }

    #[test]
    fn penalty_detects_a_finder_like_run() {
        let mut g = ModuleGrid::new(21u16, true);
        // dark, light, dark×3, light, dark preceded by four light modules.
        for (i, &dark) in [true, false, true, true, true, false, true].iter().enumerate() {
            g.set(5 + i as u16, 10, !dark);
        }
        assert!(penalty(&g) >= N3, "finder-like run not penalised");
    }

    #[test]
    fn choose_mask_returns_the_best_of_eight() {
        let l = Layout::new(Version::new(3).unwrap(), Profile::C1);
        let (id, score) = choose_mask(|m| {
            let mut g = ModuleGrid::new(l.side, false);
            for &(x, y) in &l.payload {
                g.set(x, y, luma_mask(m, x, y));
            }
            g
        });
        assert!(id < MASK_COUNT);
        for m in 0..MASK_COUNT {
            let mut g = ModuleGrid::new(l.side, false);
            for &(x, y) in &l.payload {
                g.set(x, y, luma_mask(m, x, y));
            }
            assert!(penalty(&g) >= score);
        }
    }
}
