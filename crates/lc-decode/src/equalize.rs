//! Re-reading a symbol with neighbour bleed removed (D24, D25).
//!
//! Defocus is what binds a ringed symbol once it is located: at blur radius 5 on `C1` every
//! decode the harness lost had a converged luma layer and a failed chroma layer, and at
//! radius 7–8 luma went too. The cause is inter-symbol interference. A module's sampled
//! colour is its own plus a fraction of its neighbours', and `C1`'s two hues are
//! near-opposites, so a few per cent of bleed from each side is enough to pull a module
//! across the hue boundary — ~9% of chroma bits arrived wrong at radius 5.
//!
//! The bleed is cancellable because nearly everything around a module is known or nearly
//! known:
//!
//! * fixed patterns, references, the ring, the gap and the quiet zone, a priori;
//! * every payload module's **level** — exactly, once luma has converged (from its
//!   codeword), or as a first-pass decision before that;
//! * hence every cell's nominal **luminance**, because the hue states are iso-luminant
//!   (§4.1), and so the **blur kernel itself**, fitted on luminance over thousands of cells;
//! * and each payload neighbour's hue: fixed for modules outside any chroma cell, otherwise
//!   a first-pass decision, shared across a chroma cell's modules.
//!
//! Each module's neighbours are estimated from those, their share subtracted, and the module
//! re-demodulated. Measured against an oracle given the true neighbour colours, first-pass
//! decisions lose nothing: at radius 5, 85–112 wrong chroma bits of 1,170 became 0–13
//! either way, and every trial's chroma code converged.
//!
//! The equaliser reads an [`Observation`] — every cell's corrected colour — rather than an
//! image, so the same code serves a single frame and the running mean
//! [`crate::FrameAccumulator`] keeps (D25).

use crate::demod::{ChannelStats, LLR_CLAMP};
use crate::illum::IlluminationModel;
use crate::image::ImageView;
use crate::math::least_squares;
use crate::sample::sample_module;
use crate::transform::Transform;
use alloc::vec;
use alloc::vec::Vec;
use lc_core::border::{self, RingMode};
use lc_core::color::{self, Linear};
use lc_core::layout::{Layout, NO_CHROMA};
use lc_core::mask;
use lc_core::modulate::chroma_bit_llrs;
use lc_core::palette::{BORDER_WHITE, Palette};

/// Smallest centre weight a fitted kernel may have. Below it the fit has found something
/// other than blur, and dividing by it would amplify noise.
const MIN_CENTRE: f64 = 0.4;

/// Decision-feedback passes: each after the first re-decides neighbours from the previous
/// pass's equalised colours.
const PASSES: usize = 2;

/// Luma LLR magnitude a first-pass decision needs before the kernel fit trusts its level.
const CONFIDENT: f32 = 4.0;

/// Every payload module's §12.3-corrected colour, on the dense grid the equaliser works in —
/// summed over however many frames have been added, so a multi-frame mean costs one grid
/// rather than one per frame.
///
/// Payload modules are observed by default. They come free from demodulation, which samples
/// each one with the same kernel, and they are the only cells whose colour the equaliser
/// ever subtracts from; the cells around them enter as nominal colours. Sampling every cell
/// for every accumulated frame cost 34 ms per v32 frame, so [`Observation::surround`] does it
/// only for a single frame's last retry (D25).
#[derive(Clone, Debug)]
pub struct Observation {
    reach: i32,
    n: i32,
    sum: Vec<Linear>,
    seen: Vec<bool>,
    frames: f32,
}

impl Observation {
    /// One frame's payload colours, in [`crate::demod::Demodulation::colours`] order.
    /// `None` when the colours do not match the layout.
    pub fn new(layout: &Layout, ringed: bool, colours: &[Linear]) -> Option<Observation> {
        let reach = reach(ringed);
        let n = layout.side as i32 + 2 * (reach + 1);
        let mut sum = vec![Linear::new(0.0, 0.0, 0.0); (n * n) as usize];
        let mut seen = vec![false; sum.len()];
        let mut count = 0;
        for ((x, y), &c) in layout.luma_positions().zip(colours) {
            let i = index(reach, n, x, y);
            (sum[i], seen[i]) = (c, true);
            count += 1;
        }
        (count == colours.len() && count == layout.capacity().luma_bits).then_some(Observation {
            reach,
            n,
            sum,
            seen,
            frames: 1.0,
        })
    }

