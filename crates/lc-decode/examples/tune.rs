//! Measure the §8.2 rate table against the channel — §14.5 step 9.
//!
//! `cargo run -p lc-decode --release --example tune [trials]`
//!
//! §8.2 pins four ECC levels, each with a luma rate and a lower chroma rate, and says the
//! inequality is load-bearing: "chroma bits are measured over a noisier, lower-bandwidth
//! channel. Equal rates would either overprotect luma or underprotect chroma." The table
//! itself is labelled a guess.
//!
//! The right calibration criterion follows from that sentence. Protection is balanced when
//! the two layers **fail at the same degradation magnitude**. If chroma gives out first,
//! its rate is too high and the luma redundancy above it is wasted — the symbol fails as
//! soon as chroma does. If luma gives out first, chroma is carrying redundancy it never
//! gets to use. Either way the mismatch is spent area.
//!
//! So this reports, per ECC level and impairment, the magnitude at which each layer
//! crosses 50% success:
//!
//! * **luma m50** — where `luma` stops converging, measured as the point the decoder can
//!   no longer return even a `Partial` prefix.
//! * **chroma m50** — where a *full* decode stops, which needs both layers.
//!
//! `Δ` is the gap between them in magnitude units, normalised by the luma threshold so
//! impairments on different scales can be compared. Negative means chroma fails first.

#![allow(dead_code)]

use lc_core::capacity::capacity_for_symbol;
use lc_core::version::Version;
use lc_core::{Compression, EccLevel, Packet, Profile};
use lc_decode::{DecodeOptions, Decoder};
use lc_encode::EncodeOptions;
use lc_render::{Raster, RenderOptions, render_rgb};

include!("../tests/support/ops.rs");

/// Symbol size held fixed, with the payload sized to whatever the profile and ECC level
/// can actually carry. Under-filling a layer is not neutral: LDPC shortening drops the
/// effective rate, so a half-empty layer is better protected than the table says and the
/// balance being measured is not the one that ships.
const VERSION: u8 = 4;
const DEFAULT_TRIALS: usize = 12;
const STEPS: usize = 11;

/// Photometric impairments only. Tilt and rotation degrade the *geometry*, which both
/// layers share, so they move the two thresholds together and say nothing about the
/// rate split. These six hit luma and chroma differently, which is what is being measured.
const TUNING: [Impair; 6] = [
    Impair::Noise,
    Impair::SpecularStrength,
    Impair::SpecularRadius,
    Impair::Blur,
    Impair::Shading,
    Impair::Occlusion,
];

/// Per-layer SNR of one capture, in dB, on the same scale as `docs/ldpc.md`.
fn snrs(r: &Raster) -> Option<(f32, f32)> {
    let decoder = Decoder::new().with_options(DecodeOptions {
        allow_partial: true,
        ..Default::default()
    });
    // A capture too damaged to decode contributes no sample, which biases the reported
    // means *upward* at high magnitudes. Noted rather than corrected: the quantity of
    // interest is the gap between the layers, which that bias moves far less than it
    // moves either level on its own.
    decoder
        .decode_rgb(&r.data, r.width, r.height, r.width * 3)
        .ok()
        .map(|o| (o.diagnostics.estimated_snr_db, o.diagnostics.chroma_snr_db))
}

/// Outcome of one trial, as the two layers see it.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Outcome {
    /// Both layers decoded.
    Full,
    /// Luma converged, chroma did not — §12.5's prefix.
    LumaOnly,
    /// Neither.
    Dead,
}

