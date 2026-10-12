//! Generic module grid and the module-role taxonomy (§14.3).

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

/// What a module is for. Determines whether it carries payload, how it is rendered, and
/// whether masking applies (§6.2, §6.3, §8.5).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum ModuleRole {
    /// Part of a 7×7 finder bullseye (§6.2).
    Finder,
    /// One-module light separator around a finder (§6.2).
    Separator,
    /// Alternating row 6 / column 6 (§6.2).
    Timing,
    /// Part of a 5×5 alignment bullseye (§6.2).
    Alignment,
    /// Known-colour anchor on the ring outside a finder separator (§6.2).
    PaletteAnchor,
    /// Format region carrying a BCH(30,15) codeword (§7).
    Format,
    /// Scattered known-colour reference (§6.3).
    Reference,
    /// Reference border outside the quiet zone (§6.4). Never appears inside the grid;
    /// present for API completeness and used by the renderer.
    Border,
    /// Carries payload bits.
    Payload,
    /// Quiet zone (§6.1). Never appears inside the grid.
    Quiet,
}

impl ModuleRole {
    /// True when this module carries payload bits and is therefore masked (§8.5).
    #[inline]
    pub const fn is_payload(self) -> bool {
        matches!(self, ModuleRole::Payload)
    }

    /// True when this module's colour is known to the decoder a priori and can be used
    /// to fit the §12.3 illumination model.
    #[inline]
    pub const fn is_reference(self) -> bool {
        matches!(
            self,
            ModuleRole::PaletteAnchor | ModuleRole::Reference | ModuleRole::Border
        )
    }
}

/// A `side × side` array of per-module values.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModuleGrid<T> {
    side: u16,
    cells: Box<[T]>,
}

impl<T: Clone> ModuleGrid<T> {
    /// Fill a `side × side` grid with `fill`.
    pub fn new(side: u16, fill: T) -> ModuleGrid<T> {
        ModuleGrid {
            side,
            cells: vec![fill; side as usize * side as usize].into_boxed_slice(),
        }
    }
}

impl<T> ModuleGrid<T> {
    /// Grid side in modules.
    #[inline]
    pub const fn side(&self) -> u16 {
        self.side
    }

    /// True when `(x, y)` is inside the grid.
    #[inline]
    pub const fn contains(&self, x: u16, y: u16) -> bool {
        x < self.side && y < self.side
    }

    #[inline]
    const fn idx(&self, x: u16, y: u16) -> usize {
        y as usize * self.side as usize + x as usize
    }

    /// Read `(x, y)`, or `None` if out of bounds.
    #[inline]
    pub fn get(&self, x: u16, y: u16) -> Option<&T> {
        if self.contains(x, y) {
            Some(&self.cells[self.idx(x, y)])
        } else {
            None
        }
    }

    /// Mutable access to `(x, y)`, or `None` if out of bounds.
    #[inline]
    pub fn get_mut(&mut self, x: u16, y: u16) -> Option<&mut T> {
        if self.contains(x, y) {
            let i = self.idx(x, y);
            Some(&mut self.cells[i])
        } else {
            None
        }
    }

    /// Read `(x, y)`. Panics if out of bounds.
    #[inline]
    pub fn at(&self, x: u16, y: u16) -> &T {
        &self.cells[self.idx(x, y)]
    }

    /// Write `(x, y)`. Panics if out of bounds.
    #[inline]
    pub fn set(&mut self, x: u16, y: u16, v: T) {
        let i = self.idx(x, y);
        self.cells[i] = v;
    }

    /// Row-major view of every cell.
    #[inline]
    pub fn cells(&self) -> &[T] {
        &self.cells
    }

    /// Mutable row-major view of every cell.
    #[inline]
    pub fn cells_mut(&mut self) -> &mut [T] {
        &mut self.cells
    }

    /// Iterate `(x, y, &value)` in row-major order.
    pub fn iter(&self) -> impl Iterator<Item = (u16, u16, &T)> {
        let side = self.side;
        self.cells
            .iter()
            .enumerate()
            .map(move |(i, v)| ((i % side as usize) as u16, (i / side as usize) as u16, v))
    }

    /// Map into a grid of another type, preserving geometry.
    pub fn map<U>(&self, f: impl FnMut(&T) -> U) -> ModuleGrid<U> {
        ModuleGrid {
            side: self.side,
            cells: self
                .cells
                .iter()
                .map(f)
                .collect::<Vec<U>>()
                .into_boxed_slice(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexing_is_row_major() {
        let mut g = ModuleGrid::new(4u16, 0u8);
        g.set(1, 2, 7);
        assert_eq!(*g.at(1, 2), 7);
        assert_eq!(g.cells()[2 * 4 + 1], 7);
        assert_eq!(g.get(4, 0), None);
    }

    #[test]
    fn iter_reports_coordinates() {
        let g = ModuleGrid::new(3u16, 1u8);
        let coords: Vec<_> = g.iter().map(|(x, y, _)| (x, y)).collect();
        assert_eq!(coords[0], (0, 0));
        assert_eq!(coords[4], (1, 1));
        assert_eq!(coords[8], (2, 2));
    }

    #[test]
    fn payload_and_reference_classification() {
        assert!(ModuleRole::Payload.is_payload());
        assert!(!ModuleRole::Reference.is_payload());
        assert!(ModuleRole::Reference.is_reference());
        assert!(ModuleRole::PaletteAnchor.is_reference());
        assert!(!ModuleRole::Finder.is_reference());
    }
}
