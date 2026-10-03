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

/// Modules of white between the symbol grid and the ring.
///
/// **One, not [`QUIET`]'s two.** §6.1's quiet zone exists to isolate the *finders* for the
/// 1:1:3:1:1 run-length scan, and a finder-less symbol (D17) has none — the ring is found by
/// its outer edge, which faces the other way. Nothing samples this gap; its remaining job is
/// keeping ring colour from bleeding into the edge of the grid under defocus, and one module
/// is several times the ~1/3 module a blur at the §13.1 pitch floor actually spreads.
///
/// Worth one module per side: 11.8% of a v1 symbol's area, 10.5% of a v2's, 2.5% of a v32's
/// — or, at a fixed size on screen, ~6% more pixels per module at v2, which is range.
pub const GAP: i32 = 1;

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

/// Sequence positions around the ring, for a symbol of `side` modules.
///
/// **Positions, not cells.** Both radial layers of the border take the same value, so each
/// position presents a 2-module-deep sampling target (P12) and the sequence runs once
/// around the perimeter rather than twice.
#[inline]
pub fn positions(side: u16) -> usize {
    let s = side as usize + 2 * (GAP + WIDTH) as usize;
    4 * s - 4
}

/// Which edge of the ring a coordinate sits on, and how far along it.
///
/// Shared with [`cell_index`] so that the periodic cycle and the position sequence agree
/// about where an edge starts — corners resolve to whichever axis the cell is further
/// outside on, exactly as P12 pins it.
fn edge_and_offset(x: i32, y: i32, side: u16) -> (usize, i32) {
    let s = side as i32;
    let out_x = x < 0 || x >= s;
    let out_y = y < 0 || y >= s;
    let depth = |c: i32| {
        if c < 0 {
            -c
        } else if c >= s {
            c - s + 1
        } else {
            0
        }
    };
    // 0 = top, 1 = right, 2 = bottom, 3 = left.
    if (out_y && !out_x) || (out_x && out_y && depth(y) >= depth(x)) {
        (if y < 0 { 0 } else { 2 }, x)
    } else {
        (if x < 0 { 3 } else { 1 }, y)
    }
}

/// Position of a border cell around the ring, or `None` if it is not a border cell.
///
/// Walks the ring clockwise from the top-left corner of its outer square, so consecutive
/// positions are physically adjacent — which is the whole point, since a decoder reads a
/// contiguous *arc* and correlates it against the sequence.
pub fn position(x: i32, y: i32, side: u16) -> Option<usize> {
    let d = shell(x, y, side);
    if !(GAP + 1..=GAP + WIDTH).contains(&d) {
        return None;
    }
    let lo = -(GAP + WIDTH);
    let span = side as i32 + 2 * (GAP + WIDTH); // outer square side
    let (edge, t) = edge_and_offset(x, y, side);
    // Clamp: an inner-layer cell can sit one short of the outer square's extent.
    let u = (t - lo).clamp(0, span - 1);
    let p = match edge {
        0 => u,
        1 => (span - 1) + u,
        2 => 3 * span - 3 - u,
        _ => 4 * span - 4 - u,
    };
    Some((p as usize) % positions(side))
}