    /// Sample every cell around the payload as well — references, ring, gap and quiet zone —
    /// so the kernel fit can use them as targets. The retry's second rung for a single frame
    /// (D25); an accumulated mean has no frame to sample.
    pub fn surround(
        &mut self,
        img: &ImageView,
        t: &Transform,
        layout: &Layout,
        illum: &IlluminationModel,
    ) {
        let (side, reach) = (layout.side as i32, self.reach);
        for y in -reach..side + reach {
            for x in -reach..side + reach {
                let i = index(reach, self.n, x, y);
                if !self.seen[i] {
                    let raw = sample_module(img, t, x, y);
                    let c = illum.correct(x as f32 + 0.5, y as f32 + 0.5, raw);
                    self.sum[i] =
                        Linear::new(c.r * self.frames, c.g * self.frames, c.b * self.frames);
                    self.seen[i] = true;
                }
            }
        }
    }

    /// Fold another frame of the same symbol in. A mismatched grid is ignored.
    pub fn add(&mut self, other: &Observation) {
        if other.n != self.n {
            return;
        }
        for (a, b) in self.sum.iter_mut().zip(&other.sum) {
            a.r += b.r;
            a.g += b.g;
            a.b += b.b;
        }
        self.frames += other.frames;
    }

    /// Mean colour of the cell at grid index `i`.
    fn colour(&self, i: usize) -> Linear {
        let s = self.sum[i];
        Linear::new(s.r / self.frames, s.g / self.frames, s.b / self.frames)
    }
}

/// Cells out to the quiet zone's edge, beyond the grid.
fn reach(ringed: bool) -> i32 {
    if ringed {
        border::GAP + border::WIDTH + border::QUIET
    } else {
        border::QUIET
    }
}

fn index(reach: i32, n: i32, x: i32, y: i32) -> usize {
    let m = reach + 1;
    ((y + m) * n + (x + m)) as usize
}

/// Where a payload module's hue comes from.
#[derive(Copy, Clone)]
enum Hue {
    /// Rendered at a fixed hue: modules outside any chroma cell, and `C2`'s ring cells.
    Fixed(usize),
    /// Shared with the rest of grid chroma cell `n`.
    Cell(u32),
    /// Its own: a ring payload cell carrying one-module chroma.
    Own,
}

/// Where each luma position's hue comes from, in [`Layout::luma_positions`] order. Follows
/// `lc-core`'s `modulate`: a grid module takes its chroma cell's hue, or 0 outside one; a
/// ring cell carries its own only where the chroma cell is one module, and is drawn at 0
/// otherwise.
fn hue_sources(layout: &Layout) -> Vec<Hue> {
    let ring_chroma =
        layout.profile.chroma_cell() == 1 && layout.profile.chroma_bits_per_cell() > 0;
    let grid = layout.payload.len();
    layout
        .luma_positions()
        .enumerate()
        .map(|(k, (x, y))| {
            if k < grid {
                match *layout.chroma_of.at(x as u16, y as u16) {
                    NO_CHROMA => Hue::Fixed(0),
                    c => Hue::Cell(c),
                }
            } else if ring_chroma {
                Hue::Own
            } else {
                Hue::Fixed(0)
            }
        })
        .collect()
}

/// The hue every luma position was drawn in, given one hue per chroma cell in demodulation
/// order — grid cells, then the ring's own — as [`chroma_hues`] produces. The one place that
/// maps per-cell hues back onto modules.
pub fn drawn_hues(layout: &Layout, cell_hue: &[usize]) -> Vec<usize> {
    let (grid, cells) = (layout.payload.len(), layout.chroma_cells.len());
    hue_sources(layout)
        .into_iter()
        .enumerate()
        .map(|(k, h)| match h {
            Hue::Fixed(v) => v,
            Hue::Cell(c) => cell_hue.get(c as usize).copied().unwrap_or(0),
            Hue::Own => cell_hue.get(cells + k - grid).copied().unwrap_or(0),
        })
        .collect()
}

/// What is known about one cell before equalisation.
#[derive(Copy, Clone)]
enum Cell {
    /// Nominal colour known a priori: a fixed pattern, a reference, ring, gap or quiet zone.
    Known(Linear),
    /// A payload module: luma level (exact or decided), whether the kernel fit may rely on
    /// it, and where its hue comes from.
    Payload { light: bool, sure: bool, hue: Hue },
    /// Background beyond the quiet zone: nothing known.
    Unknown,
}

