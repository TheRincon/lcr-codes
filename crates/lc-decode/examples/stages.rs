//! Which decode stage gives up, impairment by impairment.
//!
//! `cargo run -p lc-decode --release --example stages [trials] [profile] [version] [filter]`
//!
//! `filter` keeps only the scenarios whose name contains it.
//!
//! The generalisation of `--example glare`. Each failed trial is binned by the first stage
//! that could not do its job, because each bin wants a different fix:
//!
//! * **ring** — no ring located at all (`ring::detect_ring` found nothing).
//! * **format** — a ring was located but the format did not read, so nothing could be
//!   demodulated. Geometry wrong enough to misread a 30-bit BCH word, usually.
//! * **luma** — format read, luma layer did not converge (or the CRC rejected both).
//! * **chroma** — luma converged, chroma did not: the case a decision-directed refit can
//!   rescue, because luma is then known everywhere.
//!
//! Symbols are v4 at each profile's own capacity unless a version is given, ring on, 12
//! px/module — the `just profiles` setup.

#![allow(dead_code)]

use lc_core::capacity::capacity_for_symbol;
use lc_core::version::{EccLevel, Profile, Version};
use lc_core::{Compression, Packet};
use lc_decode::image::{ImageView, PixelFormat};
use lc_decode::{DecodeOptions, Decoder, ring};
use lc_encode::EncodeOptions;
use lc_render::{Raster, RenderOptions, render_rgb};

include!("../tests/support/ops.rs");

const MODULE_PX: u32 = 12;

#[derive(Default)]
struct Tally {
    ok: usize,
    chroma: usize,
    luma: usize,
    format: usize,
    ring: usize,
}

fn classify(r: &Raster, expect: &[u8], t: &mut Tally) {
    let decoder = Decoder::new().with_options(DecodeOptions {
        allow_partial: true,
        ..Default::default()
    });
    match decoder.decode_rgb(&r.data, r.width, r.height, r.width * 3) {
        Ok(out) => match out.packet {
            Packet::Unsigned { ref body, .. } if body == expect => t.ok += 1,
            Packet::Partial { .. } => t.chroma += 1,
            _ => t.luma += 1,
        },
        Err(_) => {
            let img = ImageView::packed(&r.data, r.width, r.height, PixelFormat::Rgb8).unwrap();
            if decoder.probe(&img).is_ok() {
                t.luma += 1;
            } else if ring::detect_ring(&img.gray_buffer(), r.width, r.height, Version::MAX)
                .is_some()
            {
                t.format += 1;
            } else {
                t.ring += 1;
            }
        }
    }
}

type Scenario = (
    &'static str,
    &'static [f32],
    fn(&Raster, f32, &mut Rng, f32) -> Raster,
);

fn scenarios() -> Vec<Scenario> {
    vec![
        (
            "blur radius",
            &[4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0],
            |r, m, _, _| blur(r, m as u32),
        ),
        ("noise sigma", &[0.18, 0.22, 0.26], |r, m, rng, _| {
            noise(r, m, rng.next())
        }),
        ("4:2:0 + blur", &[2.0, 3.0, 4.0], |r, m, _, _| {
            chroma_subsample(&blur(r, m as u32))
        }),
        ("night shift", &[0.4, 0.6, 0.8], |r, m, _, _| {
            color_cast(r, [1.0, 1.0 - 0.3 * m, 1.0 - 0.9 * m])
        }),
        ("shading", &[0.7, 0.8, 0.85], |r, m, rng, _| {
            shading_dir(r, m, rng.range(0.0, 360.0))
        }),
        ("tilt", &[0.7, 0.8, 0.9], |r, m, rng, _| {
            perspective(r, m, rng.range(0.0, 360.0))
        }),
        ("corner occlusion", &[0.06, 0.10, 0.14], |r, m, rng, q| {
            // A square of side `m` (fraction of the canvas) centred on one ring corner.
            let (cx, cy) = corner(rng, q);
            occlude(r, cx - m / 2.0, cy - m / 2.0, m, m)
        }),
        ("corner glare", &[0.8, 1.2, 1.6], |r, m, rng, q| {
            let (cx, cy) = corner(rng, q);
            specular(r, cx, cy, 0.2, m)
        }),
        (
            "lens distortion",
            &[0.01, 0.02, 0.05, 0.10],
            |r, m, _, _| barrel(r, m),
        ),
        ("lens pincushion", &[0.02, 0.05, 0.10], |r, m, _, _| {
            barrel(r, -m)
        }),
        ("lens off-centre", &[0.02, 0.05, 0.10], |r, m, rng, _| {
            // The symbol in one quadrant of a frame twice its size: its edges bow unevenly,
            // and the distortion centre is nowhere near the symbol's.
            let k = rng.next() % 4;
            barrel(&pad(r, (k & 1) as u32, (k >> 1) as u32), m)
        }),
        ("distortion + blur 3", &[0.02, 0.05, 0.10], |r, m, _, _| {
            barrel(&blur(r, 3), m)
        }),
    ]
}

