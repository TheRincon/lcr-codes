//! Wall time of one decode, by version — clean, under a large highlight, and defocused.
//!
//! `cargo run -p lcr-decode --release --example decode_time`
//!
//! Native, not wasm: read the ratio between two builds, not the milliseconds.

#![allow(dead_code)]

use lcr_core::Compression;
use lcr_core::version::{Profile, Version};
use lcr_decode::Decoder;
use lcr_encode::EncodeOptions;
use lcr_render::{Raster, RenderOptions, render_rgb};
use std::time::Instant;

include!("../tests/support/ops.rs");

fn time(decoder: &Decoder, r: &Raster) -> f64 {
    let _ = decoder.decode_rgb(&r.data, r.width, r.height, r.width * 3);
    let n = 20;
    let t0 = Instant::now();
    for _ in 0..n {
        let _ = decoder.decode_rgb(&r.data, r.width, r.height, r.width * 3);
    }
    t0.elapsed().as_secs_f64() * 1000.0 / n as f64
}

fn main() {
    for v in [2u8, 4, 8, 16, 32] {
        let opts = EncodeOptions {
            profile: Profile::C1,
            version: Some(Version::new(v).unwrap()),
            reference_border: true,
            compression: Some(Compression::None),
            ..Default::default()
        };
        let sym = lcr_encode::encode_bytes(&[0x5A; 16], &opts).unwrap();
        let r = render_rgb(
            &sym,
            &RenderOptions {
                module_px: 10,
                ..RenderOptions::screen()
            },
        );
        let decoder = Decoder::new();
        let d = decoder
            .decode_rgb(&r.data, r.width, r.height, r.width * 3)
            .unwrap()
            .diagnostics;
        let clean = time(&decoder, &r);
        // The case the local luma read (D22) does the most work in.
        let glared = time(&decoder, &specular(&r, 0.5, 0.5, 0.45, 0.6));
        // The case D24's equalising retries do their work in.
        let b5 = blur(&r, 5);
        let blurred = time(&decoder, &b5);
        let blur_ok = decoder
            .decode_rgb(&b5.data, b5.width, b5.height, b5.width * 3)
            .is_ok();
        println!(
            "v{v:<3} {}×{} px  clean {clean:>7.2} ms (LDPC {}+{} it)  glared {glared:>7.2} ms  \
             blurred {blurred:>7.2} ms ({})",
            r.width,
            r.height,
            d.luma_iterations,
            d.chroma_iterations,
            if blur_ok { "decoded" } else { "failed" }
        );
    }
}
