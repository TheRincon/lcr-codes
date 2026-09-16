//! Module mapping, masking and colourisation — §8.4 through §8.6, and their inverses.
//!
//! Both directions live here rather than being split between `lc-encode` and `lc-decode`,
//! because this is the part of the format where an encoder/decoder disagreement is
//! silent: the symbol still decodes, just to the wrong bits. Keeping the forward map and
//! its inverse adjacent — and round-trip tested against each other — is the cheapest
//! insurance available.
//!
//! ## Bit-to-level convention
//!
//! A luma bit maps straight to its luma level: **`0` is `L0` (dark), `1` is `L1`
//! (light)**. Combined with `index(l, h) = l·k + h` from [`crate::palette`], the palette
//! index of a payload module is `luma_bit · k + hue`, which is §8.6's colourisation step
//! with no lookup table in between.

use crate::grid::ModuleRole;
use crate::layout::{Layout, NO_CHROMA};
use crate::mask;
use crate::symbol::{Symbol, chroma_symbol_to_hue, hue_to_chroma_symbol};
use crate::{FormatInfo, ModuleGrid};
use alloc::vec::Vec;

/// Bits in one format codeword copy.
const FORMAT_BITS: usize = 30;

/// Assemble a symbol from two interleaved layer codewords (§8.4–§8.6).
///
/// `luma_cw` is indexed by payload-walk position and `chroma_cw` by chroma-cell ordinal
/// times `log2(k)`. Both may be short; missing bits read as zero, which is what lets a
/// caller pass a codeword sized to the layer without padding it by hand.
pub fn modulate(
    layout: &Layout,
    format: FormatInfo,
    luma_cw: &[bool],
    chroma_cw: &[bool],
) -> Symbol {
    let hues = layout.profile.hues();
    let hues_u8 = hues as u8;
    let mut modules = ModuleGrid::new(layout.side, 0u8);

    // Fixed patterns and known-colour references.
    for y in 0..layout.side {
        for x in 0..layout.side {
            let idx = match layout.roles.at(x, y) {
                ModuleRole::PaletteAnchor | ModuleRole::Reference => *layout.ref_color.at(x, y),
                ModuleRole::Payload | ModuleRole::Format => continue,
                _ => layout.fixed.at(x, y).unwrap_or(false) as u8 * hues_u8,
            };
            modules.set(x, y, idx);
        }
    }

    // Payload: masked luma bit, masked hue from the covering chroma cell.
    let bpc = layout.profile.chroma_bits_per_cell();
    for (i, &(x, y)) in layout.payload.iter().enumerate() {
        let mut l = luma_cw.get(i).copied().unwrap_or(false);
        if mask::luma_mask(format.mask, x, y) {
            l = !l;
        }
        let cell = *layout.chroma_of.at(x, y);
        let h = if cell == NO_CHROMA || bpc == 0 {
            0
        } else {
            let (cx, cy) = layout.chroma_cells[cell as usize];
            let mut symbol = 0usize;
            for b in 0..bpc {
                symbol = (symbol << 1)
                    | chroma_cw
                        .get(cell as usize * bpc + b)
                        .copied()
                        .unwrap_or(false) as usize;
            }
            mask::chroma_mask_apply(chroma_symbol_to_hue(symbol, hues), cx, cy, hues)
        };
        modules.set(x, y, l as u8 * hues_u8 + h as u8);
    }

    // The ring's payload cells (D19), taking the tail of each layer's codeword. No mask:
    // `mask::luma_mask` is defined over grid coordinates and these cells sit outside it.
    let ring_len = layout.ring_cells;
    let ring_chroma = layout.profile.chroma_cell() == 1 && bpc > 0;
    let mut ring = Vec::with_capacity(ring_len);
    for i in 0..ring_len {
        let l = luma_cw
            .get(layout.payload.len() + i)
            .copied()
            .unwrap_or(false);
        let h = if ring_chroma {
            let base = layout.chroma_cells.len() * bpc + i * bpc;
            let mut symbol = 0usize;
            for b in 0..bpc {
                symbol = (symbol << 1) | chroma_cw.get(base + b).copied().unwrap_or(false) as usize;
            }
            chroma_symbol_to_hue(symbol, hues)
        } else {
            0
        };
        ring.push(l as u8 * hues_u8 + h as u8);
    }

    // Format region: both copies of the same BCH codeword, luma-only.
    let cw = format.to_codeword();
    for copy in &layout.format_cells {
        for (bit, &(x, y)) in copy.iter().enumerate() {
            let l = (cw >> (FORMAT_BITS - 1 - bit)) & 1 == 1;
            modules.set(x, y, l as u8 * hues_u8);
        }
    }

    Symbol {
        format,
        modules,
        ring,
    }
}

