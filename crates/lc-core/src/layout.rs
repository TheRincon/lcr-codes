//! Symbol layout: which module does what, and in what order payload visits them
//! (§6.2, §6.3, §8.4).
//!
//! Everything downstream — rendering, masking, interleaving, sampling — reads its
//! geometry from [`Layout`], so there is exactly one definition of the symbol and no way
//! for encoder and decoder to disagree about it.
//!
//! ## Pinned: allocation is greedy over a fixed candidate order
//!
//! §6.2 places the palette anchors "on a ring outside each finder separator" and §7 puts
//! 30 format modules "adjacent to the top-left and top-right finders". Taken literally
//! these overlap: the ring immediately outside a 7×7 finder plus its separator has 17
//! cells, two of which are timing, and the two features want 8 and 30 respectively. At
//! v1 there is not room to give each its own ring without the top-left and top-right
//! format copies colliding in the middle of a 21-module symbol.
//!
//! Rather than special-case small versions, this implementation defines the layout as a
//! **claim order over a fixed candidate enumeration**, which is collision-free by
//! construction at every version:
//!
//! 1. Finders and separators.
//! 2. Alignment markers.
//! 3. Timing patterns.
//! 4. Format copy A (top-left), then copy B (top-right) — 30 each, from the ring walk
//!    below.
//! 5. Palette anchors — `2k` per finder, from the same walk.
//! 6. Scattered references (§6.3).
//! 7. Everything left is payload.
//!
//! The ring walk around a finder enumerates ring `d = 8, 9, 10, …`, and within a ring
//! alternates between the two arms working **inward from the symbol edge**:
//! `(d,0), (0,d), (d,1), (1,d), …, (d,d)`.
//!
//! ## Why the format region claims before the palette anchors
//!
//! This ordering is not a preference — it is forced. There are `2k` palette anchors per
//! finder, so `k` determines how many cells the anchors consume, so **anchors-first would
//! make the format region's position depend on the profile**. The decoder reads the format
//! region *to learn* the profile (§12.2), and cannot know where to look for it until it
//! has. Claiming format first makes `format_cells` a function of the version alone, which
//! is what closes the loop: the decoder guesses a version from the finder geometry, and
//! the format region confirms or corrects it.
//!
//! [`tests::format_cells_do_not_depend_on_the_profile`] locks this in. It is the kind of
//! coupling that produces a decoder which works on one profile and mysteriously does not
//! on another.
//!
//! The anchors land one ring further out as a result. That costs little — they are still
//! adjacent to the finders, which is what §6.2 asks for, and they are still at the outer
//! ends of their ring, away from the symbol interior where a finger is most likely to
//! land.
//!
//! ## Pinned: interior alignment markers (spec open decision #9)
//!
//! §6.2 leaves the `v ≥ 14` placement rule open. Pinned here as the QR-style rule:
//! coordinates run from 6 to `side − 7`, evenly spaced, rounded to even, with as few
//! steps as keeps every spacing ≤ 28 modules. Markers go at every coordinate pair except
//! the three that would land on a finder. Even coordinates matter — an alignment marker
//! crossing row or column 6 then agrees with the timing pattern module-for-module
//! instead of contradicting it.

use crate::grid::{ModuleGrid, ModuleRole};
use crate::palette::Palette;
use crate::version::{Profile, Version};
use alloc::vec::Vec;

/// Sentinel in [`Layout::chroma_of`] for a module whose chroma cell carries no symbol.
pub const NO_CHROMA: u32 = u32::MAX;

/// Which finder a ring walk is centred on.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
}

/// Bits carried by each layer of one symbol.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Capacity {
    /// Payload modules — one luma bit each.
    pub luma_bits: usize,
    /// Chroma-carrying cells × `log2(k)`.
    pub chroma_bits: usize,
}

impl Capacity {
    /// Total raw channel bits before ECC.
    #[inline]
    pub const fn total(self) -> usize {
        self.luma_bits + self.chroma_bits
    }
}