/// The cells around a symbol, out to its quiet zone, in one dense grid.
struct Field<'a> {
    layout: &'a Layout,
    palette: &'a Palette,
    obs: &'a Observation,
    cells: Vec<Cell>,
    /// Luma positions in LLR order: grid payload, then ring payload cells.
    coords: Vec<(i32, i32)>,
    /// Grid indices of each chroma cell's modules.
    members: Vec<Vec<usize>>,
}

impl<'a> Field<'a> {
    fn new(
        layout: &'a Layout,
        palette: &'a Palette,
        obs: &'a Observation,
        ring: Option<RingMode>,
        light: &[bool],
        sure: &[bool],
    ) -> Option<Field<'a>> {
        let side = layout.side as i32;
        let reach = obs.reach;
        if reach != self::reach(ring.is_some()) || obs.n != side + 2 * (reach + 1) {
            return None;
        }
        let mut f = Field {
            layout,
            palette,
            obs,
            cells: vec![Cell::Unknown; obs.sum.len()],
            coords: layout.luma_positions().collect(),
            members: vec![Vec::new(); layout.chroma_cells.len()],
        };
        if f.coords.len() != light.len() || light.len() != sure.len() {
            return None;
        }

        let white = color::srgb8_to_linear(BORDER_WHITE);
        for y in -reach..side + reach {
            for x in -reach..side + reach {
                let in_grid = (0..side).contains(&x) && (0..side).contains(&y);
                let cell = if in_grid {
                    layout
                        .known_color(x as u16, y as u16, palette)
                        .map_or(Cell::Unknown, |c| Cell::Known(color::srgb8_to_linear(c)))
                } else if let Some(c) =
                    ring.and_then(|mode| border::cell_color_in(x, y, layout.side, palette, mode))
                {
                    Cell::Known(color::srgb8_to_linear(c))
                } else {
                    Cell::Known(white) // gap and quiet zone
                };
                let i = f.at(x, y);
                f.cells[i] = cell;
            }
        }
        // Payload overrides: the ring's inner data cells carry payload too (D19).
        for (k, (&(x, y), hue)) in f.coords.clone().iter().zip(hue_sources(layout)).enumerate() {
            let i = f.at(x, y);
            if let Hue::Cell(c) = hue {
                f.members.get_mut(c as usize)?.push(i);
            }
            f.cells[i] = Cell::Payload {
                light: light[k],
                sure: sure[k],
                hue,
            };
        }
        Some(f)
    }

    fn at(&self, x: i32, y: i32) -> usize {
        index(self.obs.reach, self.obs.n, x, y)
    }

    /// Fit `obs ≈ k0·centre + k1·Σedge + k2·Σdiagonal + kb` on luminance, from every cell
    /// whose 3×3 neighbourhood is known or surely decided. A payload module's nominal
    /// luminance is its level's whatever its hue, so this needs no chroma decision.
    fn kernel(&self) -> Option<[f32; 4]> {
        let hues = self.palette.hues;
        let level_y = [
            self.palette.linear[0].luminance(),
            self.palette.linear[hues].luminance(),
        ];
        let (side, reach) = (self.layout.side as i32, self.obs.reach);
        let mut rows = Vec::new();
        for y in -reach + 1..side + reach - 1 {
            for x in -reach + 1..side + reach - 1 {
                let mut sums = [0.0f64; 3]; // centre, edge, diagonal
                let mut ok = true;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let v = match self.cells[self.at(x + dx, y + dy)] {
                            Cell::Known(k) => k.luminance(),
                            Cell::Payload {
                                light, sure: true, ..
                            } => level_y[usize::from(light)],
                            _ => {
                                ok = false;
                                continue;
                            }
                        };
                        sums[(dx.abs() + dy.abs()) as usize] += v as f64;
                    }
                }
                let centre = self.at(x, y);
                if ok && self.obs.seen[centre] {
                    let target = self.obs.colour(centre).luminance() as f64;
                    rows.push((vec![sums[0], sums[1], sums[2], 1.0], target));
                }
            }
        }
        let k = least_squares(&rows, 4)?;
        (k[0] >= MIN_CENTRE).then(|| [k[0] as f32, k[1] as f32, k[2] as f32, k[3] as f32])
    }

    /// Nearest hue centroid to a colour, in `a*b*`.
    fn nearest(&self, c: Linear) -> usize {
        let lab = color::linear_to_lab(c);
        let p = self.palette;
        let d = |h: usize| (lab.a - p.hue_ab[h][0]).powi(2) + (lab.b - p.hue_ab[h][1]).powi(2);
        (0..p.hues)
            .min_by(|&i, &j| d(i).total_cmp(&d(j)))
            .unwrap_or(0)
    }

    /// Mean of `colours` over a chroma cell's modules.
    fn cell_mean(&self, c: usize, colours: &[Linear]) -> Linear {
        let m = &self.members[c];
        let k = m.len().max(1) as f32;
        let (mut r, mut g, mut b) = (0.0, 0.0, 0.0);
        for &i in m {
            (r, g, b) = (r + colours[i].r, g + colours[i].g, b + colours[i].b);
        }
        Linear::new(r / k, g / k, b / k)
    }

    /// Decide every payload module's hue from `colours`: a chroma cell by its mean, a lone
    /// module by itself, a fixed one not at all.
    fn decide_hues(&self, colours: &[Linear], hue: &mut [usize]) {
        let cell_hue: Vec<usize> = (0..self.members.len())
            .map(|c| self.nearest(self.cell_mean(c, colours)))
            .collect();
        for &(x, y) in &self.coords {
            let i = self.at(x, y);
            if let Cell::Payload { hue: h, .. } = self.cells[i] {
                hue[i] = match h {
                    Hue::Fixed(v) => v,
                    Hue::Cell(c) => cell_hue[c as usize],
                    Hue::Own => self.nearest(colours[i]),
                };
            }
        }
    }

    /// Set every payload module's hue from `prior`, one physical hue per chroma cell in
    /// demodulation order (grid cells, then the ring's own).
    fn apply_prior(&self, prior: &[usize], hue: &mut [usize]) {
        for (&(x, y), h) in self.coords.iter().zip(drawn_hues(self.layout, prior)) {
            hue[self.at(x, y)] = h;
        }
    }

    /// Equalised colour of every payload cell, by decision feedback. Each pass takes the
    /// neighbours' nominal colours — level from `cells`, hue from the previous pass or from
    /// `prior` — and subtracts their share. `relevel` lets a pass also re-decide levels, for
    /// when luma itself is what is being rescued.
    fn equalise(
        &mut self,
        k: [f32; 4],
        relevel: Option<&ChannelStats>,
        prior: Option<&[usize]>,
    ) -> Vec<Linear> {
        let [k0, k1, k2, kb] = k;
        let hues = self.palette.hues;
        let palette = self.palette;
        let obs: Vec<Linear> = (0..self.obs.sum.len())
            .map(|i| self.obs.colour(i))
            .collect();
        let mut hue = vec![0usize; obs.len()];
        match prior {
            Some(p) => self.apply_prior(p, &mut hue),
            None => self.decide_hues(&obs, &mut hue),
        }
        let mut out = obs.clone();
        for pass in 0..PASSES {
            let nominal = |i: usize| match self.cells[i] {
                Cell::Known(k) => Some(k),
                Cell::Payload { light, .. } => {
                    Some(palette.linear[usize::from(light) * hues + hue[i]])
                }
                Cell::Unknown => None,
            };
            for &(x, y) in &self.coords {
                let i = self.at(x, y);
                let (mut edge, mut diag) = ([0.0f32; 3], [0.0f32; 3]);
                let mut ok = true;
                for (dx, dy) in [
                    (1, 0),
                    (-1, 0),
                    (0, 1),
                    (0, -1),
                    (1, 1),
                    (-1, 1),
                    (1, -1),
                    (-1, -1),
                ] {
                    let Some(c) = nominal(self.at(x + dx, y + dy)) else {
                        ok = false;
                        break;
                    };
                    let acc = if dx == 0 || dy == 0 {
                        &mut edge
                    } else {
                        &mut diag
                    };
                    acc[0] += c.r;
                    acc[1] += c.g;
                    acc[2] += c.b;
                }
                let o = obs[i];
                out[i] = if ok {
                    let f = |v: f32, c: usize| (v - k1 * edge[c] - k2 * diag[c] - kb) / k0;
                    Linear::new(f(o.r, 0), f(o.g, 1), f(o.b, 2))
                } else {
                    o
                };
            }
            if pass + 1 < PASSES {
                // A decoder's prior is better than anything re-decided from colour here.
                if prior.is_none() {
                    self.decide_hues(&out, &mut hue);
                }
                if let Some(stats) = relevel {
                    for &(x, y) in &self.coords {
                        let i = self.at(x, y);
                        if let Cell::Payload { sure, hue: h, .. } = self.cells[i] {
                            let l = color::linear_to_lab(out[i]).l;
                            self.cells[i] = Cell::Payload {
                                light: stats.luma_llr(l) < 0.0,
                                sure,
                                hue: h,
                            };
                        }
                    }
                }
            }
        }
        out
    }
}