/// Recover both layer codewords from a symbol, hard-decision.
///
/// The exact inverse of [`modulate`]. Used by the encoder's round-trip assertions and by
/// the degradation harness; the real decoder works in LLRs (§12.4) and never calls this.
pub fn demodulate(layout: &Layout, symbol: &Symbol) -> (Vec<bool>, Vec<bool>) {
    let hues = layout.profile.hues();
    let bpc = layout.profile.chroma_bits_per_cell();
    let mut luma = Vec::with_capacity(layout.payload.len());
    let mut chroma = alloc::vec![false; layout.chroma_cells.len() * bpc];

    for &(x, y) in &layout.payload {
        let idx = *symbol.modules.at(x, y) as usize;
        let mut l = idx >= hues;
        if mask::luma_mask(symbol.format.mask, x, y) {
            l = !l;
        }
        luma.push(l);

        let cell = *layout.chroma_of.at(x, y);
        if cell == NO_CHROMA || bpc == 0 {
            continue;
        }
        let (cx, cy) = layout.chroma_cells[cell as usize];
        let h = mask::chroma_mask_remove(idx % hues, cx, cy, hues);
        let s = hue_to_chroma_symbol(h, hues);
        for b in 0..bpc {
            chroma[cell as usize * bpc + b] = (s >> (bpc - 1 - b)) & 1 == 1;
        }
    }

    // The ring's payload cells (D19) are the tail of each layer, unmasked.
    let ring_chroma = layout.profile.chroma_cell() == 1 && bpc > 0;
    for &idx in &symbol.ring {
        luma.push(idx as usize >= hues);
        if ring_chroma {
            let s = hue_to_chroma_symbol(idx as usize % hues, hues);
            for b in 0..bpc {
                chroma.push((s >> (bpc - 1 - b)) & 1 == 1);
            }
        }
    }
    (luma, chroma)
}

/// Read the two format codeword copies out of a symbol's luma layer.
pub fn read_format_copies(layout: &Layout, luma: &ModuleGrid<bool>) -> [u32; 2] {
    core::array::from_fn(|copy| {
        let mut w = 0u32;
        for &(x, y) in &layout.format_cells[copy] {
            w = (w << 1) | *luma.at(x, y) as u32;
        }
        w
    })
}

