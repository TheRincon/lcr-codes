//! Compare the colour profiles head to head at a fixed symbol size.
//!
//! `cargo run -p lc-decode --release --example profiles [trials]`
//!
//! ## The question this answers
//!
//! §5 offers three colour profiles and says almost nothing about how to choose between
//! them beyond intended use. The choice is a density/robustness trade, and only the
//! density half was ever computed — `xtask capacity` gives bytes per profile, but nothing
//! measured what those bytes cost in margin.
//!
//! So: **one symbol size, each profile loaded to its own capacity.** That is the honest
//! comparison, because it is how a profile is actually used — a denser profile carries
//! more in the same square, and if it is no less robust while doing so it simply wins.
//! Holding the payload equal instead would under-fill the denser profiles, and LDPC
//! shortening would hand them extra protection they would not have in practice.
//!
//! Reported as `m50`: the impairment magnitude at which success falls through 50%. Bigger
//! is better, and the units differ per impairment, so compare down a row, never across.

#![allow(dead_code)]

use lc_core::capacity::capacity;
use lc_core::version::{EccLevel, Profile, Version};
use lc_core::{Compression, Packet};
use lc_decode::Decoder;
use lc_encode::EncodeOptions;
use lc_render::{Raster, RenderOptions, render_rgb};

include!("../tests/support/ops.rs");

/// Fixed at 33×33 — the same grid a QR version 4 occupies, which makes the capacity
/// column directly comparable to published QR figures.
const VERSION: u8 = 4;
const DEFAULT_TRIALS: usize = 16;
const STEPS: usize = 11;

/// Photometric impairments. Tilt and rotation are excluded deliberately: they damage the
/// *geometry*, which every profile shares, and the harness's tilt sweep has a known
/// resampling artefact (see `Impair::Tilt`).
const SWEEP: [Impair; 6] = [
    Impair::Noise,
    Impair::Blur,
    Impair::SpecularStrength,
    Impair::SpecularRadius,
    Impair::Shading,
    Impair::Occlusion,
];

/// One profile's trial set: each payload paired with the clean symbol carrying it.
type TrialSet = Vec<(Vec<u8>, Raster)>;

fn bases(profile: Profile, trials: usize) -> (usize, TrialSet) {
    let v = Version::new(VERSION).unwrap();
    let bytes = capacity(v, profile, EccLevel::DEFAULT).body_bytes;
    let opts = EncodeOptions {
        profile,
        version: Some(v),
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    let out = (0..trials)
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
        .collect();
    (bytes, out)
}

fn ok(r: &Raster, expect: &[u8]) -> bool {
    matches!(
        Decoder::new()
            .decode_rgb(&r.data, r.width, r.height, r.width * 3)
            .map(|o| o.packet),
        Ok(Packet::Unsigned { ref body, .. }) if body == expect
    )
}

fn fine(imp: Impair) -> Vec<f32> {
    let m = imp.magnitudes();
    let (lo, hi) = (m[0], m[m.len() - 1]);
    (0..STEPS)
        .map(|i| lo + (hi - lo) * i as f32 / (STEPS - 1) as f32)
        .collect()
}

/// Magnitude at which the success rate crosses 50%, interpolated.
fn m50(imp: Impair, set: &[(Vec<u8>, Raster)]) -> Option<f32> {
    let mags = fine(imp);
    let mut prev = 1.0f32;
    for (i, &m) in mags.iter().enumerate() {
        let hits = set
            .iter()
            .enumerate()
            .filter(|(t, (p, r))| {
                let seed = (*t as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D);
                ok(&apply(imp, r, m, seed), p)
            })
            .count();
        let rate = hits as f32 / set.len() as f32;
        if rate < 0.5 && i > 0 && prev >= 0.5 {
            let t = (prev - 0.5) / (prev - rate).max(1e-6);
            return Some(mags[i - 1] + t * (m - mags[i - 1]));
        }
        prev = rate;
    }
    None
}

fn main() {
    let trials = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(DEFAULT_TRIALS);
    let profiles = [Profile::C1, Profile::C2, Profile::C3];

    let v = Version::new(VERSION).unwrap();
    println!(
        "v{VERSION} ({}×{}), each profile at its own capacity, {trials} trials/point.",
        v.side(),
        v.side()
    );
    println!("m50 = magnitude at 50% success; higher is better. Compare down a row.\n");

    let sets: Vec<(Profile, usize, TrialSet)> = profiles
        .iter()
        .map(|&p| {
            let (bytes, b) = bases(p, trials);
            (p, bytes, b)
        })
        .collect();

    print!("  {:<20}", "");
    for (p, bytes, _) in &sets {
        print!("{:>14}", format!("{p:?} ({bytes} B)"));
    }
    println!();

    // Control: an undegraded symbol must decode for every profile, or the rest is noise.
    print!("  {:<20}", "clean control");
    for (_, _, set) in &sets {
        let n = set.iter().filter(|(p, r)| ok(r, p)).count();
        print!("{:>14}", format!("{n}/{trials}"));
    }
    println!();

    for imp in SWEEP {
        print!("  {:<20}", imp.name());
        for (_, _, set) in &sets {
            match m50(imp, set) {
                Some(v) => print!("{v:>14.3}"),
                None => print!("{:>14}", "> range"),
            }
        }
        println!();
    }
}
