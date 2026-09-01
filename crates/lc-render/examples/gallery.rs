//! Generate the `examples/` gallery: real symbols for representative payloads, plus an
//! index recording what each one is and what it cost.
//!
//! `cargo run -p lc-render --release --example gallery -- examples`
//!
//! Every symbol written here is round-tripped through the decoder before it is saved, so
//! nothing lands in the gallery that cannot be read back.

use lc_core::{ContentType, Packet, Profile};
use lc_decode::Decoder;
use lc_encode::EncodeOptions;
use lc_render::{RenderOptions, render_png, render_rgb, render_svg};

/// Payloads spanning the §11 candidate schema families, at the sizes they occur at.
const PAYLOADS: &[(&str, &str, &str)] = &[
    ("contact", r#"{"name":"Fred","loc":"45.00,55.0"}"#, "The §6.1 worked example."),
    (
        "location-fix",
        r#"{"lat":51.4779,"lon":-0.0015,"acc":8,"t":1756512000}"#,
        "A position with accuracy and a timestamp.",
    ),
    (
        "allergen-profile",
        r#"{"allergens":["gluten","peanuts","milk"],"severity":"anaphylaxis"}"#,
        "Appendix A, written out as JSON rather than schema-coded — the size argument \
         the schema registry (§11) exists to win back.",
    ),
    (
        "device-pairing",
        r#"{"ssid":"lab-2g","psk":"correct-horse-battery","sec":"wpa2","ch":6}"#,
        "Wi-Fi handoff with no network in between, which is §1.1 goal 1 exactly.",
    ),
    (
        "membership",
        r#"{"org":"Climbing Co-op","id":40912,"tier":"full","exp":1788048000,"gym":["north","east"]}"#,
        "Unsigned (§10.7): the holder asserts it, nobody attests it.",
    ),
    (
        "device-config",
        r#"{"dev":"sensor-14","fw":"2.3.1","net":{"host":"10.0.4.2","port":8883,"tls":true},"rate":30,"tags":["hvac","floor-3"],"cal":[1.004,-0.117,0.998]}"#,
        "Nested objects and a float array — provisioning a device off a screen.",
    ),
    (
        "event-admission",
        r#"{"event":"EV-2026-0417","seat":{"sec":"104","row":"F","num":12},"holder":"A. N. Other","tier":"ga","gates":[2,3],"issued":1756512000,"exp":1776988800}"#,
        "The shape that would carry a COSE signature (§10) in a deployment that needed one.",
    ),
];

struct Row {
    file: String,
    what: String,
    json: usize,
    cbor: usize,
    version: u8,
    side: u16,
    profile: &'static str,
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "examples".into());
    std::fs::create_dir_all(&dir).expect("create output directory");
    let mut rows: Vec<Row> = Vec::new();

    // One symbol per payload, at the §5 screen default.
    for (name, json, what) in PAYLOADS {
        rows.push(write_one(&dir, name, json, what, Profile::C1, true));
    }

    // The same payload at every profile — §5's density trade, side by side.
    let (_, json, _) = PAYLOADS[6];
    for (profile, label) in [
        (Profile::M, "m"),
        (Profile::C1, "c1"),
        (Profile::C2, "c2"),
        (Profile::C3, "c3"),
    ] {
        let what = match profile {
            Profile::M => "Monochrome fallback (§4.6). Luma layer only — this is what a \
                           mono sensor or an unrecoverable chroma layer leaves you.",
            Profile::C1 => "Screen default: 4 colours, co-sited chroma, 2.00 bits/module.",
            Profile::C2 => "Print default: 4 colours, 2×2 chroma cells, more robust and \
                            less dense.",
            Profile::C3 => "8 colours, 2×2 cells. Optional (§5) and wants a good capture.",
        };
        rows.push(write_one(&dir, &format!("profile-{label}"), json, what, profile, true));
    }

    // The §6.4 border is optional, signalled by format bit 14. Two payloads without it,
    // matching `contact` and `event-admission` above so the pair can be compared directly.
    for (i, name) in [(0usize, "contact"), (6, "event-admission")] {
        let (_, json, _) = PAYLOADS[i];
        rows.push(write_one(
            &dir,
            &format!("{name}-no-border"),
            json,
            "No §6.4 reference border. Same payload, same version — the ring is outside the \
             symbol grid, so dropping it changes nothing about what is encoded.",
            Profile::C1,
            false,
        ));
    }

    write_index(&dir, &rows);
    println!("\n{} symbols in {dir}/", rows.len());
}