/// The complete geometry of one symbol.
#[derive(Clone, Debug)]
pub struct Layout {
    /// Size class.
    pub version: Version,
    /// Colour configuration.
    pub profile: Profile,
    /// Grid side in modules.
    pub side: u16,
    /// Role of every module.
    pub roles: ModuleGrid<ModuleRole>,
    /// Luma value of luma-only fixed patterns: `Some(true)` is L1 (light).
    pub fixed: ModuleGrid<Option<bool>>,
    /// Palette index of anchor and reference modules; `u8::MAX` elsewhere.
    pub ref_color: ModuleGrid<u8>,
    /// The two format copies, each 30 cells in codeword bit order (index 0 = MSB).
    pub format_cells: [Vec<(u16, u16)>; 2],
    /// Payload modules in §8.4 walk order.
    pub payload: Vec<(u16, u16)>,
    /// Chroma-carrying cells in walk order, as cell coordinates.
    pub chroma_cells: Vec<(u16, u16)>,
    /// Ordinal into [`Layout::chroma_cells`] for each module, or [`NO_CHROMA`].
    pub chroma_of: ModuleGrid<u32>,
    /// Centres of every alignment marker.
    pub alignment_centers: Vec<(u16, u16)>,
}

impl Layout {
    /// Build the layout for a version and profile. Deterministic and side-effect free;
    /// callers that build many symbols should cache it.
    pub fn new(version: Version, profile: Profile) -> Layout {
        let side = version.side();
        let n_colors = profile.colors();
        let mut roles = ModuleGrid::new(side, ModuleRole::Payload);
        let mut fixed = ModuleGrid::new(side, None::<bool>);
        let mut ref_color = ModuleGrid::new(side, u8::MAX);

        // 1. Finders and separators.
        for &(ox, oy) in &[(0u16, 0u16), (side - 7, 0), (0, side - 7)] {
            for dy in 0..7u16 {
                for dx in 0..7u16 {
                    let d = (dx as i32 - 3).abs().max((dy as i32 - 3).abs());
                    roles.set(ox + dx, oy + dy, ModuleRole::Finder);
                    fixed.set(ox + dx, oy + dy, Some(d == 2));
                }
            }
            // The separator is the one-module light ring on the two interior sides,
            // extended one module past the corner so the finder is fully enclosed.
            let (sx, sy) = (ox as i32, oy as i32);
            for i in -1i32..=7 {
                for &(px, py) in &[
                    (sx + 7, sy + i),
                    (sx - 1, sy + i),
                    (sx + i, sy + 7),
                    (sx + i, sy - 1),
                ] {
                    if px >= 0 && py >= 0 && (px as u16) < side && (py as u16) < side {
                        let (px, py) = (px as u16, py as u16);
                        if *roles.at(px, py) == ModuleRole::Payload {
                            roles.set(px, py, ModuleRole::Separator);
                            fixed.set(px, py, Some(true));
                        }
                    }
                }
            }
        }

        // 2. Alignment markers.
        let alignment_centers = alignment_centers(version);
        for &(cx, cy) in &alignment_centers {
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let (x, y) = ((cx as i32 + dx) as u16, (cy as i32 + dy) as u16);
                    roles.set(x, y, ModuleRole::Alignment);
                    fixed.set(x, y, Some(dx.abs().max(dy.abs()) == 1));
                }
            }
        }

        // 3. Timing patterns. Dark on even coordinates, matching the alignment markers
        //    they cross.
        for i in 8..side - 8 {
            for &(x, y) in &[(i, 6u16), (6u16, i)] {
                if *roles.at(x, y) == ModuleRole::Payload {
                    roles.set(x, y, ModuleRole::Timing);
                    fixed.set(x, y, Some(i % 2 == 1));
                }
            }
        }

        // 4. Format region: 30 modules per copy, top-left first then top-right. Claimed
        //    before the anchors so its position depends only on the version — see the
        //    module docs.
        let copy_a = claim(&mut roles, Corner::TopLeft, side, 30, ModuleRole::Format);
        let copy_b = claim(&mut roles, Corner::TopRight, side, 30, ModuleRole::Format);
        debug_assert_eq!(copy_a.len(), 30);
        debug_assert_eq!(copy_b.len(), 30);

        // 5. Palette anchors: 2k per finder, cycling the palette once.
        let n_anchors = 2 * profile.hues();
        for corner in [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft] {
            let cells = claim(
                &mut roles,
                corner,
                side,
                n_anchors,
                ModuleRole::PaletteAnchor,
            );
            for (i, &(x, y)) in cells.iter().enumerate() {
                ref_color.set(x, y, (i % n_colors) as u8);
            }
        }

        // 6. Scattered references (§6.3), cycling the palette in raster order of tile.
        let tiles_per_row = side.div_ceil(16) as usize;
        for y in 0..side {
            for x in 0..side {
                let sub = match (x % 16, y % 16) {
                    (3, 3) => Some(0usize),
                    (11, 3) => Some(1),
                    (3, 11) => Some(2),
                    (11, 11) => Some(3),
                    _ => None,
                };
                let Some(sub) = sub else { continue };
                if *roles.at(x, y) != ModuleRole::Payload {
                    continue;
                }
                let tile = (y / 16) as usize * tiles_per_row + (x / 16) as usize;
                roles.set(x, y, ModuleRole::Reference);
                ref_color.set(x, y, ((tile * 4 + sub) % n_colors) as u8);
            }
        }

        // 7. Payload walk and chroma cells.
        let payload = payload_walk(&roles, side);
        let (chroma_cells, chroma_of) = chroma_layout(&roles, side, profile, &payload);

        Layout {
            version,
            profile,
            side,
            roles,
            fixed,
            ref_color,
            format_cells: [copy_a, copy_b],
            payload,
            chroma_cells,
            chroma_of,
            alignment_centers,
        }
    }

    /// Bits available to each layer before ECC (§8.1).
    pub fn capacity(&self) -> Capacity {
        Capacity {
            luma_bits: self.payload.len(),
            chroma_bits: self.chroma_cells.len() * self.profile.chroma_bits_per_cell(),
        }
    }

    /// The nominal colour of a module whose colour the decoder knows a priori — a
    /// finder, separator, timing, alignment or reference module (§12.3).
    ///
    /// Returns `None` for payload and format modules, whose colour depends on data.
    pub fn known_color(&self, x: u16, y: u16, palette: &Palette) -> Option<crate::color::Srgb8> {
        match self.roles.at(x, y) {
            ModuleRole::PaletteAnchor | ModuleRole::Reference => {
                Some(palette.colors[*self.ref_color.at(x, y) as usize])
            }
            ModuleRole::Finder
            | ModuleRole::Separator
            | ModuleRole::Timing
            | ModuleRole::Alignment => self.fixed.at(x, y).map(|l| palette.luma_only(l)),
            _ => None,
        }
    }

    /// Centres of the three finders, in `(top-left, top-right, bottom-left)` order.
    pub fn finder_centers(&self) -> [(u16, u16); 3] {
        let s = self.side;
        [(3, 3), (s - 4, 3), (3, s - 4)]
    }
}

