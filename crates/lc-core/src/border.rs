//! The screen reference border (§6.4).
//!
//! > Screen symbols MAY carry a 2-module-wide ring immediately outside the quiet zone
//! > cycling through the full palette plus white and black. — §6.4
//!
//! The border lives **outside** the symbol grid, so it is addressed in *module-space
//! coordinates that fall outside `0..side`* — negative, or past the far edge. That is
//! deliberate: the decoder's homography is fitted from the finders and extrapolates
//! perfectly well past the symbol edge, so the border needs no separate detection step.
//! It is simply more sample points for the §12.3 fit, placed exactly where a global fit
//! is weakest and where glare gradients are strongest.
//!
//! ## Pinned: the cycle and its indexing
//!
//! §6.4 fixes neither the colour order nor how a cell's position in the cycle is
//! determined. Pinned here:
//!
//! * The cycle is the palette in index order, then white, then black — `2k + 2` entries.
//! * A cell's index is its coordinate **along** the edge it lies on: `x` on the top and
//!   bottom runs, `y` on the left and right. A corner cell, which is outside on both
//!   axes, resolves to whichever axis it is further outside on, with ties going to `y`.
//! * Both radial layers take the same colour, so each entry presents a 2-module-deep
//!   target to the sampler instead of a 1-module sliver.

use crate::color::Srgb8;
use crate::palette::{BORDER_BLACK, BORDER_WHITE, Palette};

/// Quiet-zone width in modules (§6.1).
pub const QUIET: i32 = 2;
/// Reference-border width in modules (§6.4).
pub const WIDTH: i32 = 2;

/// How far outside the symbol a module-space coordinate lies. `0` means inside.
#[inline]
pub fn shell(x: i32, y: i32, side: u16) -> i32 {
    let d = |c: i32| {
        if c < 0 {
            -c
        } else if c >= side as i32 {
            c - side as i32 + 1
        } else {
            0
        }
    };
    d(x).max(d(y))
}

/// Number of entries in the border cycle for a palette of `colors` colours.
#[inline]
pub const fn cycle_len(colors: usize) -> usize {
    colors + 2
}

/// The `i`-th colour of the border cycle.
pub fn cycle_color(palette: &Palette, i: usize) -> Srgb8 {
    let n = palette.len();
    match i % cycle_len(n) {
        i if i < n => palette.colors[i],
        i if i == n => BORDER_WHITE,
        _ => BORDER_BLACK,
    }
}

/// The border-cycle index of a module-space coordinate, or `None` if it is not a border
/// cell.
pub fn cell_index(x: i32, y: i32, side: u16, colors: usize) -> Option<usize> {
    let d = shell(x, y, side);
    if !(QUIET + 1..=QUIET + WIDTH).contains(&d) {
        return None;
    }
    let s = side as i32;
    let out_x = x < 0 || x >= s;
    let out_y = y < 0 || y >= s;
    let depth = |c: i32| if c < 0 { -c } else if c >= s { c - s + 1 } else { 0 };
    let t = if out_y && !out_x {
        x
    } else if out_x && !out_y {
        y
    } else if depth(x) > depth(y) {
        x
    } else {
        y
    };
    Some(t.rem_euclid(cycle_len(colors) as i32) as usize)
}

/// The colour of a border cell, or `None` if the coordinate is not on the border.
pub fn cell_color(x: i32, y: i32, side: u16, palette: &Palette) -> Option<Srgb8> {
    cell_index(x, y, side, palette.len()).map(|i| cycle_color(palette, i))
}

/// Every border cell's module-space coordinate.
pub fn cells(side: u16) -> impl Iterator<Item = (i32, i32)> {
    let lo = -(QUIET + WIDTH);
    let hi = side as i32 + QUIET + WIDTH;
    (lo..hi).flat_map(move |y| (lo..hi).map(move |x| (x, y))).filter(move |&(x, y)| {
        (QUIET + 1..=QUIET + WIDTH).contains(&shell(x, y, side))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Profile;
    use alloc::collections::BTreeSet;

    #[test]
    fn shells_count_outward_from_the_symbol_edge() {
        let side = 21;
        assert_eq!(shell(0, 0, side), 0);
        assert_eq!(shell(20, 20, side), 0);
        assert_eq!(shell(-1, 5, side), 1);
        assert_eq!(shell(-2, 5, side), 2);
        assert_eq!(shell(-3, 5, side), 3);
        assert_eq!(shell(21, 5, side), 1);
        assert_eq!(shell(23, 5, side), 3);
        assert_eq!(shell(-3, -4, side), 4);
    }

    #[test]
    fn the_border_occupies_only_shells_three_and_four() {
        let side = 25;
        for y in -8..(side as i32 + 8) {
            for x in -8..(side as i32 + 8) {
                let d = shell(x, y, side);
                assert_eq!(cell_index(x, y, side, 4).is_some(), (3..=4).contains(&d), "({x},{y})");
            }
        }
    }

    #[test]
    fn cells_enumerates_exactly_the_border() {
        let side = 21;
        let listed: BTreeSet<(i32, i32)> = cells(side).collect();
        for &(x, y) in &listed {
            assert!(cell_index(x, y, side, 4).is_some());
        }
        let mut count = 0;
        for y in -6..(side as i32 + 6) {
            for x in -6..(side as i32 + 6) {
                if cell_index(x, y, side, 4).is_some() {
                    count += 1;
                    assert!(listed.contains(&(x, y)), "missed ({x},{y})");
                }
            }
        }
        assert_eq!(listed.len(), count);
    }

    #[test]
    fn every_reference_colour_appears() {
        for profile in [Profile::M, Profile::C1, Profile::C3] {
            let palette = Palette::for_profile(profile);
            let side = 29;
            let seen: BTreeSet<u32> = cells(side)
                .filter_map(|(x, y)| cell_color(x, y, side, &palette))
                .map(|c| c.to_hex())
                .collect();
            for i in 0..cycle_len(palette.len()) {
                assert!(seen.contains(&cycle_color(&palette, i).to_hex()), "{profile:?} missing {i}");
            }
            assert!(seen.contains(&BORDER_WHITE.to_hex()));
            assert!(seen.contains(&BORDER_BLACK.to_hex()));
        }
    }

    #[test]
    fn both_radial_layers_agree() {
        let palette = Palette::for_profile(Profile::C1);
        let side = 29;
        for x in 0..side as i32 {
            assert_eq!(
                cell_color(x, -3, side, &palette),
                cell_color(x, -4, side, &palette),
                "column {x}"
            );
            assert_eq!(
                cell_color(-3, x, side, &palette),
                cell_color(-4, x, side, &palette),
                "row {x}"
            );
        }
    }

    #[test]
    fn corners_are_unambiguous() {
        let palette = Palette::for_profile(Profile::C1);
        let side = 21;
        // A corner cell must resolve to exactly one colour regardless of which edge you
        // approach it from — the function is total, so this is really a no-panic check
        // plus a determinism check.
        for &(x, y) in &[(-3, -3), (-4, -4), (23, -3), (-3, 23), (23, 23), (24, 24)] {
            assert!(cell_color(x, y, side, &palette).is_some(), "({x},{y})");
        }
    }
}