/// `r` placed in quadrant `(qx, qy)` of a white frame twice its size.
fn pad(r: &Raster, qx: u32, qy: u32) -> Raster {
    let (w, h) = (r.width * 2, r.height * 2);
    let mut out = Raster {
        width: w,
        height: h,
        data: vec![255; (w * h * 3) as usize],
    };
    let (ox, oy) = (qx * r.width, qy * r.height);
    for y in 0..r.height {
        let src = (y * r.width * 3) as usize;
        let dst = (((y + oy) * w + ox) * 3) as usize;
        out.data[dst..dst + (r.width * 3) as usize]
            .copy_from_slice(&r.data[src..src + (r.width * 3) as usize]);
    }
    out
}

/// One of the four ring corners, in canvas fractions; `q` is the corner's inset.
fn corner(rng: &mut Rng, q: f32) -> (f32, f32) {
    let k = (rng.next() % 4) as u8;
    let x = if k & 1 == 0 { q } else { 1.0 - q };
    let y = if k & 2 == 0 { q } else { 1.0 - q };
    (x, y)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let trials: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(16);
    let only = args.get(2).cloned();
    let version = Version::new(args.get(3).and_then(|a| a.parse().ok()).unwrap_or(4)).unwrap();
    let filter = args.get(4).cloned().unwrap_or_default();

    for profile in [Profile::C1, Profile::C2, Profile::C3] {
        if only
            .as_deref()
            .is_some_and(|o| !format!("{profile:?}").eq_ignore_ascii_case(o))
        {
            continue;
        }
        let bytes = capacity_for_symbol(version, profile, EccLevel::DEFAULT, true).body_bytes;
        let opts = EncodeOptions {
            profile,
            version: Some(version),
            reference_border: true,
            compression: Some(Compression::None),
            ..Default::default()
        };
        let set: Vec<(Vec<u8>, Raster)> = (0..trials)
            .map(|t| {
                let p = trial_payload(bytes, t as u64 * 0x9E37 + 1);
                let sym = lc_encode::encode_bytes(&p, &opts).unwrap();
                let r = render_rgb(
                    &sym,
                    &RenderOptions {
                        module_px: MODULE_PX,
                        ..RenderOptions::screen()
                    },
                );
                (p, r)
            })
            .collect();
        // Ring outer corner: two modules of quiet zone in from the canvas edge, on a canvas
        // `side + 10` modules across (D18).
        let q = 2.0 / (version.side() as f32 + 10.0);

        println!(
            "{profile:?} v{} ({bytes} B), {trials} trials",
            version.get()
        );
        println!(
            "  {:<22}{:>6} {:>5} {:>7} {:>6} {:>7} {:>6}",
            "", "mag", "ok%", "chroma", "luma", "format", "ring"
        );
        for (name, mags, f) in scenarios()
            .into_iter()
            .filter(|s| s.0.contains(filter.as_str()))
        {
            for &m in mags {
                let mut t = Tally::default();
                for (i, (p, r)) in set.iter().enumerate() {
                    let mut rng = Rng((i as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D) | 1);
                    classify(&f(r, m, &mut rng, q), p, &mut t);
                }
                println!(
                    "  {name:<22}{m:>6.2} {:>5.0} {:>7} {:>6} {:>7} {:>6}",
                    100.0 * t.ok as f32 / trials as f32,
                    t.chroma,
                    t.luma,
                    t.format,
                    t.ring
                );
            }
        }
        println!();
    }
}
