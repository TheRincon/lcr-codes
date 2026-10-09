//! Input adaptation and binarisation (§12.1).
//!
//! ## Zero-copy input
//!
//! [`ImageView`] borrows the caller's pixel buffer and understands stride and channel
//! count, so a `Uint8ClampedArray` straight out of `canvas.getImageData` (§14.2) or a
//! camera plane with row padding can be decoded without a copy or a repack.
//!
//! ## Two views of the same pixels, for two different jobs
//!
//! Detection wants a *binary* image: the 1:1:3:1:1 run scan is a run-length problem and
//! wants hard black and white. Demodulation wants *linear light*: glare and shading are
//! multiplicative there and become an awkward non-linearity after gamma encoding
//! (§12.3). Both views are provided and neither is derived from the other — binarising
//! and then trying to recover colour would throw away exactly the information §12.4
//! needs.

use crate::error::DecodeError;
use alloc::vec;
use alloc::vec::Vec;
use lc_core::color::{Linear, Srgb8, srgb_to_linear_f};

/// Pixel layout of a caller-supplied buffer.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum PixelFormat {
    /// Three bytes per pixel.
    Rgb8,
    /// Four bytes per pixel; the fourth is ignored.
    Rgba8,
    /// One byte per pixel. Chroma decoding is unavailable, so only the luma layer of
    /// §8.1 is recoverable — which §1.1 goal 3 requires to still work.
    Gray8,
}

impl PixelFormat {
    /// Bytes per pixel.
    #[inline]
    pub const fn channels(self) -> usize {
        match self {
            PixelFormat::Rgb8 => 3,
            PixelFormat::Rgba8 => 4,
            PixelFormat::Gray8 => 1,
        }
    }

    /// Whether this format can carry the chroma layer at all.
    #[inline]
    pub const fn has_chroma(self) -> bool {
        !matches!(self, PixelFormat::Gray8)
    }
}

/// A borrowed view of caller pixels.
#[derive(Copy, Clone, Debug)]
pub struct ImageView<'a> {
    data: &'a [u8],
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes per row, which may exceed `width · channels`.
    pub stride: u32,
    /// Pixel layout.
    pub format: PixelFormat,
}

