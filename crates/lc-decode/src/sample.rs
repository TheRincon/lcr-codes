//! Module and chroma-cell sampling kernels (§12.6).
//!
//! > * Luma: central ~50% of module footprint.
//! > * Chroma: central ~70% of the **chroma cell** footprint — deliberately wider, since
//! >   chroma is spatially low-passed by the capture pipeline anyway and a wider aperture
//! >   buys SNR at no real resolution cost. — §12.6
//!
//! The asymmetry is the whole point. Making both kernels the same width would either
//! throw away chroma SNR that costs nothing to collect, or let luma pick up bleed from
//! its neighbours — and luma is the layer that must survive alone (§8.1).
//!
//! Sampling is a small grid of bilinear taps rather than a full footprint integral. At
//! the ≥8 device-pixels-per-module of §13.1, a 3×3 grid over the central half of a module
//! already averages a substantial part of it, and the cost difference over a v32 symbol
//! is 20k taps against 1.3M.

use crate::image::ImageView;
use crate::transform::Transform;
use lc_core::color::Linear;

/// Half-width of the luma kernel, in module units. Central 50%.
pub const LUMA_HALF: f32 = 0.25;
/// Half-width of the chroma kernel, as a fraction of the chroma cell. Central 70%.
pub const CHROMA_FRACTION: f32 = 0.35;
/// Taps per axis.
const TAPS: u32 = 3;

/// Average linear-light colour over a square region of module space.
///
/// `(cx, cy)` is the region centre in module coordinates and `half` its half-extent,
/// both in module units. Coordinates outside the symbol are fine — the border of §6.4
/// lives there and the homography extrapolates.
pub fn sample_region(img: &ImageView, t: &Transform, cx: f32, cy: f32, half: f32) -> Linear {
    let mut acc = Linear::new(0.0, 0.0, 0.0);
    let denom = (TAPS - 1).max(1) as f32;
    for iy in 0..TAPS {
        for ix in 0..TAPS {
            let u = cx + half * (2.0 * ix as f32 / denom - 1.0);
            let v = cy + half * (2.0 * iy as f32 / denom - 1.0);
            let (px, py) = t.apply(u, v);
            let s = img.linear_bilinear(px, py);
            acc.r += s.r;
            acc.g += s.g;
            acc.b += s.b;
        }
    }
    let n = (TAPS * TAPS) as f32;
    Linear::new(acc.r / n, acc.g / n, acc.b / n)
}

/// Sample one module with the luma kernel.
#[inline]
pub fn sample_module(img: &ImageView, t: &Transform, x: i32, y: i32) -> Linear {
    sample_region(img, t, x as f32 + 0.5, y as f32 + 0.5, LUMA_HALF)
}

/// Sample one chroma cell with the wider chroma kernel.
///
/// `cell` is the cell coordinate and `size` the cell side in modules (1 or 2).
#[inline]
pub fn sample_chroma_cell(img: &ImageView, t: &Transform, cx: u16, cy: u16, size: u16) -> Linear {
    let s = size as f32;
    sample_region(
        img,
        t,
        cx as f32 * s + s / 2.0,
        cy as f32 * s + s / 2.0,
        CHROMA_FRACTION * s,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::PixelFormat;
    use alloc::vec;
    use alloc::vec::Vec;

    /// A 2×2-module image at 10 px/module: top-left black, the rest white.
    fn quad_image() -> (Vec<u8>, Transform) {
        let (w, h) = (20u32, 20u32);
        let mut data = vec![255u8; (w * h * 3) as usize];
        for y in 0..10 {
            for x in 0..10 {
                let i = ((y * w + x) * 3) as usize;
                data[i..i + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let t = Transform::from_quad(
            [(0.0, 0.0), (2.0, 0.0), (0.0, 2.0), (2.0, 2.0)],
            [(0.0, 0.0), (20.0, 0.0), (0.0, 20.0), (20.0, 20.0)],
        )
        .unwrap();
        (data, t)
    }

    #[test]
    fn luma_kernel_stays_inside_its_module() {
        let (data, t) = quad_image();
        let img = ImageView::packed(&data, 20, 20, PixelFormat::Rgb8).unwrap();
        // The dark module must read as fully dark despite touching three light ones.
        assert!(sample_module(&img, &t, 0, 0).luminance() < 0.01);
        assert!(sample_module(&img, &t, 1, 0).luminance() > 0.99);
        assert!(sample_module(&img, &t, 1, 1).luminance() > 0.99);
    }

    #[test]
    fn chroma_kernel_is_wider_than_the_luma_kernel() {
        assert!(CHROMA_FRACTION > LUMA_HALF);
        // For a 2×2 cell the chroma aperture is 1.4 modules across versus 0.5 for luma:
        // ~8× the area, which is where the SNR comes from.
        let luma_area = (2.0 * LUMA_HALF).powi(2);
        let chroma_area = (2.0 * CHROMA_FRACTION * 2.0).powi(2);
        assert!(chroma_area / luma_area > 7.0);
    }

    #[test]
    fn chroma_cell_sampling_covers_the_whole_cell() {
        let (data, t) = quad_image();
        let img = ImageView::packed(&data, 20, 20, PixelFormat::Rgb8).unwrap();
        // One 2×2 cell over the whole image: three light modules and one dark.
        let c = sample_chroma_cell(&img, &t, 0, 0, 2);
        assert!(c.luminance() > 0.3 && c.luminance() < 0.95, "{:?}", c.luminance());
    }

    #[test]
    fn sampling_outside_the_symbol_does_not_panic() {
        let (data, t) = quad_image();
        let img = ImageView::packed(&data, 20, 20, PixelFormat::Rgb8).unwrap();
        let _ = sample_module(&img, &t, -4, -4);
        let _ = sample_module(&img, &t, 100, 100);
    }
}
