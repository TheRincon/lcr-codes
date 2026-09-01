//! The rendered symbol: a grid of palette indices, plus its format information.
//!
//! This is the boundary object between encoding (§8) and rendering (§14.1) — and the
//! thing a decoder reconstructs. It deliberately stores *palette indices* rather than
//! colours, so the same symbol can be rendered for screen, print, or a monochrome
//! fallback without re-running the pipeline.

use crate::color::Srgb8;
use crate::format::FormatInfo;
use crate::grid::ModuleGrid;
use crate::layout::Layout;
use crate::palette::Palette;

/// A complete LC Code symbol.
#[derive(Clone, Debug)]
pub struct Symbol {
    /// Version, profile, ECC levels, mask and border flag (§7).
    pub format: FormatInfo,
    /// Palette index of every module, `index(l, h) = l·k + h`.
    pub modules: ModuleGrid<u8>,
}

impl Symbol {
    /// Grid side in modules, excluding the quiet zone.
    #[inline]
    pub fn side(&self) -> u16 {
        self.modules.side()
    }

    /// The palette this symbol's profile uses.
    pub fn palette(&self) -> Palette {
        Palette::for_profile(self.format.profile)
    }

    /// Rebuild the geometry. Deterministic from [`Symbol::format`].
    pub fn layout(&self) -> Layout {
        Layout::new(self.format.version, self.format.profile)
    }

    /// Colour of a module.
    #[inline]
    pub fn color_at(&self, palette: &Palette, x: u16, y: u16) -> Srgb8 {
        palette.colors[*self.modules.at(x, y) as usize]
    }

    /// Luma bit of a module: `true` is light (L1).
    ///
    /// This is the monochrome view of the symbol — §1.1 goal 3's graceful degradation is
    /// exactly this projection, and it is a complete monochrome code on its own.
    #[inline]
    pub fn luma_at(&self, x: u16, y: u16) -> bool {
        *self.modules.at(x, y) as usize >= self.format.profile.hues()
    }

    /// The luma layer as its own grid.
    pub fn luma_grid(&self) -> ModuleGrid<bool> {
        let hues = self.format.profile.hues() as u8;
        self.modules.map(|&i| i >= hues)
    }
}

/// Reflected binary code: `gray(n)` and `gray(n+1)` differ in exactly one bit.
#[inline]
pub const fn gray(n: usize) -> usize {
    n ^ (n >> 1)
}

/// Inverse of [`gray`].
#[inline]
pub const fn gray_inverse(mut g: usize) -> usize {
    let mut n = 0;
    while g != 0 {
        n ^= g;
        g >>= 1;
    }
    n
}

/// Map a chroma data symbol to a hue state (§12.4).
///
/// Hue states are numbered by increasing `a*b*` angle, so states `h` and `h+1` are
/// *neighbours on the colour wheel* and are the pair a noisy chroma sample is most likely
/// to confuse. Labelling them with a Gray code means such a confusion costs one bit
/// rather than two, which is what makes the per-bit chroma LLRs of §12.4 well-founded:
/// a bit's reliability is then a property of one decision boundary, not two.
#[inline]
pub const fn chroma_symbol_to_hue(symbol: usize, hues: usize) -> usize {
    gray_inverse(symbol % hues) % hues
}

/// Inverse of [`chroma_symbol_to_hue`].
#[inline]
pub const fn hue_to_chroma_symbol(hue: usize, hues: usize) -> usize {
    gray(hue % hues) % hues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_roundtrips() {
        for n in 0..64usize {
            assert_eq!(gray_inverse(gray(n)), n);
        }
    }

    #[test]
    fn adjacent_hues_differ_in_one_bit() {
        for hues in [2usize, 4] {
            for h in 0..hues {
                let a = hue_to_chroma_symbol(h, hues);
                let b = hue_to_chroma_symbol((h + 1) % hues, hues);
                assert_eq!((a ^ b).count_ones(), 1, "hues={hues} h={h}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn chroma_symbol_mapping_is_a_bijection() {
        for hues in [2usize, 4] {
            let mut seen = alloc::vec![false; hues];
            for s in 0..hues {
                let h = chroma_symbol_to_hue(s, hues);
                assert!(!seen[h]);
                seen[h] = true;
                assert_eq!(hue_to_chroma_symbol(h, hues), s);
            }
        }
    }
}
