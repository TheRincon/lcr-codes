//! What soft decisions are worth: the same symbols decoded twice, once with the LDPC
//! decoder fed the demodulator's LLRs and once with hard decisions
//! ([`Decoder::with_hard_decisions`] — each bit keeps its sign and loses its confidence).
//!
//! `cargo run -p lcr-decode --release --example soft_gain [trials] [profile]`
//!
//! §12.4 says soft information is "worth roughly 2 dB". That is the textbook figure for
//! LDPC on a Gaussian channel, not a measurement of this one, and it is the main thing
//! LDPC buys over a hard-decision code such as QR's Reed–Solomon. This measures it here.
//!
//! Two tables per profile, on the symbols `just profiles` uses (v4, ring on, 12 px/module,
//! each loaded to its own capacity):
//!
//! 1. **Each payload impairment, soft row over hard row.** Tilt and rotation are left out:
//!    they are detection limits, and detection runs before any LLR exists.
//! 2. **The storage question.** Under sensor noise, the heaviest noise each ECC level
//!    survives, soft and hard. Where hard decisions need a stronger level to reach the
//!    noise soft reaches at a weaker one, the payload difference between those two levels
//!    is what giving up soft decisions would cost in bytes.
//!
//! Hard decisions here still feed an LDPC decoder, so this isolates the soft-information
//! gain; it is not a Reed–Solomon baseline. A rate over `n` trials carries a standard error
//! of at most `0.5/√n`: the default 16 resolves about ±12 points, so use 64 or more before
//! quoting a figure.

// The shared harness in `tests/support/ops.rs` carries every impairment; each consumer
// uses a subset.
#![allow(dead_code)]

use lcr_core::capacity::capacity_for_symbol;
use lcr_core::ecc::ldpc::{CHROMA_RATE_INDEX, LUMA_RATE_INDEX, RATES};
use lcr_core::version::{EccLevel, Profile, Version};
use lcr_core::{Compression, Packet};
use lcr_decode::Decoder;
use lcr_encode::EncodeOptions;
use lcr_render::{Raster, RenderOptions, render_rgb};

include!("../tests/support/ops.rs");

const DEFAULT_TRIALS: usize = 16;
const VERSION: u8 = 4;
/// A magnitude counts as survived when at least this share of trials decode.
const RELIABLE: f32 = 0.9;
/// Per-pixel σ in linear light. Far heavier than the envelope's 0.02–0.14: a module is
/// sampled over many pixels, which averages most of it away, so soft decoding at v4 does
/// not start failing until well past the envelope's range.
const NOISE: [f32; 15] = [
    0.10, 0.15, 0.20, 0.25, 0.30, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60, 0.65, 0.70, 0.75, 0.80,
];