/// Chroma LLRs re-demodulated with neighbour bleed subtracted, in the order
/// [`crate::demod::demodulate`] produces them; `None` without a chroma layer or a usable
/// kernel.
///
/// `light` is the physical luma level of every luma position, from the converged luma
/// layer by [`luma_levels`]. `ring` is the symbol's ring mode, `None` without a ring.
/// `prior` is the hues a previous, unconverged chroma decode settled on, from
/// [`chroma_hues`] — turbo equalisation (D25) — and otherwise hues are decided from colour.
pub fn chroma(
    obs: &Observation,
    layout: &Layout,
    palette: &Palette,
    stats: &ChannelStats,
    ring: Option<RingMode>,
    light: &[bool],
    prior: Option<&[usize]>,
) -> Option<Vec<f32>> {
    let bpc = layout.profile.chroma_bits_per_cell();
    if bpc == 0 {
        return None;
    }
    let sure = vec![true; light.len()];
    let mut f = Field::new(layout, palette, obs, ring, light, &sure)?;
    let k = f.kernel()?;
    let eq = f.equalise(k, None, prior);

    // A chroma cell is read from the mean of its equalised modules — four of them on `C2` —
    // then the ring's own one-module cells, in demodulation order.
    let (mut ll, mut bits) = (Vec::new(), Vec::new());
    let mut llrs = Vec::with_capacity(layout.capacity().chroma_bits);
    let mut push = |c: Linear, cell: Option<(u16, u16)>, llrs: &mut Vec<f32>| {
        let lab = color::linear_to_lab(c);
        stats.hue_log_likelihoods(lab, palette, &mut ll);
        chroma_bit_llrs(&ll, cell, palette.hues, &mut bits);
        let w = stats.reliability(lab, palette);
        llrs.extend(bits.iter().map(|v| (v * w).clamp(-LLR_CLAMP, LLR_CLAMP)));
    };
    for (c, &cell) in layout.chroma_cells.iter().enumerate() {
        push(f.cell_mean(c, &eq), Some(cell), &mut llrs);
    }
    if layout.profile.chroma_cell() == 1 {
        for &(x, y) in &f.coords[layout.payload.len()..] {
            push(eq[f.at(x, y)], None, &mut llrs);
        }
    }
    Some(llrs)
}