impl<'a> ImageView<'a> {
    /// Wrap a buffer, validating that it is large enough.
    pub fn new(
        data: &'a [u8],
        width: u32,
        height: u32,
        stride: u32,
        format: PixelFormat,
    ) -> Result<ImageView<'a>, DecodeError> {
        if width == 0 || height == 0 {
            return Err(DecodeError::ShortBuffer);
        }
        // Checked, because every operand here is caller-supplied and the products are big
        // enough to wrap. `width · channels` alone exceeds `u32` past ~1.07e9, and **the
        // scanner is wasm32**, where `usize` is 32 bits — so on the target that matters
        // most this overflows at merely large values, not absurd ones.
        //
        // Unchecked, the two builds failed differently and both were wrong: debug panicked
        // with "attempt to add with overflow", which in a wasm component is a trap that
        // poisons the instance and which §14.2 forbids in the hot path; release wrapped to
        // a small `need` and returned the right answer only because `stride < min_stride`
        // short-circuits ahead of it. Reordering that condition would have turned a
        // silently wrapped bound into an out-of-range view.
        let Some(min_stride) = (width as usize).checked_mul(format.channels()) else {
            return Err(DecodeError::ShortBuffer);
        };
        let need = (height as usize - 1)
            .checked_mul(stride as usize)
            .and_then(|rows| rows.checked_add(min_stride));
        let Some(need) = need else {
            return Err(DecodeError::ShortBuffer);
        };
        if (stride as usize) < min_stride || data.len() < need {
            return Err(DecodeError::ShortBuffer);
        }
        Ok(ImageView {
            data,
            width,
            height,
            stride,
            format,
        })
    }

    /// Wrap a tightly-packed buffer.
    pub fn packed(
        data: &'a [u8],
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<ImageView<'a>, DecodeError> {
        // Checked for the same reason as `new`: this product is `u32`, so it wraps past a
        // width of ~1.07e9 on *every* target rather than only on wasm32.
        let Some(stride) = width.checked_mul(format.channels() as u32) else {
            return Err(DecodeError::ShortBuffer);
        };
        ImageView::new(data, width, height, stride, format)
    }

    #[inline]
    fn offset(&self, x: u32, y: u32) -> usize {
        y as usize * self.stride as usize + x as usize * self.format.channels()
    }

    /// Read a pixel as 8-bit sRGB. Out-of-range coordinates clamp to the edge.
    #[inline]
    pub fn rgb(&self, x: u32, y: u32) -> Srgb8 {
        let x = x.min(self.width - 1);
        let y = y.min(self.height - 1);
        let i = self.offset(x, y);
        match self.format {
            PixelFormat::Gray8 => {
                let v = self.data[i];
                Srgb8::new(v, v, v)
            }
            _ => Srgb8::new(self.data[i], self.data[i + 1], self.data[i + 2]),
        }
    }

    /// Bilinearly sample linear-light RGB at fractional coordinates.
    pub fn linear_bilinear(&self, x: f32, y: f32) -> Linear {
        // `max` then `min`, not `clamp`: `f32::clamp` asserts `min <= max`, and with a bound
        // only known at run time that assertion — and the Debug formatting of two floats in
        // its panic message — stays in the binary. That one assertion kept ~16 KB of float
        // printing alive in the scanner. `ImageView::new` already rejects an empty image, so
        // the bounds are always ordered here.
        let x = x.max(0.0).min(self.width as f32 - 1.0);
        let y = y.max(0.0).min(self.height as f32 - 1.0);
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));

        let lin = |px: Srgb8| {
            Linear::new(
                srgb_to_linear_f(px.r as f32 / 255.0),
                srgb_to_linear_f(px.g as f32 / 255.0),
                srgb_to_linear_f(px.b as f32 / 255.0),
            )
        };
        let (a, b, c, d) = (
            lin(self.rgb(x0, y0)),
            lin(self.rgb(x1, y0)),
            lin(self.rgb(x0, y1)),
            lin(self.rgb(x1, y1)),
        );
        let mix = |p: f32, q: f32, r: f32, s: f32| {
            (p * (1.0 - fx) + q * fx) * (1.0 - fy) + (r * (1.0 - fx) + s * fx) * fy
        };
        Linear::new(
            mix(a.r, b.r, c.r, d.r),
            mix(a.g, b.g, c.g, d.g),
            mix(a.b, b.b, c.b, d.b),
        )
    }

    /// Gamma-encoded luma, `0..=255`. Used only for detection.
    pub fn gray_buffer(&self) -> Vec<u8> {
        let mut out = vec![0u8; (self.width * self.height) as usize];
        for y in 0..self.height {
            for x in 0..self.width {
                let p = self.rgb(x, y);
                // Integer Rec.709, rounded. Detection does not need more than this.
                let v = (54 * p.r as u32 + 183 * p.g as u32 + 19 * p.b as u32) >> 8;
                out[(y * self.width + x) as usize] = v as u8;
            }
        }
        out
    }
}

/// Side of a binarisation block, in pixels.
const BLOCK: u32 = 8;
/// Below this within-block contrast, the block is assumed uniform and inherits its
/// neighbours' level rather than inventing an edge out of sensor noise.
const MIN_DYNAMIC_RANGE: u32 = 24;

/// A one-bit-per-pixel image. `true` is dark.
#[derive(Clone, Debug)]
pub struct Binary {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    bits: Vec<u8>,
}