/// The border-cycle index of a module-space coordinate, or `None` if it is not a border
/// cell.
pub fn cell_index(x: i32, y: i32, side: u16, colors: usize) -> Option<usize> {
    let d = shell(x, y, side);
    if !(GAP + 1..=GAP + WIDTH).contains(&d) {
        return None;
    }
    let s = side as i32;
    let out_x = x < 0 || x >= s;
    let out_y = y < 0 || y >= s;
    let depth = |c: i32| {
        if c < 0 {
            -c
        } else if c >= s {
            c - s + 1
        } else {
            0
        }
    };
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

/// Whether the ring is light at a given position, from the committed sequence.
///
/// The **luma** layer, which is what §12.1 requires detection to work from. Every window of
/// `border_seq::WINDOW[v−1]` consecutive positions is unique, so a decoder that reads a
/// contiguous arc knows where on the ring it is looking — orientation, which edge, and how
/// far along — without any localised marker to lose to a glare blob.
pub fn sequence_light(side: u16, position: usize) -> bool {
    let v = version_index(side);
    let seq = crate::border_seq::SEQUENCES[v];
    let p = position % positions(side);
    (seq[p / 8] >> (p % 8)) & 1 == 1
}

/// Zero-based version for a side length, per §6.1's `side = 21 + 4·(v−1)`.
#[inline]
fn version_index(side: u16) -> usize {
    // Clamped to the table rather than to a literal: the ceiling moved from 32 to 64 and
    // a hard-coded 31 silently returned v32's sequence for every larger version — a
    // shorter sequence than that ring has positions, so it indexed past the end.
    ((side.saturating_sub(21) / 4) as usize).min(crate::border_seq::SEQUENCES.len() - 1)
}

/// The colour of a border cell, or `None` if the coordinate is not on the border.
///
/// The luma comes from the position sequence; the colour is then chosen from the entries
/// of §6.4's cycle that sit at that luma level — the `L0` palette colours plus black when
/// dark, the `L1` colours plus white when light. Rotating through the eligible set by
/// position keeps every border colour present and spatially spread, which is what the
/// §12.3 illumination fit wants from the ring.
///
/// So the sequence changes the **order** of the border colours, not their coverage.
pub fn cell_color(x: i32, y: i32, side: u16, palette: &Palette) -> Option<Srgb8> {
    let p = position(x, y, side)?;
    let light = sequence_light(side, p);
    let hues = palette.len() / 2;
    // Border-cycle indices: 0..hues are L0, hues..2·hues are L1, then white, then black.
    let eligible = hues + 1;
    let pick = p % eligible;
    let idx = if light {
        if pick == hues {
            palette.len()
        } else {
            hues + pick
        }
    } else if pick == hues {
        palette.len() + 1
    } else {
        pick
    };
    Some(cycle_color(palette, idx))
}

// ---------------------------------------------------------------- data ring (D17)

/// What the ring's colour choices are used for.
///
/// At every ring position the luma is fixed by the position sequence, but the *colour* at
/// that luma is a free choice among `hues + 1` candidates. [`RingMode::Plain`] spends that
/// freedom on nothing — colour is a fixed function of position. [`RingMode::Data`] spends it
/// on the format codeword, which is what lets the finder-less layout drop the in-grid format
/// region entirely.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RingMode {
    /// Colour is a fixed function of position (D14). Carried by symbols that still have
    /// finders.
    Plain,
    /// The finder-less layout (D17). Every [`REF_STRIDE`]-th position is a fixed-colour
    /// reference; the rest carry the 30-bit masked format codeword, in two interleaved
    /// copies.
    Data(u32),
}

/// Every `REF_STRIDE`-th position of a data ring is a fixed-colour reference.
///
/// The references are what break the circularity: a decoder needs illumination fitted
/// before it can classify hues, and it needs the format before it knows every ring colour.
/// Reference colours depend on position alone, so the fit can run first.
pub const REF_STRIDE: usize = 3;

/// Bits in one format codeword copy.
pub const FORMAT_COPY_BITS: usize = 30;
/// Format copies carried by a data ring.
pub const FORMAT_COPIES: usize = 2;

/// Whether ring position `p` is a fixed-colour reference in a data ring.
#[inline]
pub fn is_reference(p: usize) -> bool {
    p.is_multiple_of(REF_STRIDE)
}

/// Number of data (non-reference) positions on the ring of a `side`-module symbol.
#[inline]
pub fn data_positions(side: u16) -> usize {
    let n = positions(side);
    n - n.div_ceil(REF_STRIDE)
}

/// Ring position of the `d`-th data position.
#[inline]
pub fn data_position(d: usize) -> usize {
    3 * (d / 2) + 1 + d % 2
}

/// Format bits carried per data position: `log2` of the hue count.
#[inline]
pub fn bits_per_position(hues: usize) -> usize {
    hues.trailing_zeros() as usize
}