/// Alignment marker centres for a version (§6.2, pinned rule in the module docs).
pub fn alignment_centers(version: Version) -> Vec<(u16, u16)> {
    let v = version.get();
    if v == 1 {
        return Vec::new();
    }
    let side = version.side();
    let last = side - 7;
    let span = last - 6;
    // As few steps as keeps every spacing at or under 28 modules.
    let count: u16 = if v <= 13 {
        2
    } else {
        2 + span.div_ceil(28).saturating_sub(1)
    };
    let coords: Vec<u16> = if count == 2 {
        alloc::vec![6, last]
    } else {
        let step = 2 * (span as f32 / (2 * (count - 1)) as f32).round() as u16;
        let mut c: Vec<u16> = (0..count - 1)
            .map(|i| last - (count - 2 - i) * step)
            .collect();
        c.insert(0, 6);
        c
    };

    let mut out = Vec::new();
    for &cy in &coords {
        for &cx in &coords {
            // Skip the three that would sit on a finder.
            let on_finder =
                (cx == 6 && cy == 6) || (cx == 6 && cy == last) || (cx == last && cy == 6);
            if !on_finder {
                out.push((cx, cy));
            }
        }
    }
    out
}

/// Enumerate ring `d` around a finder, alternating arms and working inward from the
/// symbol edge.
fn ring(corner: Corner, d: u16, side: u16) -> Vec<(u16, u16)> {
    let mut out = Vec::with_capacity(2 * d as usize + 1);
    for i in 0..d {
        match corner {
            Corner::TopLeft => {
                out.push((d, i));
                out.push((i, d));
            }
            Corner::TopRight => {
                out.push((side - 1 - d, i));
                out.push((side - 1 - i, d));
            }
            Corner::BottomLeft => {
                out.push((i, side - 1 - d));
                out.push((d, side - 1 - i));
            }
        }
    }
    out.push(match corner {
        Corner::TopLeft => (d, d),
        Corner::TopRight => (side - 1 - d, d),
        Corner::BottomLeft => (d, side - 1 - d),
    });
    out
}