impl Binary {
    /// Is the pixel dark? Out-of-range reads are light, matching a quiet zone.
    #[inline]
    pub fn dark(&self, x: u32, y: u32) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let i = (y * self.width + x) as usize;
        self.bits[i / 8] & (0x80 >> (i % 8)) != 0
    }

    #[inline]
    fn set(&mut self, x: u32, y: u32, dark: bool) {
        let i = (y * self.width + x) as usize;
        let mask = 0x80u8 >> (i % 8);
        if dark {
            self.bits[i / 8] |= mask;
        } else {
            self.bits[i / 8] &= !mask;
        }
    }

    /// 3×3 majority filter, removing isolated flipped pixels.
    ///
    /// Detection is entirely run-length based (§12.1), and a run-length scan is
    /// pathologically fragile to salt-and-pepper: **one** flipped pixel in the middle of a
    /// dark run splits it in two and the 1:1:3:1:1 test fails, even though the run is 12
    /// pixels wide and visually perfect. Sensor noise produces exactly that, so without
    /// this filter the detector loses symbols at noise levels the LDPC decoder would not
    /// even notice.
    ///
    /// A majority vote cannot move a real edge — at the ≥ 8 device pixels per module
    /// §13.1 requires, every genuine run is far wider than the 3×3 window — so this costs
    /// nothing but the pass.
    pub fn denoised(&self) -> Binary {
        let mut out = self.clone();
        for y in 1..self.height.saturating_sub(1) {
            for x in 1..self.width.saturating_sub(1) {
                let mut votes = 0u32;
                for dy in 0..3u32 {
                    for dx in 0..3u32 {
                        votes += self.dark(x + dx - 1, y + dy - 1) as u32;
                    }
                }
                out.set(x, y, votes >= 5);
            }
        }
        out
    }
}

