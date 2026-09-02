//! What JSON-shaped payloads actually cost, end to end.
//! `cargo run -p lc-decode --release --example json`
//!
//! §9.4 says author in JSON, transmit CBOR, and claims 30–50% before compression. §6.1
//! puts numbers on how much fits in each version. This walks a set of payloads of the
//! shape §11 expects — config blobs, pairing tokens, contact records, location fixes —
//! from JSON text through CBOR, framing, and compression to a rendered symbol, decodes
//! the render back, and checks the recovered CBOR is the JSON we started with.
//!
//! Every number printed is measured, not from the spec's tables.

use lc_core::{Compression, ContentType, Frame, Packet, Profile};
use lc_decode::Decoder;
use lc_encode::EncodeOptions;
use lc_render::{RenderOptions, render_rgb};

/// Payloads chosen to span the range §11 lists as candidate schema families, in the
/// sizes those families actually occur at.
const CASES: &[(&str, &str)] = &[
    ("contact (§6.1)", r#"{"name":"Fred","loc":"45.00,55.0"}"#),
    (
        "location fix",
        r#"{"lat":51.4779,"lon":-0.0015,"acc":8,"t":1756512000}"#,
    ),
    (
        "allergen profile (App. A)",
        r#"{"allergens":["gluten","peanuts","milk"],"severity":"anaphylaxis"}"#,
    ),
    (
        "device pairing",
        r#"{"ssid":"lab-2g","psk":"correct-horse-battery","sec":"wpa2","ch":6}"#,
    ),
    (
        "membership",
        r#"{"org":"Climbing Co-op","id":40912,"tier":"full","exp":1788048000,"gym":["north","east"]}"#,
    ),
    (
        "device config",
        r#"{"dev":"sensor-14","fw":"2.3.1","net":{"host":"10.0.4.2","port":8883,"tls":true},"rate":30,"tags":["hvac","floor-3"],"cal":[1.004,-0.117,0.998]}"#,
    ),
    (
        "event admission",
        r#"{"event":"EV-2026-0417","seat":{"sec":"104","row":"F","num":12},"holder":"A. N. Other","tier":"ga","gates":[2,3],"issued":1756512000,"exp":1776988800}"#,
    ),
];

fn main() {
    println!(
        "{:<26} {:>5} {:>5} {:>6} {:>7} {:>5} {:>4} {:>5}  round-trip",
        "payload", "json", "cbor", "saved", "framed", "cflag", "ver", "side"
    );
    println!("{}", "-".repeat(88));

    for (name, json) in CASES {
        // §9.4's boundary: parse the JSON, emit CBOR. This is what `encode_json` does
        // internally; done by hand here so the intermediate size is visible.
        let value = lc_core::json::parse(json).expect("valid JSON fixture");
        let cbor = value.to_vec();
        let saved = 100.0 - (cbor.len() as f32 / json.len() as f32) * 100.0;

        // Let the encoder pick compression, which per §9.3 often means "none" at these
        // sizes — a compressor spends its budget warming a dictionary it never gets to use.
        let frame = Frame::compressed(ContentType::Cbor, &cbor).expect("frames");
        let framed = frame.encoded_bits().div_ceil(8);

        let opts = EncodeOptions::screen();
        let sym = lc_encode::encode_json(json, &opts).expect("encodes");

        // Decode the render rather than the encoder's own bit buffer, so the round-trip
        // goes through the real sampling path. 10 px/module is §13.1's preferred pitch;
        // note that its normative floor of 8 does not currently detect at all.
        let raster = render_rgb(
            &sym,
            &RenderOptions {
                module_px: 10,
                ..RenderOptions::screen()
            },
        );
        let out = Decoder::new()
            .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
            .expect("decodes");

        let ok = match &out.packet {
            Packet::Unsigned { body, ctype } => {
                let back = lc_core::cbor::Value::from_slice(body).expect("valid CBOR out");
                *ctype == ContentType::Cbor && back == value
            }
            _ => false,
        };

        println!(
            "{:<26} {:>5} {:>5} {:>5.0}% {:>7} {:>5} {:>4} {:>5}  {}",
            name,
            json.len(),
            cbor.len(),
            saved,
            framed,
            cflag_name(frame.cflag),
            sym.format.version.get(),
            sym.format.version.side(),
            if ok { "ok" } else { "MISMATCH" },
        );
    }

    println!();
    profile_comparison();
    println!();
    round_trip_shape();
}

/// The same payload across all four profiles — §5's density trade, measured.
fn profile_comparison() {
    let json = CASES[6].1;
    println!("`{}` at each profile:", CASES[6].0);
    println!(
        "{:<10} {:>5} {:>6} {:>9}",
        "profile", "ver", "side", "modules"
    );

    for profile in [Profile::M, Profile::C1, Profile::C2, Profile::C3] {
        let opts = EncodeOptions {
            profile,
            reference_border: true,
            ..Default::default()
        };
        match lc_encode::encode_json(json, &opts) {
            Ok(sym) => {
                let side = sym.format.version.side();
                println!(
                    "{:<10} {:>5} {:>3}×{:<2} {:>9}",
                    format!("{:?}", profile),
                    sym.format.version.get(),
                    side,
                    side,
                    side as u32 * side as u32,
                );
            }
            Err(e) => println!("{:<10} {:?}", format!("{:?}", profile), e),
        }
    }
}

/// One payload shown all the way through, so the intermediate forms are inspectable
/// rather than just counted.
fn round_trip_shape() {
    let json = CASES[0].1;
    let value = lc_core::json::parse(json).unwrap();
    let cbor = value.to_vec();

    println!("Worked example — {}", CASES[0].0);
    println!("  json   {:>3} B  {}", json.len(), json);
    println!("  cbor   {:>3} B  {}", cbor.len(), hex(&cbor));
    println!("  back         {}", lc_core::json::to_json(&value));

    let sym = lc_encode::encode_json(json, &EncodeOptions::screen()).unwrap();
    println!(
        "  symbol       v{} {}×{} profile {:?}, ecc luma {} chroma {}, mask {}",
        sym.format.version.get(),
        sym.format.version.side(),
        sym.format.version.side(),
        sym.format.profile,
        sym.format.ecc.luma,
        sym.format.ecc.chroma,
        sym.format.mask,
    );

    let raster = render_rgb(
        &sym,
        &RenderOptions {
            module_px: 10,
            ..RenderOptions::screen()
        },
    );
    let out = Decoder::new()
        .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
        .unwrap();
    let d = &out.diagnostics;
    println!(
        "  decode       luma {} iters, chroma {} iters, chroma recovered {}, snr {:.1} dB, illum residual {:.4}",
        d.luma_iterations,
        d.chroma_iterations,
        d.chroma_recovered,
        d.estimated_snr_db,
        d.illum_residual_rms,
    );
    if let Packet::Unsigned { body, .. } = &out.packet {
        let back = lc_core::cbor::Value::from_slice(body).unwrap();
        println!("  recovered    {}", lc_core::json::to_json(&back));
    }
}

fn cflag_name(c: Compression) -> &'static str {
    match c {
        Compression::None => "none",
        Compression::Deflate => "defl",
        Compression::Brotli => "br",
        Compression::BrotliPreset => "br+d",
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
