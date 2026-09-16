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
    (
        "contact",
        r#"{"name":"Fred","loc":"45.00,55.0"}"#,
        "The §6.1 worked example.",
    ),
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
    /// How the symbol is found: "Ring" (finder-less, D17) or "Finders" (classic).
    layout: &'static str,
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
            Profile::M => {
                "Monochrome fallback (§4.6). Luma layer only — this is what a \
                           mono sensor or an unrecoverable chroma layer leaves you."
            }
            Profile::C1 => "Screen default: 4 colours, co-sited chroma, 2.00 bits/module.",
            Profile::C2 => {
                "Print default: 4 colours, 2×2 chroma cells, more robust and \
                            less dense."
            }
            Profile::C3 => "8 colours, 2×2 cells. Optional (§5) and wants a good capture.",
        };
        rows.push(write_one(
            &dir,
            &format!("profile-{label}"),
            json,
            what,
            profile,
            true,
        ));
    }

    // Without the §6.4 ring a symbol keeps its finders (D17). Two payloads without it,
    // matching `contact` and `event-admission` above so each pair can be compared directly.
    for (i, name) in [(0usize, "contact"), (6, "event-admission")] {
        let (_, json, _) = PAYLOADS[i];
        rows.push(write_one(
            &dir,
            &format!("{name}-no-border"),
            json,
            "No §6.4 ring, so the classic layout: finders, alignment and format region. \
             Readable from a greyscale capture.",
            Profile::C1,
            false,
        ));
    }

    write_index(&dir, &rows);
    println!("\n{} symbols in {dir}/", rows.len());
}

fn write_one(dir: &str, name: &str, json: &str, what: &str, profile: Profile, border: bool) -> Row {
    let opts = EncodeOptions {
        profile,
        reference_border: border,
        ..Default::default()
    };
    let sym = lc_encode::encode_json(json, &opts).expect("encode");
    let cbor = lc_core::json::parse(json).unwrap().to_vec();

    // §13.1 prefers 10–12 device pixels per module; 12 keeps the PNGs legible when a
    // reader zooms in on one.
    let render_opts = RenderOptions {
        module_px: 12,
        ..RenderOptions::screen()
    };

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
        "{name:<24} v{:<2} {}×{}  {:?}  {:<7}  {} B json → {} B cbor",
        sym.format.version.get(),
        sym.format.version.side(),
        sym.format.version.side(),
        profile,
        layout(profile, border),
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
        layout: layout(profile, border),
    }
}

fn layout(profile: Profile, border: bool) -> &'static str {
    if lc_core::layout::Layout::is_finderless(profile, border) {
        "Ring"
    } else {
        "Finders"
    }
}

fn write_index(dir: &str, rows: &[Row]) {
    let mut s = String::new();
    s.push_str("# LC Code example symbols\n\n");
    s.push_str(
        "Generated, not hand-made. Regenerate with:\n\n\
         ```\njust gallery\n```\n\n\
         Each entry is a `.png` (12 device pixels per module, §13.1's preferred range), \
         the same symbol as `.svg`, and the source `.json`. Every one is decoded before \
         it is written, so anything here reads back to the JSON beside it.\n\n\
         Payloads travel as CBOR, not JSON — §9.4's \"author in JSON, transmit CBOR\". \
         The two size columns are that conversion. **Layout** is how a symbol is found: \
         **Ring** is finder-less (D17), located and described entirely by its §6.4 ring; \
         **Finders** is the classic layout.\n\n",
    );
    s.push_str(
        "**New to the format?** [`anatomy-c3.png`](anatomy-c3.png) boxes and explains every \
         part of a C3 symbol: corners, sequence, version and format bits, references and \
         payload. Regenerate it with `just anatomy`.\n\n",
    );

    s.push_str("## Payloads\n\n");
    s.push_str("| Symbol | Profile | Layout | Version | Side | JSON | CBOR | What it is |\n");
    s.push_str("|---|---|---|---|---|---|---|---|\n");
    for r in rows.iter().take(PAYLOADS.len()) {
        s.push_str(&format!(
            "| [`{}`]({}.png) | {} | {} | v{} | {}×{} | {} B | {} B | {} |\n",
            r.file, r.file, r.profile, r.layout, r.version, r.side, r.side, r.json, r.cbor, r.what
        ));
    }

    s.push_str("\n## The same payload at every profile\n\n");
    s.push_str(
        "`event-admission`, encoded four ways. This is §5's density trade made visible: \
         the monochrome fallback needs a visibly bigger symbol for the same bytes — partly \
         because it carries one bit per module where C1 carries two, and partly because M \
         keeps its finders even with the ring. Its format cannot ride on hue, because it \
         has none.\n\n",
    );
    s.push_str("| Symbol | Profile | Layout | Version | Side | Modules | What it is |\n");
    s.push_str("|---|---|---|---|---|---|---|\n");
    for r in rows.iter().skip(PAYLOADS.len()).take(4) {
        s.push_str(&format!(
            "| [`{}`]({}.png) | {} | {} | v{} | {}×{} | {} | {} |\n",
            r.file,
            r.file,
            r.profile,
            r.layout,
            r.version,
            r.side,
            r.side,
            r.side as u32 * r.side as u32,
            r.what
        ));
    }

    s.push_str("\n## With the ring, and without\n\n");
    s.push_str(
        "A colour symbol that carries the §6.4 ring has **no finders, separators, palette \
         anchors, alignment marker, timing or format region** (D17). The ring's four \
         forced-dark corners give the homography; its luma position sequence names the \
         version, the rotation and the mirror; and the format codeword rides on the colour \
         choices of the ring positions between its fixed-colour references. Those structures \
         were 204 modules at every version — 46% of a v1 — and all of them go back to \
         payload.\n\n\
         Without the ring a symbol keeps the classic layout, and that is still the right \
         choice wherever a capture might be greyscale: a finder-less symbol's format is \
         carried in hue, so a mono sensor can locate one but cannot read it.\n\n\
         These are the same payloads as their ringed twins above.\n\n",
    );
    s.push_str("| Symbol | Profile | Layout | Version | Side | Ringed twin | JSON | CBOR |\n");
    s.push_str("|---|---|---|---|---|---|---|---|\n");
    for r in rows.iter().skip(PAYLOADS.len() + 4) {
        let twin = rows
            .iter()
            .find(|t| format!("{}-no-border", t.file) == r.file)
            .map_or("—".to_string(), |t| {
                format!("[v{} {}×{}]({}.png)", t.version, t.side, t.side, t.file)
            });
        s.push_str(&format!(
            "| [`{}`]({}.png) | {} | {} | v{} | {}×{} | {} | {} B | {} B |\n",
            r.file, r.file, r.profile, r.layout, r.version, r.side, r.side, twin, r.json, r.cbor
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
