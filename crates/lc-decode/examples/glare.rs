//! Where does specular glare break a decode?
//!
//! `cargo run -p lc-decode --release --example glare [trials] [profile]`
//!
//! `just envelope` and `just profiles` report *whether* a glared symbol decodes. This
//! reports *which stage gave up*, because the remedies are different:
//!
//! * **lost** — nothing located, or the format did not read. Detection's problem.
//! * **luma** — located, and the luma layer (or the CRC over both) failed.
//! * **chroma** — luma converged and chroma did not: the layer a hue-angle likelihood or
//!   clipping erasures could rescue.
//!
//! Same symbols as `just profiles` — v4, each profile at its own capacity, ring on, 12
//! px/module — and the same seeded glare placement, so the rows are comparable with it.

#![allow(dead_code)]

use lc_core::capacity::capacity_for_symbol;
use lc_core::version::{EccLevel, Profile, Version};
use lc_core::{Compression, Packet};
use lc_decode::{DecodeOptions, Decoder};
use lc_encode::EncodeOptions;
use lc_render::{Raster, RenderOptions, render_rgb};

include!("../tests/support/ops.rs");

const VERSION: u8 = 4;
const DEFAULT_TRIALS: usize = 32;

type TrialSet = Vec<(Vec<u8>, Raster)>;

fn bases(profile: Profile, trials: usize) -> TrialSet {
    let v = Version::new(VERSION).unwrap();
    let bytes = capacity_for_symbol(v, profile, EccLevel::DEFAULT, true).body_bytes;
    let opts = EncodeOptions {
        profile,
        version: Some(v),
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    (0..trials)
        .map(|t| {
            let p = trial_payload(bytes, t as u64 * 0x9E37 + 1);
            let sym = lc_encode::encode_bytes(&p, &opts).unwrap();
            let r = render_rgb(
                &sym,
                &RenderOptions {
                    module_px: 12,
                    ..RenderOptions::screen()
                },
            );
            (p, r)
        })
        .collect()
}

#[derive(Default)]
struct Tally {
    ok: usize,
    chroma: usize,
    luma: usize,
    lost: usize,
}

fn classify(r: &Raster, expect: &[u8], tally: &mut Tally) {
    let decoder = Decoder::new().with_options(DecodeOptions {
        allow_partial: true,
        ..Default::default()
    });
    match decoder.decode_rgb(&r.data, r.width, r.height, r.width * 3) {
        Ok(out) => match out.packet {
            Packet::Unsigned { ref body, .. } if body == expect => tally.ok += 1,
            Packet::Partial { .. } => tally.chroma += 1,
            // Decoded to the wrong bytes: the CRC let a mistake through. Count it loudly
            // as a luma failure so it cannot hide inside a success rate.
            _ => tally.luma += 1,
        },
        Err(_) => {
            if decoder
                .probe(
                    &lc_decode::image::ImageView::packed(
                        &r.data,
                        r.width,
                        r.height,
                        lc_decode::image::PixelFormat::Rgb8,
                    )
                    .unwrap(),
                )
                .is_ok()
            {
                tally.luma += 1;
            } else {
                tally.lost += 1;
            }
        }
    }
}

fn sweep(name: &str, set: &TrialSet, mags: &[f32], imp: Impair) {
    println!("  {name}");
    println!(
        "    {:>6} {:>6} {:>7} {:>6} {:>6}",
        "mag", "ok%", "chroma", "luma", "lost"
    );
    for &m in mags {
        let mut t = Tally::default();
        for (i, (p, r)) in set.iter().enumerate() {
            let seed = (i as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D);
            classify(&apply(imp, r, m, seed), p, &mut t);
        }
        println!(
            "    {m:>6.2} {:>6.0} {:>7} {:>6} {:>6}",
            100.0 * t.ok as f32 / set.len() as f32,
            t.chroma,
            t.luma,
            t.lost
        );
    }
}

fn main() {
    let trials = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(DEFAULT_TRIALS);
    let only = std::env::args().nth(2);
    for profile in [Profile::C1, Profile::C2, Profile::C3] {
        if only
            .as_deref()
            .is_some_and(|o| !format!("{profile:?}").eq_ignore_ascii_case(o))
        {
            continue;
        }
        let set = bases(profile, trials);
        println!("{profile:?}, v{VERSION}, {trials} trials/point");
        sweep(
            "specular strength (radius 0.30)",
            &set,
            &[0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.2],
            Impair::SpecularStrength,
        );
        sweep(
            "specular radius (strength 0.6)",
            &set,
            &[0.15, 0.20, 0.25, 0.30, 0.35, 0.40, 0.50],
            Impair::SpecularRadius,
        );
        println!();
    }
}