/// The payload impairments, each on a ladder of its own rather than the envelope's. The
/// envelope's steps are set where a *soft* decoder's interesting range is at the default
/// payload; at v4 most of them never find soft decoding's knee, which is half of what is
/// being compared.
fn ladders() -> [(Impair, &'static [f32]); 6] {
    [
        (Impair::Shading, &[0.6, 0.7, 0.8, 0.85, 0.9, 0.95]),
        (Impair::SpecularStrength, &[0.6, 0.8, 1.0, 1.2, 1.4, 1.6]),
        (Impair::SpecularRadius, &[0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]),
        (Impair::Noise, &NOISE),
        (Impair::Blur, &[2.0, 4.0, 6.0, 8.0, 10.0, 12.0]),
        (
            Impair::Occlusion,
            &[0.10, 0.15, 0.20, 0.25, 0.30, 0.35, 0.40, 0.45, 0.50, 0.55],
        ),
    ]
}

/// One clean symbol per trial at `profile` and `ecc`, each loaded to capacity with a
/// different payload. Returns the payload size too.
fn bases(profile: Profile, ecc: EccLevel, trials: usize) -> (usize, Vec<(Vec<u8>, Raster)>) {
    let v = Version::new(VERSION).unwrap();
    let bytes = capacity_for_symbol(v, profile, ecc, true).body_bytes;
    let opts = EncodeOptions {
        profile,
        version: Some(v),
        ecc,
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    let set = (0..trials)
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
        .collect();
    (bytes, set)
}

fn ok(dec: &Decoder, r: &Raster, expect: &[u8]) -> bool {
    matches!(
        dec.decode_rgb(&r.data, r.width, r.height, r.width * 3).map(|o| o.packet),
        Ok(Packet::Unsigned { ref body, .. }) if body == expect
    )
}

/// Success rate at one magnitude. Trial `t` draws the same nuisance parameters in both
/// modes, so soft and hard see identical images.
fn rate(dec: &Decoder, imp: Impair, set: &[(Vec<u8>, Raster)], magnitude: f32) -> f32 {
    let hits = set
        .iter()
        .enumerate()
        .filter(|(t, (p, r))| {
            let seed = (*t as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D);
            ok(dec, &apply(imp, r, magnitude, seed), p)
        })
        .count();
    hits as f32 / set.len() as f32
}

/// Success rate at each magnitude in turn. Once two in a row decode nothing the rest are
/// taken as zero rather than run: every impairment here only gets worse up its ladder, and
/// a failing decode — every equalising retry spent — is the expensive kind.
fn sweep(dec: &Decoder, imp: Impair, set: &[(Vec<u8>, Raster)], mags: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(mags.len());
    for &m in mags {
        let dead = out.len() >= 2 && out[out.len() - 2..].iter().all(|&r: &f32| r == 0.0);
        out.push(if dead { 0.0 } else { rate(dec, imp, set, m) });
    }
    out
}

/// The last magnitude before the first one that falls below [`RELIABLE`], scanning up.
fn knee(rates: &[f32], mags: &[f32]) -> String {
    match rates.iter().position(|&r| r < RELIABLE) {
        Some(0) => format!("<{}", mags[0]),
        Some(i) => format!("{}", mags[i - 1]),
        None => format!("≥{}", mags[mags.len() - 1]),
    }
}

fn row(rates: &[f32]) -> String {
    rates
        .iter()
        .map(|r| format!("{:>5.0}", 100.0 * r))
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let trials = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(DEFAULT_TRIALS);
    let all = [Profile::M, Profile::C1, Profile::C2, Profile::C3];
    let profiles: Vec<Profile> = match args.next() {
        Some(name) => {
            let p = all
                .into_iter()
                .find(|p| format!("{p:?}").eq_ignore_ascii_case(&name))
                .unwrap_or_else(|| panic!("unknown profile `{name}`: use M, C1, C2 or C3"));
            vec![p]
        }
        None => all.to_vec(),
    };
    let soft = Decoder::new();
    let hard = Decoder::new().with_hard_decisions(true);

    println!(
        "v{VERSION}, ring on, 12 px/module, {trials} trials per cell — success %, ±{:.0} points\n",
        100.0 * 0.5 / (trials as f32).sqrt()
    );

    for profile in profiles {
        let (bytes, set) = bases(profile, EccLevel::DEFAULT, trials);
        let clean = |d: &Decoder| set.iter().filter(|(p, r)| ok(d, r, p)).count();
        println!(
            "{profile:?} at ECC level 1, {bytes} B — clean control: soft {}/{trials}, hard {}/{trials}",
            clean(&soft),
            clean(&hard)
        );
        println!("  {:<20} {:<5}", "", "knee");
        for (imp, mags) in ladders() {
            let s = sweep(&soft, imp, &set, mags);
            let h = sweep(&hard, imp, &set, mags);
            let labels: String = mags.iter().map(|m| format!("{m:>5}")).collect();
            println!("  {:<20} {:<5} {}", imp.name(), "", labels);
            println!("  {:<20} {:<5} {}", "  soft", knee(&s, mags), row(&s));
            println!("  {:<20} {:<5} {}", "  hard", knee(&h, mags), row(&h));
        }

        // Table 2. Profile `M` has no chroma layer, so its chroma level is moot; the
        // levels move together everywhere else, as `EccLevel::DEFAULT` does.
        println!(
            "\n  noise knee by ECC level (heaviest σ with ≥{:.0}% decoding)",
            100.0 * RELIABLE
        );
        println!("  level  luma/chroma rate  payload   soft    hard");
        for level in 0..=3u8 {
            let ecc = EccLevel::new(level, level).unwrap();
            let (bytes, set) = bases(profile, ecc, trials);
            let s = sweep(&soft, Impair::Noise, &set, &NOISE);
            let h = sweep(&hard, Impair::Noise, &set, &NOISE);
            let luma = RATES[LUMA_RATE_INDEX[level as usize]];
            let chroma = match profile {
                Profile::M => "   —".to_string(),
                _ => format!("{:.2}", RATES[CHROMA_RATE_INDEX[level as usize]]),
            };
            println!(
                "  {level:>5}  {luma:.2} / {chroma:<4}       {bytes:>5} B  {:>6}  {:>6}",
                knee(&s, &NOISE),
                knee(&h, &NOISE)
            );
        }
        println!();
    }
}