/// Convert per-hue log-likelihoods for one chroma cell into per-bit LLRs (§12.4).
///
/// `hue_ll[h]` is the log-likelihood that the cell was rendered in hue state `h`, before
/// the mask rotation is undone. The output is `log2(k)` LLRs in transmission order, with
/// **positive meaning bit 0**, ready to hand to the LDPC decoder.
///
/// Two format details are folded in here rather than left to the caller, because getting
/// either wrong produces a decoder that half-works:
///
/// * the §8.5 mask rotation, removed before labelling — `cell` is the chroma cell it is
///   keyed on, or `None` for the ring's payload cells (D19), which sit outside the grid the
///   mask is defined over and are written unrotated;
/// * the Gray labelling of hue states, so a confusion between two hues adjacent on the
///   colour wheel costs one bit rather than two.
pub fn chroma_bit_llrs(hue_ll: &[f32], cell: Option<(u16, u16)>, hues: usize, out: &mut Vec<f32>) {
    let bits = hues.trailing_zeros() as usize;
    out.clear();
    for b in 0..bits {
        // max-log approximation: LLR = max{ll : bit b == 0} − max{ll : bit b == 1}.
        let mut best = [f32::NEG_INFINITY; 2];
        for (h_observed, &ll) in hue_ll.iter().enumerate().take(hues) {
            let h = match cell {
                Some((cx, cy)) => mask::chroma_mask_remove(h_observed, cx, cy, hues),
                None => h_observed,
            };
            let s = hue_to_chroma_symbol(h, hues);
            let bit = (s >> (bits - 1 - b)) & 1;
            best[bit] = best[bit].max(ll);
        }
        out.push(best[0] - best[1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::{EccLevel, Profile, Version};

    fn fmt(v: u8, p: Profile, m: u8) -> FormatInfo {
        FormatInfo {
            version: Version::new(v).unwrap(),
            profile: p,
            ecc: EccLevel::DEFAULT,
            mask: m,
            reference_border: false,
        }
    }

    fn pseudo(n: usize, seed: u64) -> Vec<bool> {
        let mut s = seed | 1;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                s >> 62 & 1 == 1
            })
            .collect()
    }

    #[test]
    fn modulate_demodulate_roundtrips_everywhere() {
        for v in [1u8, 2, 7, 14, 32] {
            for p in Profile::ALL {
                for m in 0..8u8 {
                    let f = fmt(v, p, m);
                    let layout = Layout::new(f.version, p);
                    let cap = layout.capacity();
                    let luma = pseudo(cap.luma_bits, 0xABCD ^ v as u64);
                    let chroma = pseudo(cap.chroma_bits, 0x1234 ^ m as u64);
                    let sym = modulate(&layout, f, &luma, &chroma);
                    let (gl, gc) = demodulate(&layout, &sym);
                    assert_eq!(gl, luma, "luma v{v} {p:?} mask {m}");
                    assert_eq!(gc, chroma, "chroma v{v} {p:?} mask {m}");
                }
            }
        }
    }

    #[test]
    fn every_module_gets_a_valid_palette_index() {
        for p in Profile::ALL {
            let f = fmt(6, p, 3);
            let layout = Layout::new(f.version, p);
            let cap = layout.capacity();
            let sym = modulate(
                &layout,
                f,
                &pseudo(cap.luma_bits, 7),
                &pseudo(cap.chroma_bits, 9),
            );
            let n = p.colors() as u8;
            assert!(
                sym.modules.cells().iter().all(|&i| i < n),
                "{p:?} out-of-range index"
            );
        }
    }

    #[test]
    fn fixed_patterns_are_luma_only() {
        for p in [Profile::C1, Profile::C2, Profile::C3] {
            let f = fmt(9, p, 0);
            let layout = Layout::new(f.version, p);
            let cap = layout.capacity();
            let sym = modulate(
                &layout,
                f,
                &pseudo(cap.luma_bits, 1),
                &pseudo(cap.chroma_bits, 2),
            );
            let hues = p.hues();
            for y in 0..layout.side {
                for x in 0..layout.side {
                    if matches!(
                        layout.roles.at(x, y),
                        ModuleRole::Finder
                            | ModuleRole::Separator
                            | ModuleRole::Timing
                            | ModuleRole::Alignment
                            | ModuleRole::Format
                    ) {
                        assert_eq!(*sym.modules.at(x, y) as usize % hues, 0, "({x},{y}) not H0");
                    }
                }
            }
        }
    }

    #[test]
    fn format_region_survives_the_round_trip() {
        for v in [1u8, 5, 32] {
            for p in Profile::ALL {
                let f = fmt(v, p, 6);
                let layout = Layout::new(f.version, p);
                let sym = modulate(&layout, f, &[], &[]);
                let copies = read_format_copies(&layout, &sym.luma_grid());
                assert_eq!(copies[0], f.to_codeword(), "v{v} {p:?} copy A");
                assert_eq!(copies[1], f.to_codeword(), "v{v} {p:?} copy B");
                assert_eq!(FormatInfo::from_copies(copies[0], copies[1]).unwrap(), f);
            }
        }
    }

    #[test]
    fn the_luma_layer_alone_is_a_complete_monochrome_code() {
        // §1.1 goal 3 / §8.1: dropping chroma entirely must still yield the luma bits.
        let f = fmt(8, Profile::C1, 2);
        let layout = Layout::new(f.version, Profile::C1);
        let cap = layout.capacity();
        let luma = pseudo(cap.luma_bits, 42);
        let sym = modulate(&layout, f, &luma, &pseudo(cap.chroma_bits, 43));

        let grid = sym.luma_grid();
        let recovered: Vec<bool> = layout
            .payload
            .iter()
            .map(|&(x, y)| *grid.at(x, y) ^ mask::luma_mask(f.mask, x, y))
            .collect();
        assert_eq!(recovered, luma);
    }

    #[test]
    fn chroma_bit_llrs_recover_the_transmitted_symbol() {
        for p in [Profile::C1, Profile::C3] {
            let hues = p.hues();
            let bits = hues.trailing_zeros() as usize;
            for cy in 0..3u16 {
                for cx in 0..3u16 {
                    for s in 0..hues {
                        // Render symbol `s` in this cell, then hand the demodulator a
                        // confident likelihood for the hue that was actually drawn.
                        let h =
                            mask::chroma_mask_apply(chroma_symbol_to_hue(s, hues), cx, cy, hues);
                        let ll: Vec<f32> = (0..hues)
                            .map(|i| if i == h { 0.0 } else { -12.0 })
                            .collect();
                        let mut out = Vec::new();
                        chroma_bit_llrs(&ll, Some((cx, cy)), hues, &mut out);
                        assert_eq!(out.len(), bits);
                        let got: usize = out
                            .iter()
                            .fold(0, |acc, &l| (acc << 1) | usize::from(l < 0.0));
                        assert_eq!(got, s, "{p:?} cell ({cx},{cy}) symbol {s}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_hue_confusion_costs_one_bit_not_two() {
        // Two hues adjacent on the wheel, equally likely: exactly one LLR must be
        // ambiguous. This is the Gray labelling doing its job.
        let hues = 4;
        let ll: Vec<f32> = alloc::vec![0.0, 0.0, -12.0, -12.0];
        let mut out = Vec::new();
        chroma_bit_llrs(&ll, Some((0, 0)), hues, &mut out);
        let ambiguous = out.iter().filter(|l| l.abs() < 1e-6).count();
        assert_eq!(ambiguous, 1, "{out:?}");
    }
}