/// Take the next `want` unclaimed cells from a finder's ring walk and assign them `role`.
fn claim(
    roles: &mut ModuleGrid<ModuleRole>,
    corner: Corner,
    side: u16,
    want: usize,
    role: ModuleRole,
) -> Vec<(u16, u16)> {
    let mut got = Vec::with_capacity(want);
    let mut d = 8u16;
    while got.len() < want && d < side {
        for (x, y) in ring(corner, d, side) {
            if got.len() == want {
                break;
            }
            if *roles.at(x, y) == ModuleRole::Payload {
                roles.set(x, y, role);
                got.push((x, y));
            }
        }
        d += 1;
    }
    got
}

/// Boustrophedon column-pair walk from bottom-right upward (§8.4).
///
/// Column 6 is skipped wholesale: it is timing or finder along its entire length and
/// never carries payload, and stepping over it keeps the column pairing aligned.
fn payload_walk(roles: &ModuleGrid<ModuleRole>, side: u16) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    let mut x = side - 1;
    let mut upward = true;
    loop {
        if x == 6 {
            x -= 1;
        }
        for i in 0..side {
            let y = if upward { side - 1 - i } else { i };
            for dx in 0..2u16 {
                let cx = x - dx;
                if roles.at(cx, y).is_payload() {
                    out.push((cx, y));
                }
            }
        }
        upward = !upward;
        if x < 2 {
            break;
        }
        x -= 2;
    }
    out
}