/// Adaptive local binarisation.
///
/// A global threshold is useless here: §3 lists glare, shading and auto-brightness as the
/// dominant screen impairments, and all three move the black point across the frame. The
/// block scheme takes each 8×8 block's own midpoint, smoothed over its 5×5 block
/// neighbourhood so a block that happens to be entirely inside one dark module does not
/// threshold its own noise into a checkerboard.
///
/// ## Uniform blocks carry no estimate, rather than borrowing one
///
/// A block flatter than [`MIN_DYNAMIC_RANGE`] has no edge in it, so its own mean says
/// nothing about where the black point is — it is just the level of whichever module it
/// landed inside. Such a block is marked *invalid* and contributes nothing to the
/// smoothing pass; its threshold comes entirely from whichever neighbours within the 5×5
/// window did see an edge.
///
/// It must not instead inherit a value from an already-computed neighbour. That is a
/// recurrence — pass one both reads and writes `block_avg` — and any such rule ratchets
/// monotonically: seeded with `min(neighbour, own_min)` a single dark block drags every
/// block below-right of it downward, and the threshold collapses. At a module pitch equal
/// to `BLOCK` almost every block is uniform, so the ratchet ran unopposed across the whole
/// image: a 47.7% dark v32 symbol binarised to 3.2% dark and detection found nothing.
/// That was the §13.1 ≥ 8 px/module conformance failure (decisions D6) — not the block
/// alignment itself, which is merely what exposed the recurrence.
pub fn binarize(gray: &[u8], width: u32, height: u32) -> Binary {
    let bw = width.div_ceil(BLOCK);
    let bh = height.div_ceil(BLOCK);
    let mut block_avg = vec![0u32; (bw * bh) as usize];
    let mut block_min = vec![0u32; (bw * bh) as usize];
    let mut block_max = vec![0u32; (bw * bh) as usize];
    let mut block_ok = vec![false; (bw * bh) as usize];

    for by in 0..bh {
        for bx in 0..bw {
            let (x0, y0) = (bx * BLOCK, by * BLOCK);
            let (x1, y1) = ((x0 + BLOCK).min(width), (y0 + BLOCK).min(height));
            let (mut sum, mut min, mut max, mut n) = (0u32, 255u32, 0u32, 0u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    let v = gray[(y * width + x) as usize] as u32;
                    sum += v;
                    min = min.min(v);
                    max = max.max(v);
                    n += 1;
                }
            }
            let i = (by * bw + bx) as usize;
            block_avg[i] = sum / n.max(1);
            block_min[i] = min;
            block_max[i] = max;
        }
    }

    // Two levels of evidence, in order of preference.
    //
    // `saw_edge`: the block contains an edge itself, so its own mean is a local black
    // point. This is the primary estimate and the only one used wherever it is available.
    //
    // `near_edge`: the block contains no edge but sits within one block of one, so its own
    // mean is a clean sample of a single level — useful in company, useless alone. Whether
    // a block straddles a module boundary is an accident of alignment, and at a module
    // pitch equal to `BLOCK` it is an accident that never happens: the two grids coincide,
    // every block lands wholly inside one module, and not one block in the whole image
    // sees an edge. That is the §13.1 ≥ 8 px/module conformance failure (decisions D6),
    // and this second level is what covers it. Three blocks across is at least three
    // modules at any conforming pitch, so a real symbol always clears it and blank
    // background never does.
    let mut near_edge = vec![false; (bw * bh) as usize];
    for by in 0..bh {
        for bx in 0..bw {
            let i = (by * bw + bx) as usize;
            block_ok[i] = block_max[i] - block_min[i] > MIN_DYNAMIC_RANGE;

            let (mut lo, mut hi) = (255u32, 0u32);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (bx as i32 + dx, by as i32 + dy);
                    if nx >= 0 && ny >= 0 && (nx as u32) < bw && (ny as u32) < bh {
                        let j = (ny as u32 * bw + nx as u32) as usize;
                        lo = lo.min(block_min[j]);
                        hi = hi.max(block_max[j]);
                    }
                }
            }
            near_edge[i] = hi - lo > MIN_DYNAMIC_RANGE;
        }
    }

    // Local threshold: split the block means in a window into two levels and take the
    // midpoint between the level means. One step of Ridler–Calvard.
    //
    // `mask` selects which blocks count, and the caller tries the two evidence levels in
    // order: blocks that saw an edge themselves first, then blocks merely near one.
    //
    // A plain mean cannot be used here the way it is above. Where blocks straddle edges
    // their means already sit near the midpoint, so averaging them lands on the black
    // point regardless of the mix. These blocks are each wholly inside one module, so
    // their means are the *level* values, and a plain mean of them tracks the dark/light
    // balance instead — a window holding more dark modules than light would drag its own
    // threshold down and start calling light modules dark. Splitting first and averaging
    // within each level is indifferent to the mix.
    //
    // Block means rather than raw pixel extremes, for two reasons: a raw midpoint chases
    // outliers, and on a glossy screen the outlier is the impairment (one saturated block
    // in a specular highlight sets the maximum for its whole neighbourhood, §3); and each
    // block mean is already averaged over `BLOCK²` = 64 pixels, so salt-and-pepper cannot
    // move one appreciably before `denoised` ever runs.
    let level_split = |bx: i32, by: i32, radius: i32, mask: &[bool]| -> Option<u32> {
        let visit = |f: &mut dyn FnMut(u32)| {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let (nx, ny) = (bx + dx, by + dy);
                    if nx >= 0 && ny >= 0 && (nx as u32) < bw && (ny as u32) < bh {
                        let i = (ny as u32 * bw + nx as u32) as usize;
                        if mask[i] {
                            f(block_avg[i]);
                        }
                    }
                }
            }
        };
        let (mut sum, mut n) = (0u32, 0u32);
        visit(&mut |v| {
            sum += v;
            n += 1;
        });
        if n == 0 {
            return None;
        }
        let mean = sum / n;

        let (mut dsum, mut dn, mut lsum, mut ln) = (0u32, 0u32, 0u32, 0u32);
        visit(&mut |v| {
            if v <= mean {
                dsum += v;
                dn += 1;
            } else {
                lsum += v;
                ln += 1;
            }
        });
        // A window entirely on one side of its own mean has no two levels to split; its
        // mean is the best available estimate.
        if dn == 0 || ln == 0 {
            return Some(mean);
        }
        Some((dsum / dn + lsum / ln) / 2)
    };

    // Global fallback for regions where nothing within reach saw an edge.
    let (cx, cy, r) = (bw as i32 / 2, bh as i32 / 2, bw.max(bh) as i32);
    let global = level_split(cx, cy, r, &block_ok).or_else(|| level_split(cx, cy, r, &near_edge));

    let mut bits = vec![0u8; ((width * height) as usize).div_ceil(8)];
    for by in 0..bh {
        for bx in 0..bw {
            // Prefer the estimate from blocks that actually saw an edge, at the original
            // 5×5 window, widening once before giving up on it. Only when nothing in
            // reach saw one — the degenerate-alignment case — fall through to splitting
            // the near-edge block means into levels.
            let threshold = [2i32, 4]
                .into_iter()
                .find_map(|r| level_split(bx as i32, by as i32, r, &block_ok))
                .or_else(|| {
                    [2i32, 4]
                        .into_iter()
                        .find_map(|r| level_split(bx as i32, by as i32, r, &near_edge))
                })
                // Nothing in reach saw an edge. Use the image-wide midpoint if there is
                // one; if the whole image is featureless there is no edge to find
                // anywhere, and calling it all light is the answer that invents nothing.
                .or(global)
                .unwrap_or(0);
            let (x0, y0) = (bx * BLOCK, by * BLOCK);
            let (x1, y1) = ((x0 + BLOCK).min(width), (y0 + BLOCK).min(height));
            for y in y0..y1 {
                for x in x0..x1 {
                    if (gray[(y * width + x) as usize] as u32) < threshold {
                        let i = (y * width + x) as usize;
                        bits[i / 8] |= 0x80 >> (i % 8);
                    }
                }
            }
        }
    }
    Binary {
        width,
        height,
        bits,
    }
}