/// Luma LLRs re-demodulated with neighbour bleed subtracted, for when luma itself failed.
///
/// `first` is the first pass's LLRs — physical and unmasked, in
/// [`crate::demod::demodulate`] order. Levels come from `prior` when given — the physical
/// levels a previous, unconverged LDPC decode settled on, which are much better than the
/// channel's own (turbo equalisation, D25) — and otherwise from the signs of `first`, of
/// which only the confident ones are trusted for the kernel fit. The equalisation pass then
/// re-decides them.
pub fn luma(
    obs: &Observation,
    layout: &Layout,
    palette: &Palette,
    stats: &ChannelStats,
    ring: Option<RingMode>,
    first: &[f32],
    prior: Option<&[bool]>,
) -> Option<Vec<f32>> {
    // Positive LLR means dark.
    let (light, sure): (Vec<bool>, Vec<bool>) = match prior {
        Some(levels) => (levels.to_vec(), vec![true; levels.len()]),
        None => (
            first.iter().map(|&v| v < 0.0).collect(),
            first.iter().map(|v| v.abs() >= CONFIDENT).collect(),
        ),
    };
    let mut f = Field::new(layout, palette, obs, ring, &light, &sure)?;
    let k = f.kernel()?;
    let eq = f.equalise(k, Some(stats), None);
    let with_chroma = layout.profile.chroma_bits_per_cell() > 0;
    Some(
        f.coords
            .iter()
            .map(|&(x, y)| {
                let lab = color::linear_to_lab(eq[f.at(x, y)]);
                let w = if with_chroma {
                    stats.reliability(lab, palette)
                } else {
                    1.0
                };
                (stats.luma_llr(lab.l) * w).clamp(-LLR_CLAMP, LLR_CLAMP)
            })
            .collect(),
    )
}