/// Where codeword bit `bit` of copy `copy` lives: `(ring position, bit within its symbol)`.
///
/// The 60 copy bits are spread evenly over **all** data slots, the two copies offset by half
/// a step. A glare blob covering one arc therefore chips a few bits from each copy rather
/// than erasing one copy outright, which is what lets the BCH code absorb it — the same
/// reasoning §8.3 applies to the payload.
pub fn format_slot(copy: usize, bit: usize, side: u16, hues: usize) -> (usize, usize) {
    let bpp = bits_per_position(hues).max(1);
    let slots = data_positions(side) * bpp;
    let total = FORMAT_COPY_BITS * FORMAT_COPIES;
    debug_assert!(slots >= total, "ring too short for the format copies");
    let s = (2 * bit + copy) * slots / total;
    (data_position(s / bpp), s % bpp)
}

/// The data symbol at ring position `p` for a codeword: the inverse of [`format_slot`].
fn data_symbol(p: usize, side: u16, hues: usize, codeword: u32) -> usize {
    let bpp = bits_per_position(hues);
    if bpp == 0 {
        return 0;
    }
    let slots = data_positions(side) * bpp;
    let total = FORMAT_COPY_BITS * FORMAT_COPIES;
    let d = p - p / REF_STRIDE - 1;
    let mut symbol = 0;
    for b in 0..bpp {
        let s = d * bpp + b;
        // The slot map `s = j·slots/total` is injective because `slots >= total`, so the
        // only candidate `j` is the ceiling of its inverse.
        let j = (s * total).div_ceil(slots);
        if j < total && j * slots / total == s {
            let v = (codeword >> (FORMAT_COPY_BITS - 1 - j / 2)) & 1;
            symbol |= (v as usize) << b;
        }
    }
    symbol
}

/// Palette index of a reference position's fixed colour.
fn reference_index(p: usize, light: bool, hues: usize, n: usize) -> usize {
    // Rotate by `p / REF_STRIDE`, not `p`: every reference position has `p % 3 == 0`, so
    // `p % (hues + 1)` would pin one colour and the §12.3 fit would never see the rest.
    let pick = (p / REF_STRIDE) % (hues + 1);
    match (light, pick == hues) {
        (true, true) => n,      // white
        (false, true) => n + 1, // black
        (true, false) => hues + pick,
        (false, false) => pick,
    }
}

/// Colour of a border cell under a given ring mode, or `None` if it is not on the border.
pub fn cell_color_in(
    x: i32,
    y: i32,
    side: u16,
    palette: &Palette,
    mode: RingMode,
) -> Option<Srgb8> {
    let RingMode::Data(codeword) = mode else {
        return cell_color(x, y, side, palette);
    };
    let p = position(x, y, side)?;
    let light = sequence_light(side, p);
    let hues = palette.len() / 2;
    let idx = if is_reference(p) {
        reference_index(p, light, hues, palette.len())
    } else {
        let hue = crate::symbol::chroma_symbol_to_hue(data_symbol(p, side, hues, codeword), hues);
        if light { hues + hue } else { hue }
    };
    Some(cycle_color(palette, idx))
}

/// Fixed colour at a data ring's reference position, or `None` elsewhere.
///
/// Independent of the codeword, which is exactly what lets a decoder fit illumination from
/// these before it has read the format.
pub fn reference_color(x: i32, y: i32, side: u16, palette: &Palette) -> Option<Srgb8> {
    let p = position(x, y, side)?;
    if !is_reference(p) {
        return None;
    }
    let idx = reference_index(p, sequence_light(side, p), palette.len() / 2, palette.len());
    Some(cycle_color(palette, idx))
}

/// Outer-layer cell of ring position `p`: the inverse of [`position`] on the outer square.
pub fn outer_cell(p: usize, side: u16) -> (i32, i32) {
    let lo = -(GAP + WIDTH);
    let s = side as i32 + 2 * (GAP + WIDTH);
    let hi = lo + s - 1;
    let p = (p % positions(side)) as i32;
    if p < s {
        (lo + p, lo)
    } else if p <= 2 * s - 2 {
        (hi, lo + p - (s - 1))
    } else if p <= 3 * s - 3 {
        (lo + (3 * s - 3 - p), hi)
    } else {
        (lo, lo + (4 * s - 4 - p))
    }
}

