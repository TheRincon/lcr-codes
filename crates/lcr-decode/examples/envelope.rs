//! Sweep each degradation to find where decoding stops working.
//!
//! `cargo run -p lcr-decode --release --example envelope [trials]`
//!
//! §15 asks for **success rate against degradation magnitude**, and is explicit that "a
//! pass/fail corpus will not tell you whether a change helped". Each cell here is a
//! fraction over independent trials: the magnitude is held exactly, while the payload and
//! the impairment's nuisance parameters — glare position, shading direction, tilt axis,
//! occluder placement — are drawn per trial (see `apply` in the shared harness).
//!
//! The trial count is the accuracy knob. A rate measured over `n` trials carries a
//! standard error of at most `0.5/√n`, so the default 16 resolves about ±12 points and
//! 64 about ±6. Use more when comparing two builds than when eyeballing the shape.

// The shared harness in `tests/support/ops.rs` carries every impairment; each consumer
// uses a subset.
#![allow(dead_code)]

use lcr_core::{Compression, Packet, Profile};
use lcr_decode::Decoder;
use lcr_encode::EncodeOptions;
use lcr_render::{Raster, RenderOptions, render_rgb};

include!("../tests/support/ops.rs");

/// Payload size held constant so every trial lands on the same version and the sweep
/// measures the channel rather than a change of symbol geometry.
const PAYLOAD_BYTES: usize = 96;
const DEFAULT_TRIALS: usize = 16;

fn base(profile: Profile, payload: &[u8]) -> Raster {
    let opts = EncodeOptions {
        profile,
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    let sym = lcr_encode::encode_bytes(payload, &opts).unwrap();
    render_rgb(
        &sym,
        &RenderOptions {
            module_px: 12,
            ..RenderOptions::screen()
        },
    )
}

/// One clean symbol per trial, each carrying a different payload.
fn trial_bases(profile: Profile, trials: usize) -> Vec<(Vec<u8>, Raster)> {
    (0..trials)
        .map(|t| {
            let payload = trial_payload(PAYLOAD_BYTES, t as u64 * 0x9E37 + 1);
            let raster = base(profile, &payload);
            (payload, raster)
        })
        .collect()
}

fn ok(r: &Raster, expect: &[u8]) -> bool {
    matches!(
        Decoder::new()
            .decode_rgb(&r.data, r.width, r.height, r.width * 3)
            .map(|o| o.packet),
        Ok(Packet::Unsigned { ref body, .. }) if body == expect
    )
}

/// Success rate at one magnitude, over every trial.
fn success_rate(imp: Impair, bases: &[(Vec<u8>, Raster)], magnitude: f32) -> f32 {
    let hits = bases
        .iter()
        .enumerate()
        .filter(|(t, (payload, raster))| {
            let seed = (*t as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D);
            ok(&apply(imp, raster, magnitude, seed), payload)
        })
        .count();
    hits as f32 / bases.len() as f32
}

fn main() {
    let trials = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(DEFAULT_TRIALS);
    let bases = trial_bases(Profile::C1, trials);

    println!(
        "C1, {PAYLOAD_BYTES} B payload, {trials} trials per cell — success rate %, \
         ±{:.0} points\n",
        100.0 * 0.5 / (trials as f32).sqrt()
    );

    // Control. Every clean symbol must decode, or the rates below are measuring the
    // encoder rather than the channel.
    let clean = bases.iter().filter(|(p, r)| ok(r, p)).count();
    println!(
        "clean control          {:>3}   ({clean}/{trials} undegraded symbols decode)\n",
        100 * clean / trials
    );

    for imp in Impair::ALL {
        let mags = imp.magnitudes();
        let rates: String = mags
            .iter()
            .map(|&m| format!("{:>5.0}", 100.0 * success_rate(imp, &bases, m)))
            .collect();
        let labels: String = mags.iter().map(|m| format!("{m:>5}")).collect();
        println!("{:<20} {rates}\n{:<20} {labels}", imp.name(), "");
    }
}
