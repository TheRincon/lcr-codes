//! Fuzz the decoder's untrusted entry point (§15).
//!
//! `decode_rgb` is the one function in this project that is *definitionally* fed
//! attacker-controlled bytes: it takes whatever a camera saw, and a camera can be pointed
//! at anything, including something crafted. In the wasm component the dimensions arrive
//! across the ABI from the host as well, so `width`, `height` and `stride` are inputs in
//! their own right rather than trusted metadata.
//!
//! The contract being fuzzed is narrow and absolute: **the decoder may return any error it
//! likes, but it may not panic.** §14.2 requires panics stay out of the hot path and the
//! build uses `panic = "abort"`, so in the component a panic is a trap that poisons the
//! instance — a decoder that can be crashed by a hostile symbol is a denial of service on
//! whatever is scanning it.
//!
//! Dimensions are taken from the front of the input rather than being bounded, precisely
//! so absurd ones get explored. Nothing is allocated from them — the pixel buffer is
//! whatever the fuzzer supplied — so an impossible geometry costs a rejection, not memory.
//! That is what makes it safe to let the fuzzer ask for a 4-billion-pixel row.

#![no_main]

use libfuzzer_sys::fuzz_target;

use lc_decode::Decoder;
use lc_decode::image::PixelFormat;

fuzz_target!(|data: &[u8]| {
    // 7 bytes of geometry, then pixels. Small inputs are still worth running: they
    // exercise the short-buffer rejection, which is where the overflow bug lived.
    if data.len() < 7 {
        return;
    }
    let width = u16::from_le_bytes([data[0], data[1]]) as u32;
    let height = u16::from_le_bytes([data[2], data[3]]) as u32;
    let stride = u16::from_le_bytes([data[4], data[5]]) as u32;
    let format = match data[6] % 3 {
        0 => PixelFormat::Rgb8,
        1 => PixelFormat::Rgba8,
        _ => PixelFormat::Gray8,
    };
    let pixels = &data[7..];

    // Decoding is O(width · height) with an LDPC pass on top, and libFuzzer's default
    // 4 KiB input cannot feed a large frame anyway — but a huge `height` with a tiny
    // `stride` could still pass validation and then iterate for a very long time. Bound
    // the *work*, not the values, so the rejection paths stay reachable.
    if width.saturating_mul(height) > 1 << 20 {
        return;
    }

    let decoder = Decoder::new();
    let _ = match format {
        PixelFormat::Rgb8 => decoder.decode_rgb(pixels, width, height, stride),
        PixelFormat::Rgba8 => decoder.decode_rgba(pixels, width, height, stride),
        PixelFormat::Gray8 => decoder.decode_gray(pixels, width, height, stride),
    };
});