/// Box-filter downscale of a greyscale buffer by an integer factor.
///
/// Averaging rather than point sampling, deliberately. The §15 harness was once wrong for
/// exactly this reason — its `perspective` op took a single bilinear tap per output pixel
/// and measured its own resampler instead of the tilt it was sweeping. A point-sampled
/// reduction here would alias the module grid against the sampling grid and produce the
/// same class of artefact, at the one stage whose whole job is to find that grid.
pub fn downscale(gray: &[u8], width: u32, height: u32, factor: u32) -> (Vec<u8>, u32, u32) {
    debug_assert!(factor >= 1);
    if factor <= 1 {
        return (gray.to_vec(), width, height);
    }
    let (w, h) = (width / factor, height / factor);
    let mut out = vec![0u8; (w * h) as usize];
    let n = factor * factor;
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0u32;
            for dy in 0..factor {
                let row = ((y * factor + dy) * width) as usize;
                for dx in 0..factor {
                    sum += gray[row + (x * factor + dx) as usize] as u32;
                }
            }
            out[(y * w + x) as usize] = (sum / n) as u8;
        }
    }
    (out, w, h)
}

#[cfg(test)]
mod tests {
    /// Adversarial dimensions must be rejected, not overflow.
    ///
    /// `width`, `height` and `stride` all come from the caller — in the wasm component
    /// they arrive across the ABI from whatever the host chose — so every product here is
    /// attacker-controlled. Before this was checked, the largest values `decode_rgba`
    /// accepts panicked the debug build outright, and the release build was correct only
    /// because an unrelated short-circuit ran first.
    ///
    /// `usize` is 32 bits on wasm32, which is the build this matters most for, so the
    /// cases below are chosen to wrap there as well as on a 64-bit host.
    #[test]
    fn adversarial_dimensions_are_rejected_not_overflowed() {
        let data = [0u8; 64];
        let cases: [(u32, u32, u32, PixelFormat); 6] = [
            (u32::MAX, u32::MAX, u32::MAX, PixelFormat::Rgba8),
            (u32::MAX, u32::MAX, u32::MAX, PixelFormat::Rgb8),
            (u32::MAX, 1, u32::MAX, PixelFormat::Gray8),
            // width * 4 exceeds u32 past ~1.07e9: wraps on wasm32, not on a 64-bit host.
            (1_500_000_000, 2, 8, PixelFormat::Rgba8),
            (1, u32::MAX, u32::MAX, PixelFormat::Rgb8),
            (u32::MAX, u32::MAX, 1, PixelFormat::Rgba8),
        ];
        for (w, h, stride, format) in cases {
            assert!(
                ImageView::new(&data, w, h, stride, format).is_err(),
                "accepted an impossible view: {w}x{h} stride {stride} {format:?}"
            );
        }
    }

