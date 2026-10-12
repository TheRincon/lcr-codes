//! How often the format word reads correctly, by noise level, ring and finder paths.
//!
//! `cargo run -p lcr-decode --release --example format_probe [trials]`
#![allow(dead_code)]
use lcr_core::Compression;
use lcr_core::capacity::capacity_for_symbol;
use lcr_core::version::{EccLevel, Profile, Version};
use lcr_decode::Decoder;
use lcr_decode::image::{ImageView, PixelFormat};
use lcr_encode::EncodeOptions;
use lcr_render::{Raster, RenderOptions, render_rgb};
include!("../tests/support/ops.rs");
fn main() {
    let trials: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(32);
    for profile in [Profile::C1, Profile::C3, Profile::M] {
        let v = Version::new(4).unwrap();
        let bytes = capacity_for_symbol(v, profile, EccLevel::DEFAULT, true).body_bytes / 2;
        let opts = EncodeOptions {
            profile,
            version: Some(v),
            reference_border: true,
            compression: Some(Compression::None),
            ..Default::default()
        };
        let mut row = String::new();
        for sigma in [0.25f32, 0.30, 0.35, 0.40, 0.45, 0.50] {
            let mut ok = 0;
            for t in 0..trials {
                let p = trial_payload(bytes, t as u64 * 0x9E37 + 1);
                let sym = lcr_encode::encode_bytes(&p, &opts).unwrap();
                let r = render_rgb(
                    &sym,
                    &RenderOptions {
                        module_px: 12,
                        ..RenderOptions::screen()
                    },
                );
                let n = noise(&r, sigma, t as u64 + 77);
                let img = ImageView::packed(&n.data, n.width, n.height, PixelFormat::Rgb8).unwrap();
                if Decoder::new()
                    .probe(&img)
                    .is_ok_and(|pr| pr.format == sym.format)
                {
                    ok += 1;
                }
            }
            row.push_str(&format!(
                "  σ{sigma:.2}:{:>4.0}",
                100.0 * ok as f32 / trials as f32
            ));
        }
        println!("{profile:?} format read correctly %:{row}");
    }
}