/// Physical hue of every chroma cell, in demodulation order, from a chroma layer's codeword
/// (transmission order, before interleaving): the inverse of what `lc-core`'s `modulate`
/// does with it. Grid cells are masked (§8.5); the ring's one-module cells are not.
pub fn chroma_hues(codeword: &[bool], layout: &Layout, hues: usize) -> Vec<usize> {
    let placed = lc_core::interleave::Interleaver::new(codeword.len()).interleave(codeword);
    let bpc = layout.profile.chroma_bits_per_cell();
    let symbol = |base: usize| {
        (0..bpc).fold(0usize, |s, b| {
            (s << 1) | usize::from(placed.get(base + b).copied().unwrap_or(false))
        })
    };
    let mut out: Vec<usize> = layout
        .chroma_cells
        .iter()
        .enumerate()
        .map(|(c, &(cx, cy))| {
            let h = lc_core::symbol::chroma_symbol_to_hue(symbol(c * bpc), hues);
            mask::chroma_mask_apply(h, cx, cy, hues)
        })
        .collect();
    if layout.profile.chroma_cell() == 1 && bpc > 0 {
        let base = layout.chroma_cells.len() * bpc;
        out.extend(
            (0..layout.ring_cells)
                .map(|i| lc_core::symbol::chroma_symbol_to_hue(symbol(base + i * bpc), hues)),
        );
    }
    out
}

/// Physical luma level of every luma position, from a converged layer's codeword
/// (`codeword` in transmission order, before interleaving). The inverse of what
/// `lc-core`'s `modulate` does with it: interleave, then flip by the §8.5 mask on grid
/// modules; ring cells are unmasked.
pub fn luma_levels(codeword: &[bool], layout: &Layout, mask_id: u8) -> Vec<bool> {
    let placed = lc_core::interleave::Interleaver::new(codeword.len()).interleave(codeword);
    placed
        .iter()
        .enumerate()
        .map(|(i, &b)| match layout.payload.get(i) {
            Some(&(x, y)) => b ^ mask::luma_mask(mask_id, x, y),
            None => b,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lc_core::FormatInfo;
    use lc_core::interleave::Interleaver;
    use lc_core::modulate::modulate;
    use lc_core::version::{EccLevel, Profile, Version};

    /// `luma_levels` and `chroma_hues` must be exact inverses of the encoder's `modulate` —
    /// interleave, mask and Gray label included — on every profile, or the turbo rounds
    /// would feed the equaliser the wrong neighbours.
    #[test]
    fn levels_and_hues_invert_modulate() {
        for profile in [Profile::C1, Profile::C2, Profile::C3] {
            let layout = Layout::for_symbol(Version::new(3).unwrap(), profile, true);
            let cap = layout.capacity();
            let format = FormatInfo {
                version: Version::new(3).unwrap(),
                profile,
                ecc: EccLevel::DEFAULT,
                mask: 5,
                reference_border: true,
            };
            let luma: Vec<bool> = (0..cap.luma_bits).map(|i| (i * 7 + 3) % 5 < 2).collect();
            let chroma: Vec<bool> = (0..cap.chroma_bits)
                .map(|i| (i * 11 + 1) % 3 == 0)
                .collect();
            let sym = modulate(
                &layout,
                format,
                &Interleaver::new(luma.len()).interleave(&luma),
                &Interleaver::new(chroma.len()).interleave(&chroma),
            );
            let hues = profile.hues();
            let light = luma_levels(&luma, &layout, format.mask);
            let cell_hue = chroma_hues(&chroma, &layout, hues);
            for (k, &(x, y)) in layout.payload.iter().enumerate() {
                let idx = *sym.modules.at(x, y) as usize;
                assert_eq!(idx >= hues, light[k], "{profile:?} level at ({x},{y})");
                let c = *layout.chroma_of.at(x, y);
                let want = if c == NO_CHROMA {
                    0
                } else {
                    cell_hue[c as usize]
                };
                assert_eq!(idx % hues, want, "{profile:?} hue at ({x},{y})");
            }
            let grid = layout.payload.len();
            for (i, &idx) in sym.ring.iter().enumerate() {
                assert_eq!(
                    idx as usize >= hues,
                    light[grid + i],
                    "{profile:?} ring level {i}"
                );
                if profile.chroma_cell() == 1 {
                    let want = cell_hue[layout.chroma_cells.len() + i];
                    assert_eq!(idx as usize % hues, want, "{profile:?} ring hue {i}");
                }
            }
        }
    }
}
