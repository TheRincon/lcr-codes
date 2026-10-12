//! A quick A/B bench over the impairments a demodulation change can move.
//!
//! `cargo run -p lcr-decode --release --example ab [trials]`
//!
//! Same symbols and seeded nuisance draws as `just profiles` (v4, each profile at its own
//! capacity, ring on, 12 px/module), but only the cells near each impairment's knee, so two
//! builds can be compared in a minute or two rather than ten.

#![allow(dead_code)]

use lcr_core::capacity::capacity_for_symbol;
use lcr_core::version::{EccLevel, Profile, Version};
use lcr_core::{Compression, Packet};
use lcr_decode::Decoder;
use lcr_encode::EncodeOptions;
use lcr_render::{Raster, RenderOptions, render_rgb};

include!("../tests/support/ops.rs");

fn bases(profile: Profile, trials: usize) -> Vec<(Vec<u8>, Raster)> {
    let v = Version::new(4).unwrap();
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
            let sym = lcr_encode::encode_bytes(&p, &opts).unwrap();
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

fn ok(r: &Raster, expect: &[u8]) -> bool {
    matches!(
        Decoder::new().decode_rgb(&r.data, r.width, r.height, r.width * 3).map(|o| o.packet),
        Ok(Packet::Unsigned { ref body, .. }) if body == expect
    )
}

fn main() {
    let trials = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(32);
    let cells: [(Impair, &[f32]); 6] = [
        (Impair::SpecularStrength, &[0.8, 0.9, 1.0, 1.2]),
        (Impair::SpecularRadius, &[0.35, 0.40, 0.50]),
        (Impair::Occlusion, &[0.25, 0.30, 0.35]),
        (Impair::Blur, &[4.0, 5.0]),
        (Impair::Noise, &[0.14, 0.18, 0.22]),
        (Impair::Shading, &[0.7, 0.8]),
    ];
    for profile in [Profile::C1, Profile::C3] {
        let set = bases(profile, trials);
        println!("{profile:?}, {trials} trials — success %");
        for (imp, mags) in cells {
            let row: String = mags
                .iter()
                .map(|&m| {
                    let hits = set
                        .iter()
                        .enumerate()
                        .filter(|(t, (p, r))| {
                            let seed = (*t as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D);
                            ok(&apply(imp, r, m, seed), p)
                        })
                        .count();
                    format!("  {m:>5}:{:>4.0}", 100.0 * hits as f32 / set.len() as f32)
                })
                .collect();
            println!("  {:<18}{row}", imp.name());
        }
        // A *dark* occluder — a finger in front of a lit screen. The harness's own occluder
        // is mid-grey, which luminance alone already finds suspicious; this one matches the
        // dark level's luminance and has to be caught by its colour.
        let row: String = [0.20f32, 0.25, 0.30]
            .iter()
            .map(|&m| {
                let hits = set
                    .iter()
                    .enumerate()
                    .filter(|(t, (p, r))| {
                        let mut rng =
                            Rng(((*t as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D)) | 1);
                        let (x0, y0) = (rng.range(0.0, 1.0 - m), rng.range(0.0, 1.0 - m));
                        let dark = map_linear(r, |u, v, c| {
                            if u >= x0 && u < x0 + m && v >= y0 && v < y0 + m {
                                [0.06, 0.045, 0.04]
                            } else {
                                c
                            }
                        });
                        ok(&dark, p)
                    })
                    .count();
                format!("  {m:>5}:{:>4.0}", 100.0 * hits as f32 / set.len() as f32)
            })
            .collect();
        println!("  {:<18}{row}", "dark occlusion");
    }
}