/// Determine which chroma cells carry a symbol, and in what order.
///
/// ## Pinned: a chroma cell carries a symbol only if it is *entirely* payload
///
/// §5 defines a chroma cell as a group of modules sharing one chroma symbol, but does
/// not say what happens to a `2×2` cell straddling a finder edge or containing a
/// scattered reference. Partial cells are excluded here. The alternative — averaging
/// `a*b*` over a cell where some modules are rendered at a fixed hue — would bias the
/// §12.4 chroma LLR by a known-but-unmodelled amount at exactly the cells nearest the
/// fixed patterns, which is where geometry error is already worst. Payload modules in an
/// excluded cell still carry their luma bit and render at hue state H0.
///
/// The cost is small and bounded: for `C1` (1×1 cells) it is zero, and for `C2`/`C3` it
/// is the cells touching fixed patterns plus one cell per scattered reference — about 6%
/// of chroma capacity in the interior.
fn chroma_layout(
    roles: &ModuleGrid<ModuleRole>,
    side: u16,
    profile: Profile,
    payload: &[(u16, u16)],
) -> (Vec<(u16, u16)>, ModuleGrid<u32>) {
    let c = profile.chroma_cell();
    let mut chroma_of = ModuleGrid::new(side, NO_CHROMA);
    if profile.chroma_bits_per_cell() == 0 {
        return (Vec::new(), chroma_of);
    }

    let cells_side = side.div_ceil(c);
    let mut carries = ModuleGrid::new(cells_side, false);
    for cy in 0..cells_side {
        for cx in 0..cells_side {
            let mut all_payload = true;
            for dy in 0..c {
                for dx in 0..c {
                    let (x, y) = (cx * c + dx, cy * c + dy);
                    if !roles.get(x, y).is_some_and(|r| r.is_payload()) {
                        all_payload = false;
                    }
                }
            }
            carries.set(cx, cy, all_payload);
        }
    }

    let mut order = Vec::new();
    let mut seen = ModuleGrid::new(cells_side, false);
    for &(x, y) in payload {
        let (cx, cy) = (x / c, y / c);
        if !*carries.at(cx, cy) || *seen.at(cx, cy) {
            continue;
        }
        seen.set(cx, cy, true);
        let ord = order.len() as u32;
        order.push((cx, cy));
        for dy in 0..c {
            for dx in 0..c {
                chroma_of.set(cx * c + dx, cy * c + dy, ord);
            }
        }
    }
    (order, chroma_of)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::ModuleRole::*;

    fn all_layouts() -> impl Iterator<Item = Layout> {
        Version::all().flat_map(|v| Profile::ALL.into_iter().map(move |p| Layout::new(v, p)))
    }

    #[test]
    fn every_version_and_profile_allocates_without_collision() {
        for l in all_layouts() {
            assert_eq!(
                l.format_cells[0].len(),
                30,
                "v{} {:?}",
                l.version.get(),
                l.profile
            );
            assert_eq!(
                l.format_cells[1].len(),
                30,
                "v{} {:?}",
                l.version.get(),
                l.profile
            );

            let mut seen = ModuleGrid::new(l.side, 0u8);
            for copy in &l.format_cells {
                for &(x, y) in copy {
                    assert_eq!(*l.roles.at(x, y), Format);
                    assert_eq!(*seen.at(x, y), 0, "format cell reused at ({x},{y})");
                    seen.set(x, y, 1);
                }
            }
            let anchors = l
                .roles
                .cells()
                .iter()
                .filter(|r| **r == PaletteAnchor)
                .count();
            assert_eq!(anchors, 3 * 2 * l.profile.hues(), "v{}", l.version.get());
        }
    }

    /// The decoder reads the format region to learn the profile, so the region's
    /// position must not depend on it. Anchors-first allocation broke this and produced a
    /// decoder that worked on one profile and not the others.
    #[test]
    fn format_cells_do_not_depend_on_the_profile() {
        for v in Version::all() {
            let reference = Layout::new(v, Profile::M).format_cells.clone();
            for p in Profile::ALL {
                assert_eq!(
                    Layout::new(v, p).format_cells,
                    reference,
                    "v{} {p:?} moved the format region",
                    v.get()
                );
            }
        }
    }

    #[test]
    fn payload_walk_visits_every_payload_module_once() {
        for l in all_layouts() {
            let expected = l.roles.cells().iter().filter(|r| r.is_payload()).count();
            assert_eq!(l.payload.len(), expected);
            let mut seen = ModuleGrid::new(l.side, false);
            for &(x, y) in &l.payload {
                assert!(!*seen.at(x, y), "revisited ({x},{y})");
                seen.set(x, y, true);
            }
        }
    }

    #[test]
    fn every_version_has_usable_payload() {
        for l in all_layouts() {
            let cap = l.capacity();
            assert!(
                cap.luma_bits > 0,
                "v{} {:?} has no payload",
                l.version.get(),
                l.profile
            );
        }
        // v1 C1 is the tightest case in the whole format; assert it is not degenerate.
        let v1 = Layout::new(Version::new(1).unwrap(), Profile::C1);
        assert!(
            v1.capacity().total() >= 240,
            "v1 C1 total {:?}",
            v1.capacity()
        );
    }

    /// §5 quotes 1.00 / 2.00 / 1.25 / 1.50 bits per module. `M` and `C1` hit their
    /// figures exactly. `C2` and `C3` land slightly low — 1.23 and 1.45 at v32 — and
    /// this test pins the real numbers rather than the nominal ones.
    ///
    /// The shortfall is entirely the partial-chroma-cell rule documented on
    /// [`chroma_layout`], and it decomposes cleanly:
    ///
    /// * ~6.3% — one scattered reference per 64 modules disqualifies its whole 2×2 cell.
    /// * ~2.7% — `side` is always odd, so a 2×2 cell grid cannot tile it; the last cell
    ///   row and column always overhang.
    /// * the remainder — cells straddling a fixed pattern boundary.
    ///
    /// §5's figures are the geometric ideal for an infinite field with no references.
    #[test]
    fn bits_per_module_tracks_the_profile_table() {
        let v = Version::new(32).unwrap();
        let ratio = |p: Profile| {
            let l = Layout::new(v, p);
            l.capacity().total() as f32 / l.payload.len() as f32
        };
        assert!(
            (ratio(Profile::M) - 1.00).abs() < 0.001,
            "{}",
            ratio(Profile::M)
        );
        assert!(
            (ratio(Profile::C1) - 2.00).abs() < 0.001,
            "{}",
            ratio(Profile::C1)
        );
        assert!(
            (ratio(Profile::C2) - 1.227).abs() < 0.01,
            "{}",
            ratio(Profile::C2)
        );
        assert!(
            (ratio(Profile::C3) - 1.455).abs() < 0.01,
            "{}",
            ratio(Profile::C3)
        );
    }

    #[test]
    fn finders_have_the_1_1_3_1_1_run_ratio() {
        let l = Layout::new(Version::new(5).unwrap(), Profile::C1);
        // Scan the middle row of the top-left finder plus its separator.
        let dark: Vec<bool> = (0..8).map(|x| l.fixed.at(x, 3).unwrap()).collect();
        // fixed stores `true` = light, so invert to read runs of dark.
        let runs = {
            let mut runs = Vec::new();
            let mut cur = dark[0];
            let mut n = 0;
            for &d in &dark {
                if d == cur {
                    n += 1;
                } else {
                    runs.push(n);
                    cur = d;
                    n = 1;
                }
            }
            runs.push(n);
            runs
        };
        assert_eq!(
            runs,
            alloc::vec![1, 1, 3, 1, 1, 1],
            "dark/light runs across finder: {runs:?}"
        );
    }

    #[test]
    fn timing_and_alignment_agree_where_they_cross() {
        for l in all_layouts() {
            for &(cx, cy) in &l.alignment_centers {
                if cx == 6 || cy == 6 {
                    // Even centre means the marker's dark modules land on even
                    // coordinates, exactly like the timing pattern.
                    assert_eq!(cx % 2, 0);
                    assert_eq!(cy % 2, 0);
                }
            }
        }
    }

    #[test]
    fn alignment_spacing_stays_within_28_modules() {
        for v in Version::all() {
            let c = alignment_centers(v);
            if c.is_empty() {
                continue;
            }
            let mut xs: Vec<u16> = c.iter().map(|p| p.0).collect();
            xs.sort_unstable();
            xs.dedup();
            for w in xs.windows(2) {
                assert!(w[1] - w[0] <= 28, "v{} spacing {:?}", v.get(), w);
            }
            assert_eq!(*xs.last().unwrap(), v.side() - 7);
        }
    }

    #[test]
    fn scattered_references_cost_about_one_module_in_64() {
        let l = Layout::new(Version::new(20).unwrap(), Profile::C1);
        let refs = l.roles.cells().iter().filter(|r| **r == Reference).count();
        let payload_area = refs + l.payload.len();
        let overhead = refs as f32 / payload_area as f32;
        assert!(
            (0.013..0.020).contains(&overhead),
            "reference overhead {overhead}"
        );
    }

    #[test]
    fn chroma_cells_are_fully_payload() {
        for l in all_layouts() {
            let c = l.profile.chroma_cell();
            for &(cx, cy) in &l.chroma_cells {
                for dy in 0..c {
                    for dx in 0..c {
                        assert!(l.roles.at(cx * c + dx, cy * c + dy).is_payload());
                    }
                }
            }
        }
    }

    #[test]
    fn c1_loses_no_chroma_capacity() {
        for v in Version::all() {
            let l = Layout::new(v, Profile::C1);
            assert_eq!(l.chroma_cells.len(), l.payload.len(), "v{}", v.get());
        }
    }

    #[test]
    fn mono_profile_has_no_chroma() {
        let l = Layout::new(Version::new(10).unwrap(), Profile::M);
        assert_eq!(l.capacity().chroma_bits, 0);
        assert!(l.chroma_cells.is_empty());
    }
}