fn base(payload: &[u8], ecc: EccLevel, profile: Profile) -> Raster {
    let opts = EncodeOptions {
        profile,
        ecc,
        version: Some(Version::new(VERSION).unwrap()),
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    let sym = lc_encode::encode_bytes(payload, &opts).unwrap();
    render_rgb(
        &sym,
        &RenderOptions {
            module_px: 12,
            ..RenderOptions::screen()
        },
    )
}

fn outcome(r: &Raster, expect: &[u8]) -> Outcome {
    let decoder = Decoder::new().with_options(DecodeOptions {
        allow_partial: true,
        ..Default::default()
    });
    match decoder.decode_rgb(&r.data, r.width, r.height, r.width * 3) {
        Ok(o) => match &o.packet {
            Packet::Unsigned { body, .. } if body == expect => Outcome::Full,
            Packet::Partial { .. } => Outcome::LumaOnly,
            // A wrong body means the CRC passed on corrupt data, or a layer silently
            // mis-decoded. Neither is a success.
            _ => Outcome::Dead,
        },
        Err(_) => Outcome::Dead,
    }
}

/// Magnitudes spanning the impairment's range, finer than the envelope's grid so a
/// threshold can be interpolated rather than guessed between two far-apart cells.
fn fine(imp: Impair) -> Vec<f32> {
    let m = imp.magnitudes();
    let (lo, hi) = (m[0], m[m.len() - 1]);
    (0..STEPS)
        .map(|i| lo + (hi - lo) * i as f32 / (STEPS - 1) as f32)
        .collect()
}

/// First magnitude where `rates` drops below 0.5, linearly interpolated between the
/// bracketing samples. `None` if it never does.
fn m50(mags: &[f32], rates: &[f32]) -> Option<f32> {
    for i in 1..rates.len() {
        if rates[i] < 0.5 && rates[i - 1] >= 0.5 {
            let t = (rates[i - 1] - 0.5) / (rates[i - 1] - rates[i]).max(1e-6);
            return Some(mags[i - 1] + t * (mags[i] - mags[i - 1]));
        }
    }
    None
}

fn main() {
    let trials = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(DEFAULT_TRIALS);
    let profile = match std::env::args().nth(2).as_deref() {
        Some("C2") | Some("c2") => Profile::C2,
        Some("C3") | Some("c3") => Profile::C3,
        _ => Profile::C1,
    };
    let v = Version::new(VERSION).unwrap();

    println!(
        "{profile:?}, v{VERSION} ({}×{}) filled to capacity, {trials} trials/cell.\n\
         m50 = magnitude at 50% success. luma m50 = prefix still recoverable; \
         chroma m50 = full decode.\nΔ<0 means chroma fails first — i.e. chroma is the \
         layer holding the symbol back.\n",
        v.side(),
        v.side()
    );

    for level in 0..4u8 {
        let ecc = EccLevel {
            luma: level,
            chroma: level,
        };
        // The ring layout this renders with (D17/D19), not the classic one: sizing with
        // `capacity` under-filled every symbol and measured spare ECC margin instead of
        // the rate split.
        let bytes = capacity_for_symbol(v, profile, ecc, true).body_bytes;
        let cap = capacity_for_symbol(v, profile, ecc, true);
        let share = 100.0 * cap.chroma_info_bits as f32 / cap.info_bits().max(1) as f32;
        let bases: Vec<(Vec<u8>, Raster)> = (0..trials)
            .map(|t| {
                let p = trial_payload(bytes, t as u64 * 0x9E37 + 1);
                let r = base(&p, ecc, profile);
                (p, r)
            })
            .collect();

        let rates = lc_core::ecc::ldpc::RATES;
        println!(
            "── ECC level {level}  luma {:.2} / chroma {:.2}  —  {bytes} B, \
             chroma carries {share:.0}%",
            rates[lc_core::ecc::ldpc::LUMA_RATE_INDEX[level as usize]],
            rates[lc_core::ecc::ldpc::CHROMA_RATE_INDEX[level as usize]],
        );
        println!(
            "   {:<20} {:>9} {:>9} {:>8}",
            "impairment", "luma m50", "chr m50", "Δ/luma"
        );

        let mut deltas = Vec::new();
        for imp in TUNING {
            let mags = fine(imp);
            let mut luma_r = Vec::new();
            let mut chroma_r = Vec::new();
            for &m in &mags {
                let mut l = 0usize;
                let mut c = 0usize;
                for (t, (payload, raster)) in bases.iter().enumerate() {
                    let seed = (t as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D);
                    match outcome(&apply(imp, raster, m, seed), payload) {
                        Outcome::Full => {
                            l += 1;
                            c += 1;
                        }
                        Outcome::LumaOnly => l += 1,
                        Outcome::Dead => {}
                    }
                }
                luma_r.push(l as f32 / trials as f32);
                chroma_r.push(c as f32 / trials as f32);
            }
            let (lm, cm) = (m50(&mags, &luma_r), m50(&mags, &chroma_r));
            let d = match (lm, cm) {
                (Some(l), Some(c)) if l > 0.0 => {
                    deltas.push((c - l) / l);
                    format!("{:+7.0}%", 100.0 * (c - l) / l)
                }
                _ => "      —".into(),
            };
            let f = |v: Option<f32>| v.map_or("     —".into(), |x| format!("{x:6.3}"));
            println!("   {:<20} {:>9} {:>9} {d:>8}", imp.name(), f(lm), f(cm));
        }
        if !deltas.is_empty() {
            let mean = deltas.iter().sum::<f32>() / deltas.len() as f32;
            println!(
                "   {:<20} {:>28}",
                "mean Δ",
                format!("{:+.0}%", 100.0 * mean)
            );
        }
        println!();

        // The measurement §8.2's rate split actually turns on.
        if level == 0 {
            println!("   per-layer SNR (dB), measured from the symbol's own references");
            println!(
                "   {:<20} {:>8} {:>8} {:>8} {:>7}",
                "impairment", "mag", "luma", "chroma", "gap"
            );
            let mut gaps = Vec::new();
            for imp in TUNING {
                let mags = fine(imp);
                for &m in [mags[0], mags[mags.len() / 2]].iter() {
                    let mut acc = Vec::new();
                    for (t, (_, raster)) in bases.iter().enumerate() {
                        let seed = (t as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D);
                        if let Some(v) = snrs(&apply(imp, raster, m, seed)) {
                            acc.push(v);
                        }
                    }
                    if acc.is_empty() {
                        continue;
                    }
                    let n = acc.len() as f32;
                    let l = acc.iter().map(|v| v.0).sum::<f32>() / n;
                    let c = acc.iter().map(|v| v.1).sum::<f32>() / n;
                    gaps.push(l - c);
                    println!(
                        "   {:<20} {m:>8.3} {l:>8.1} {c:>8.1} {:>7.1}",
                        imp.name(),
                        l - c
                    );
                }
            }
            if !gaps.is_empty() {
                let mean = gaps.iter().sum::<f32>() / gaps.len() as f32;
                println!("\n   mean luma-minus-chroma gap: {mean:+.1} dB");
                println!("   §8.2's two-step offset grants chroma 1.8-2.4 dB (docs/ldpc.md).\n");
            }
        }
    }
}