/// The cell one step inward from [`outer_cell`], on the ring's second layer.
///
/// At the four corners this is the inner layer's corner cell, which [`position`] may assign
/// to a neighbouring position — so a sampler should check `position(inner) == Some(p)`
/// before treating the two cells as one target.
pub fn inner_cell(p: usize, side: u16) -> (i32, i32) {
    let (x, y) = outer_cell(p, side);
    let lo = -(GAP + WIDTH);
    let hi = side as i32 + GAP + WIDTH - 1;
    let step = |c: i32| {
        if c == lo {
            c + 1
        } else if c == hi {
            c - 1
        } else {
            c
        }
    };
    (step(x), step(y))
}

/// Every border cell's module-space coordinate.
pub fn cells(side: u16) -> impl Iterator<Item = (i32, i32)> {
    let lo = -(GAP + WIDTH);
    let hi = side as i32 + GAP + WIDTH;
    (lo..hi)
        .flat_map(move |y| (lo..hi).map(move |x| (x, y)))
        .filter(move |&(x, y)| (GAP + 1..=GAP + WIDTH).contains(&shell(x, y, side)))
}

// ---------------------------------------------------------------- ring payload (D19)

/// Whether ring position `p` keeps both of its radial layers.
///
/// **Reference positions do; data positions do not.** P12 gave every position a
/// 2-module-deep target, which is what the detector's sequence correlation and the §12.3
/// illumination fit are built on — but only where it is load-bearing. A reference position
/// stays two deep; a data position hands its inner cell to payload (D19).
#[inline]
pub fn is_two_deep(p: usize) -> bool {
    is_reference(p)
}

/// The inner cell position `p` gives to payload, if it has one.
///
/// `None` for reference positions, and for the corners, where the inner cell belongs to a
/// neighbouring position and there is nothing unambiguous to hand over.
fn payload_cell_of(p: usize, side: u16) -> Option<(i32, i32)> {
    if is_two_deep(p) {
        return None;
    }
    let (x, y) = inner_cell(p, side);
    (position(x, y, side) == Some(p)).then_some((x, y))
}

/// Ring cells carrying payload, in ring order (D19).
///
/// One luma bit each, plus the profile's chroma bits where a chroma cell is a single module
/// — see [`crate::layout::Layout::capacity`]. Roughly two thirds of the inner layer, which
/// is worth 15–18% more payload at v1–v2 and ~2% at v32.
pub fn payload_cells(side: u16) -> impl Iterator<Item = (i32, i32)> {
    (0..positions(side)).filter_map(move |p| payload_cell_of(p, side))
}

