//! Multi-frame accumulation against defocus.
//!
//! `cargo run -p lc-decode --release --example multiframe [trials] [profile] [frames]`
//!
//! A live preview of an out-of-focus symbol: the blur is the same in every frame, the sensor
//! noise is not. Each trial pushes `frames` such frames into a [`FrameAccumulator`] and
//! reports three rates:
//!
//! * **single** — fraction of individual frames that decode on their own;
//! * **accumulated** — whether the accumulator decodes after all frames.
//!
//! Same symbols as `just profiles`: v4, each profile at its own capacity, ring on, 12
//! px/module.

#![allow(dead_code)]

use lc_core::capacity::capacity_for_symbol;
use lc_core::version::{EccLevel, Profile, Version};
use lc_core::{Compression, Packet};
use lc_decode::image::{ImageView, PixelFormat};
use lc_decode::{Decoder, FrameAccumulator};
use lc_encode::EncodeOptions;
use lc_render::{Raster, RenderOptions, render_rgb};

include!("../tests/support/ops.rs");

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let trials: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(8);
    let profile = match args.get(2).map(|s| s.to_ascii_uppercase()).as_deref() {
        Some("C2") => Profile::C2,
        Some("C3") => Profile::C3,
        _ => Profile::C1,
    };
    let frames: u64 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(12);
    let v = Version::new(4).unwrap();
    let bytes = capacity_for_symbol(v, profile, EccLevel::DEFAULT, true).body_bytes;
    let opts = EncodeOptions {
        profile,
        version: Some(v),
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    let decoder = Decoder::new();
    println!("{profile:?} v4, {trials} trials × {frames} frames — success %");
    println!(
        "  {:>5} {:>6} {:>6} {:>8} {:>12}",
        "blur", "noise", "glare", "single", "accumulated"
    );
    // Glare, when present, wanders between frames as a real reflection does when the phone
    // moves: golden-angle steps around the symbol, as `multi_frame_beats_single_frame`.
    for (radius, sigma, glare) in [
        (7, 0.05, 0.0),
        (8, 0.05, 0.0),
        (9, 0.05, 0.0),
        (10, 0.05, 0.0),
        (8, 0.10, 0.0),
        (9, 0.10, 0.0),
        (6, 0.02, 0.8),
        (7, 0.02, 0.8),
        (8, 0.02, 0.8),
    ] {
        let (mut single, mut acc_ok) = (0usize, 0usize);
        for t in 0..trials {
            let p = trial_payload(bytes, t as u64 * 0x9E37 + 1);
            let sym = lc_encode::encode_bytes(&p, &opts).unwrap();
            let clean = render_rgb(
                &sym,
                &RenderOptions {
                    module_px: 12,
                    ..RenderOptions::screen()
                },
            );
            let blurred = blur(&clean, radius);
            let mut acc = FrameAccumulator::new();
            for k in 0..frames {
                let f = if glare > 0.0 {
                    let a = (k + 1) as f32 * 2.399;
                    specular(
                        &blurred,
                        0.5 + 0.25 * a.cos(),
                        0.5 + 0.25 * a.sin(),
                        0.3,
                        glare,
                    )
                } else {
                    blurred.clone()
                };
                let f = noise(&f, sigma, (t as u64 + 1) * 1000 + k + 1);
                let ok = |o: Result<lc_decode::DecodeOutput, _>| matches!(o.map(|o| o.packet), Ok(Packet::Unsigned { ref body, .. }) if *body == p);
                if ok(decoder.decode_rgb(&f.data, f.width, f.height, f.width * 3)) {
                    single += 1;
                }
                let view =
                    ImageView::new(&f.data, f.width, f.height, f.width * 3, PixelFormat::Rgb8)
                        .unwrap();
                let _ = acc.push(&view);
            }
            if matches!(
                acc.try_decode(&decoder).map(|o| o.packet),
                Ok(Packet::Unsigned { ref body, .. }) if *body == p
            ) {
                acc_ok += 1;
            }
        }
        println!(
            "  {radius:>5} {sigma:>6.2} {glare:>6.1} {:>8.0} {:>12.0}",
            100.0 * single as f32 / (trials as u64 * frames) as f32,
            100.0 * acc_ok as f32 / trials as f32
        );
    }
}