fn write_one(
    dir: &str,
    name: &str,
    json: &str,
    what: &str,
    profile: Profile,
    border: bool,
) -> Row {
    let opts = EncodeOptions { profile, reference_border: border, ..Default::default() };
    let sym = lc_encode::encode_json(json, &opts).expect("encode");
    let cbor = lc_core::json::parse(json).unwrap().to_vec();

    // §13.1 prefers 10–12 device pixels per module; 12 keeps the PNGs legible when a
    // reader zooms in on one.
    let render_opts = RenderOptions { module_px: 12, ..RenderOptions::screen() };

    // Nothing goes in the gallery that will not decode.
    let raster = render_rgb(&sym, &render_opts);
    let out = Decoder::new()
        .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
        .unwrap_or_else(|e| panic!("{name} does not decode: {e}"));
    match &out.packet {
        Packet::Unsigned { body, ctype } => {
            assert_eq!(*ctype, ContentType::Cbor, "{name}");
            let back = lc_core::cbor::Value::from_slice(body).expect("valid CBOR");
            assert_eq!(lc_core::json::to_json(&back), json, "{name} round-trip");
        }
        other => panic!("{name}: {other:?}"),
    }

    std::fs::write(format!("{dir}/{name}.png"), render_png(&sym, &render_opts)).unwrap();
    std::fs::write(format!("{dir}/{name}.svg"), render_svg(&sym, &render_opts)).unwrap();
    std::fs::write(format!("{dir}/{name}.json"), json).unwrap();

    println!(
        "{name:<20} v{:<2} {}×{}  {:?}  {} B json → {} B cbor",
        sym.format.version.get(),
        sym.format.version.side(),
        sym.format.version.side(),
        profile,
        json.len(),
        cbor.len()
    );

    Row {
        file: name.to_string(),
        what: what.split_whitespace().collect::<Vec<_>>().join(" "),
        json: json.len(),
        cbor: cbor.len(),
        version: sym.format.version.get(),
        side: sym.format.version.side(),
        profile: match profile {
            Profile::M => "M",
            Profile::C1 => "C1",
            Profile::C2 => "C2",
            Profile::C3 => "C3",
        },
    }
}

fn write_index(dir: &str, rows: &[Row]) {
    let mut s = String::new();
    s.push_str("# LC Code example symbols\n\n");
    s.push_str(
        "Generated, not hand-made. Regenerate with:\n\n\
         ```\ncargo run -p lc-render --release --example gallery -- examples\n```\n\n\
         Each entry is a `.png` (12 device pixels per module, §13.1's preferred range), \
         the same symbol as `.svg`, and the source `.json`. Every one is decoded before \
         it is written, so anything here reads back to the JSON beside it.\n\n\
         Payloads travel as CBOR, not JSON — §9.4's \"author in JSON, transmit CBOR\". \
         The two size columns are that conversion.\n\n",
    );

    s.push_str("## Payloads\n\n");
    s.push_str("| Symbol | Profile | Version | Side | JSON | CBOR | What it is |\n");
    s.push_str("|---|---|---|---|---|---|---|\n");
    for r in rows.iter().take(PAYLOADS.len()) {
        s.push_str(&format!(
            "| [`{}`]({}.png) | {} | v{} | {}×{} | {} B | {} B | {} |\n",
            r.file, r.file, r.profile, r.version, r.side, r.side, r.json, r.cbor, r.what
        ));
    }

    s.push_str("\n## The same payload at every profile\n\n");
    s.push_str(
        "`event-admission`, encoded four ways. This is §5's density trade made visible: \
         the monochrome fallback needs a visibly bigger symbol for the same bytes.\n\n",
    );
    s.push_str("| Symbol | Profile | Version | Side | Modules | What it is |\n");
    s.push_str("|---|---|---|---|---|---|\n");
    for r in rows.iter().skip(PAYLOADS.len()).take(4) {
        s.push_str(&format!(
            "| [`{}`]({}.png) | {} | v{} | {}×{} | {} | {} |\n",
            r.file,
            r.file,
            r.profile,
            r.version,
            r.side,
            r.side,
            r.side as u32 * r.side as u32,
            r.what
        ));
    }

    s.push_str("\n## Without the reference border\n\n");
    s.push_str(
        "The §6.4 ring is **optional** — signalled by format bit 14, and these two decode \
         without it. It lives outside the symbol grid, so removing it changes nothing about \
         what is encoded: same payload, same version, same module pattern. Compare each \
         against its bordered twin above.\n\n\
         It is not free to drop, though. Measured over a 120-case tilt × glare grid, \
         decoding succeeds in 65 cases with the border and 53 without — the ring gives the \
         §12.3 illumination fit known-colour samples at the geometric outside of the \
         symbol, which is where a global fit is weakest and where glare gradients are \
         strongest. §6.4's own note is that on screen it costs pixels nobody was using; in \
         print the area would not be worth it.\n\n",
    );
    s.push_str("| Symbol | Profile | Version | Side | JSON | CBOR |\n");
    s.push_str("|---|---|---|---|---|---|\n");
    for r in rows.iter().skip(PAYLOADS.len() + 4) {
        s.push_str(&format!(
            "| [`{}`]({}.png) | {} | v{} | {}×{} | {} B | {} B |\n",
            r.file, r.file, r.profile, r.version, r.side, r.side, r.json, r.cbor
        ));
    }

    s.push_str(
        "\n## Reading one back\n\n\
         ```rust\n\
         let px = /* RGB8 bytes from a camera frame or a decoded PNG */;\n\
         let out = lc_decode::Decoder::new().decode_rgb(&px, w, h, w * 3)?;\n\
         if let lc_core::Packet::Unsigned { body, .. } = out.packet {\n\
         \x20   let value = lc_core::cbor::Value::from_slice(&body)?;\n\
         \x20   println!(\"{}\", lc_core::json::to_json(&value));\n\
         }\n\
         ```\n\n\
         All of these are **unsigned** (§10.7). Signing is opt-in and §1.2 expects unsigned \
         structured data to be the common case; `lc-verify` is not built yet in any event.\n",
    );

    std::fs::write(format!("{dir}/README.md"), s).unwrap();
}