/// Ordinal of a ring payload cell in [`payload_cells`] order, or `None` if `(x, y)` is not
/// one.
///
/// Counts with the same predicate [`payload_cells`] enumerates with, rather than a closed
/// form over reference positions and corners. The closed form was off by one at v1, and a
/// renderer that disagrees with the decoder's enumeration by one cell writes every ring bit
/// into the wrong slot — cheap arithmetic is not worth that class of bug.
pub fn payload_cell_index(x: i32, y: i32, side: u16) -> Option<usize> {
    let p = position(x, y, side)?;
    if payload_cell_of(p, side) != Some((x, y)) {
        return None;
    }
    Some(
        (0..p)
            .filter(|&q| payload_cell_of(q, side).is_some())
            .count(),
    )
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
    fn the_border_occupies_only_shells_of_its_own_width() {
        let side = 25;
        for y in -8..(side as i32 + 8) {
            for x in -8..(side as i32 + 8) {
                let d = shell(x, y, side);
                assert_eq!(
                    cell_index(x, y, side, 4).is_some(),
                    (GAP + 1..=GAP + WIDTH).contains(&d),
                    "({x},{y})"
                );
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
                assert!(
                    seen.contains(&cycle_color(&palette, i).to_hex()),
                    "{profile:?} missing {i}"
                );
            }
            assert!(seen.contains(&BORDER_WHITE.to_hex()));
            assert!(seen.contains(&BORDER_BLACK.to_hex()));
        }
    }

    #[test]
    fn both_radial_layers_agree() {
        let palette = Palette::for_profile(Profile::C1);
        let side = 29;
        let (inner, outer) = (-(GAP + 1), -(GAP + WIDTH));
        for x in 0..side as i32 {
            assert_eq!(
                cell_color(x, inner, side, &palette),
                cell_color(x, outer, side, &palette),
                "column {x}"
            );
            assert_eq!(
                cell_color(inner, x, side, &palette),
                cell_color(outer, x, side, &palette),
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
        let (o, i) = (-(GAP + WIDTH), -(GAP + 1));
        let (fo, fi) = (side as i32 + GAP + WIDTH - 1, side as i32 + GAP);
        for &(x, y) in &[(o, o), (i, i), (fo, o), (o, fo), (fo, fo), (fi, fi)] {
            assert!(cell_color(x, y, side, &palette).is_some(), "({x},{y})");
        }
    }
}

#[cfg(test)]
mod sequence_tests {
    use super::*;
    use crate::border_seq::{SEQUENCES, WINDOW};
    use alloc::vec;

    /// Walking the ring's outer square must visit every position exactly once.
    ///
    /// The property the whole scheme rests on: if two cells share a position, a decoder
    /// reading an arc gets a sequence that is not the one in the table, and localisation
    /// silently points somewhere else.
    #[test]
    fn the_outer_ring_walk_is_a_bijection() {
        for v in [1u8, 2, 5, 12, 32] {
            let side = 21 + 4 * (v as u16 - 1);
            let p = positions(side);
            let mut seen = vec![0u32; p];
            let d = GAP + WIDTH;
            let (lo, hi) = (-d, side as i32 + d - 1);
            for x in lo..=hi {
                for y in lo..=hi {
                    if shell(x, y, side) != GAP + WIDTH {
                        continue; // outer layer only
                    }
                    let pos = position(x, y, side).expect("outer ring cell has a position");
                    seen[pos] += 1;
                }
            }
            assert!(
                seen.iter().all(|&n| n == 1),
                "v{v}: outer ring walk is not a bijection over {p} positions",
            );
        }
    }

    /// Re-check the committed table's defining property in the crate that ships it.
    ///
    /// `just debruijn` proves this when generating, but the generator is not what a
    /// decoder runs against — the table is. A bad paste would otherwise ship silently.
    #[test]
    fn every_sequence_window_is_unique() {
        for v in 1u8..=32 {
            let side = 21 + 4 * (v as u16 - 1);
            let p = positions(side);
            let k = WINDOW[v as usize - 1] as usize;
            let seq = SEQUENCES[v as usize - 1];
            assert!(
                seq.len() * 8 >= p,
                "v{v}: sequence too short for {p} positions"
            );

            let mut windows = vec![0u32; p];
            for (i, w) in windows.iter_mut().enumerate() {
                let mut acc = 0u32;
                for j in 0..k {
                    let q = (i + j) % p;
                    acc = (acc << 1) | u32::from((seq[q / 8] >> (q % 8)) & 1);
                }
                *w = acc;
            }
            windows.sort_unstable();
            assert!(
                windows.windows(2).all(|pair| pair[0] != pair[1]),
                "v{v}: window length {k} is not unique over {p} positions",
            );
        }
    }

    /// Both radial layers must agree, or the 2-module-deep sampling target P12 describes
    /// is two different values stacked.
    #[test]
    fn both_radial_layers_share_a_value() {
        let side = 29u16; // v3
        for x in 0..side as i32 {
            let outer = position(x, -(GAP + WIDTH), side).unwrap();
            let inner = position(x, -(GAP + 1), side).unwrap();
            assert_eq!(
                sequence_light(side, outer),
                sequence_light(side, inner),
                "layers disagree above column {x}",
            );
        }
    }
}

#[cfg(test)]
mod ring_data_tests {
    use super::*;
    use crate::palette::Palette;
    use crate::version::Profile;

    fn sides() -> impl Iterator<Item = u16> {
        [1u16, 2, 3, 5, 12, 20, 32]
            .into_iter()
            .map(|v| 21 + 4 * (v - 1))
    }

    #[test]
    fn outer_cell_inverts_position() {
        for side in sides() {
            for p in 0..positions(side) {
                let (x, y) = outer_cell(p, side);
                assert_eq!(position(x, y, side), Some(p), "side {side}, position {p}");
            }
        }
    }

    /// The O(1) ordinal and the enumerated list must agree, or the renderer paints one
    /// cell's bits into another's.
    #[test]
    fn payload_cell_index_matches_the_list() {
        for side in sides() {
            let cells: alloc::vec::Vec<(i32, i32)> = payload_cells(side).collect();
            for (i, &(x, y)) in cells.iter().enumerate() {
                assert_eq!(
                    payload_cell_index(x, y, side),
                    Some(i),
                    "side {side} cell {i}"
                );
            }
            // A two-deep position never hands over the inner cell it owns. At a corner
            // that cell belongs to the neighbouring position, which may well hand it over —
            // so ask only about cells the position actually owns.
            for p in (0..positions(side)).filter(|&p| is_two_deep(p)) {
                let (ix, iy) = inner_cell(p, side);
                if position(ix, iy, side) == Some(p) {
                    assert_eq!(
                        payload_cell_index(ix, iy, side),
                        None,
                        "side {side} position {p}"
                    );
                }
            }
        }
    }

    /// The detector's hull needs a guaranteed dark cell at each corner of the ring.
    #[test]
    fn ring_corners_are_dark() {
        for side in sides() {
            // From the ring's own length, not a second copy of the canvas geometry: that
            // duplication is what shipped a table for the wrong ring in D18.
            let s = (positions(side) + 4) / 4;
            for p in [0, s - 1, 2 * (s - 1), 3 * (s - 1)] {
                assert!(!sequence_light(side, p), "side {side}: corner {p} is light");
            }
        }
    }

    /// The format codeword must be recoverable from exact ring colours, for both palette
    /// families — the round trip the decoder's soft read approximates.
    #[test]
    fn format_codeword_round_trips_through_ring_colours() {
        for profile in [Profile::C1, Profile::C3] {
            let palette = Palette::for_profile(profile);
            let hues = palette.len() / 2;
            for side in sides() {
                for codeword in [0u32, 0x3FFF_FFFF, 0x1555_5555, 0x2345_6789 & 0x3FFF_FFFF] {
                    let mode = RingMode::Data(codeword);
                    for copy in 0..FORMAT_COPIES {
                        for bit in 0..FORMAT_COPY_BITS {
                            let (p, b) = format_slot(copy, bit, side, hues);
                            assert!(!is_reference(p));
                            let (x, y) = outer_cell(p, side);
                            let c = cell_color_in(x, y, side, &palette, mode).unwrap();
                            let idx = palette.colors.iter().position(|&k| k == c).unwrap();
                            let symbol = crate::symbol::hue_to_chroma_symbol(idx % hues, hues);
                            let got = (symbol >> b) & 1;
                            let want = ((codeword >> (FORMAT_COPY_BITS - 1 - bit)) & 1) as usize;
                            assert_eq!(got, want, "{profile:?} side {side} copy {copy} bit {bit}");
                        }
                    }
                }
            }
        }
    }

    /// References must not depend on the codeword, and must still show every colour.
    #[test]
    fn references_are_codeword_independent_and_cover_the_cycle() {
        for profile in [Profile::C1, Profile::C3] {
            let palette = Palette::for_profile(profile);
            for side in sides() {
                let mut seen = alloc::vec![false; cycle_len(palette.len())];
                for p in (0..positions(side)).filter(|&p| is_reference(p)) {
                    let (x, y) = outer_cell(p, side);
                    let a = cell_color_in(x, y, side, &palette, RingMode::Data(0)).unwrap();
                    let b = cell_color_in(x, y, side, &palette, RingMode::Data(0x3FFF_FFFF));
                    assert_eq!(Some(a), b);
                    assert_eq!(reference_color(x, y, side, &palette), Some(a));
                    let i = (0..cycle_len(palette.len()))
                        .find(|&i| cycle_color(&palette, i) == a)
                        .unwrap();
                    seen[i] = true;
                }
                assert!(seen.iter().all(|&s| s), "{profile:?} side {side}: {seen:?}");
            }
        }
    }
}