    /// `packed` computes the stride itself, in `u32`, so it wraps a decade earlier than
    /// `new` does — past a width of ~1.07e9 on every target, not just wasm32.
    #[test]
    fn packed_rejects_a_width_whose_stride_would_wrap() {
        let data = [0u8; 64];
        for format in [PixelFormat::Rgb8, PixelFormat::Rgba8] {
            assert!(ImageView::packed(&data, u32::MAX, 1, format).is_err());
            assert!(ImageView::packed(&data, 2_000_000_000, 1, format).is_err());
        }
    }

    /// The rejection must not be so eager that legitimate padded planes stop working —
    /// a camera plane with row padding is the normal case, not an edge one.
    #[test]
    fn a_padded_plane_is_still_accepted() {
        let data = [0u8; 4 * 16];
        let view = ImageView::new(&data, 3, 4, 16, PixelFormat::Rgba8)
            .expect("stride 16 > 3*4 = 12 is a legitimate padded row");
        assert_eq!((view.width, view.height, view.stride), (3, 4, 16));
    }

    use super::*;

    fn checker(w: u32, h: u32, cell: u32) -> Vec<u8> {
        let mut v = vec![0u8; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                v[(y * w + x) as usize] = if (x / cell + y / cell).is_multiple_of(2) {
                    20
                } else {
                    230
                };
            }
        }
        v
    }

    #[test]
    fn rejects_buffers_that_are_too_small() {
        let data = vec![0u8; 10];
        assert!(ImageView::packed(&data, 100, 100, PixelFormat::Rgb8).is_err());
        assert!(ImageView::packed(&data, 0, 10, PixelFormat::Rgb8).is_err());
        assert!(ImageView::new(&data, 4, 1, 2, PixelFormat::Rgb8).is_err());
    }

    #[test]
    fn accepts_padded_stride() {
        let data = vec![128u8; 4 * 16];
        let v = ImageView::new(&data, 4, 4, 16, PixelFormat::Rgb8).unwrap();
        assert_eq!(v.rgb(0, 0), Srgb8::new(128, 128, 128));
    }

    #[test]
    fn rgba_ignores_alpha() {
        let data = vec![10u8, 20, 30, 0, 10, 20, 30, 255];
        let v = ImageView::packed(&data, 2, 1, PixelFormat::Rgba8).unwrap();
        assert_eq!(v.rgb(0, 0), v.rgb(1, 0));
        assert_eq!(v.rgb(0, 0), Srgb8::new(10, 20, 30));
    }

    #[test]
    fn gray8_expands_to_neutral_rgb() {
        let data = vec![77u8];
        let v = ImageView::packed(&data, 1, 1, PixelFormat::Gray8).unwrap();
        assert_eq!(v.rgb(0, 0), Srgb8::new(77, 77, 77));
        assert!(!PixelFormat::Gray8.has_chroma());
    }

    #[test]
    fn bilinear_interpolates_between_pixels() {
        let data = vec![0u8, 0, 0, 255, 255, 255];
        let v = ImageView::packed(&data, 2, 1, PixelFormat::Rgb8).unwrap();
        let mid = v.linear_bilinear(0.5, 0.0);
        let white = v.linear_bilinear(1.0, 0.0);
        assert!((mid.g - white.g * 0.5).abs() < 1e-5, "{mid:?} vs {white:?}");
    }

    #[test]
    fn out_of_range_sampling_clamps() {
        let data = vec![5u8, 6, 7];
        let v = ImageView::packed(&data, 1, 1, PixelFormat::Rgb8).unwrap();
        assert_eq!(v.rgb(99, 99), Srgb8::new(5, 6, 7));
        let _ = v.linear_bilinear(-10.0, 1e6);
    }

    #[test]
    fn binarizes_a_checkerboard_exactly() {
        let (w, h, cell) = (64u32, 64u32, 8u32);
        let g = checker(w, h, cell);
        let b = binarize(&g, w, h);
        for y in 0..h {
            for x in 0..w {
                let want = (x / cell + y / cell) % 2 == 0;
                assert_eq!(b.dark(x, y), want, "({x},{y})");
            }
        }
    }

    /// The reason for local thresholding: a strong illumination gradient must not turn
    /// half the image one colour.
    #[test]
    fn survives_a_brightness_gradient() {
        let (w, h, cell) = (64u32, 64u32, 8u32);
        let mut g = checker(w, h, cell);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                // Shade from 100% at the left to 25% at the right.
                let scale = 1.0 - 0.75 * (x as f32 / w as f32);
                g[i] = (g[i] as f32 * scale) as u8;
            }
        }
        let b = binarize(&g, w, h);
        let wrong = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| b.dark(x, y) != ((x / cell + y / cell) % 2 == 0))
            .count();
        assert!(wrong < 32, "{wrong} pixels misclassified under a gradient");
    }

    #[test]
    fn a_uniform_image_does_not_hallucinate_structure() {
        let g = vec![200u8; 64 * 64];
        let b = binarize(&g, 64, 64);
        let dark = (0..64u32)
            .flat_map(|y| (0..64u32).map(move |x| (x, y)))
            .filter(|&(x, y)| b.dark(x, y))
            .count();
        assert_eq!(dark, 0, "{dark} dark pixels in a flat light image");
    }

    #[test]
    fn denoise_removes_isolated_flips_but_keeps_edges() {
        let (w, h, cell) = (64u32, 64u32, 16u32);
        let mut g = checker(w, h, cell);
        // Salt and pepper: flip scattered single pixels.
        for k in 0..400u32 {
            let x = (k * 7919) % w;
            let y = (k * 104729) % h;
            g[(y * w + x) as usize] = if g[(y * w + x) as usize] > 128 {
                10
            } else {
                245
            };
        }
        let noisy = binarize(&g, w, h);
        let clean = noisy.denoised();

        let truth = |x: u32, y: u32| (x / cell + y / cell).is_multiple_of(2);
        let count = |b: &Binary| {
            (1..h - 1)
                .flat_map(|y| (1..w - 1).map(move |x| (x, y)))
                .filter(|&(x, y)| b.dark(x, y) != truth(x, y))
                .count()
        };
        assert!(
            count(&clean) < count(&noisy) / 4,
            "{} -> {}",
            count(&noisy),
            count(&clean)
        );

        // A run through the middle of a cell must be unbroken after denoising.
        let row = cell / 2;
        let run: Vec<bool> = (1..cell - 1).map(|x| clean.dark(x, row)).collect();
        assert!(run.iter().all(|d| *d), "denoised run still broken: {run:?}");
    }

    #[test]
    fn out_of_range_binary_reads_are_light() {
        let b = binarize(&[0u8; 16], 4, 4);
        assert!(!b.dark(100, 100));
    }
}
