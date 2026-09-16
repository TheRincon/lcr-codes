//! The wire-format freeze (§15).
//!
//! `vectors/golden-v1.json` pins what this encoder emits. This test re-encodes every
//! vector and compares, so a change to the palette, the layout, the mask search, the LDPC
//! tables, the container or the interleaver turns red here instead of silently altering
//! every symbol the project produces.
//!
//! That is not hypothetical. The palette changed on 2026-09-02 (D1) and `C3`'s chroma cell
//! on 2026-09-03 (D11). Both were deliberate and both were correct, but both rewrote every
//! symbol in the repository and nothing failed. From here, either would fail this.
//!
//! **If this test fails, that is a wire-format change.** Decide whether it was intended.
//! If it was, `just vectors` regenerates the file and the diff is the record of what moved.
//! If it was not, the diff names what broke.

use lc_core::cbor::Value;
use lc_core::crc::crc32_iso_hdlc;
use lc_core::version::{EccLevel, Profile, Version};
use lc_core::{Compression, LcError, Symbol};
use lc_encode::EncodeOptions;

const VECTORS: &str = include_str!("../../../vectors/golden-v1.json");

fn field<'a>(map: &'a Value, key: &str) -> Option<&'a Value> {
    match map {
        Value::Map(entries) => entries.iter().find_map(|(k, v)| match k {
            Value::Text(t) if t == key => Some(v),
            _ => None,
        }),
        _ => None,
    }
}

fn text<'a>(map: &'a Value, key: &str) -> &'a str {
    match field(map, key) {
        Some(Value::Text(t)) => t,
        other => panic!("expected text at {key}, got {other:?}"),
    }
}

fn uint(map: &Value, key: &str) -> Option<u64> {
    match field(map, key) {
        Some(Value::Uint(n)) => Some(*n),
        Some(Value::Null) | None => None,
        other => panic!("expected uint or null at {key}, got {other:?}"),
    }
}

fn boolean(map: &Value, key: &str) -> bool {
    match field(map, key) {
        Some(Value::Bool(b)) => *b,
        other => panic!("expected bool at {key}, got {other:?}"),
    }
}

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "hex string of odd length");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
        .collect()
}

fn profile(name: &str) -> Profile {
    match name {
        "M" => Profile::M,
        "C1" => Profile::C1,
        "C2" => Profile::C2,
        "C3" => Profile::C3,
        other => panic!("unknown profile {other}"),
    }
}

fn compression(name: &str) -> Option<Compression> {
    match name {
        "auto" => None,
        "none" => Some(Compression::None),
        "deflate" => Some(Compression::Deflate),
        "brotli" => Some(Compression::Brotli),
        "brotli-preset" => Some(Compression::BrotliPreset),
        other => panic!("unknown compression {other}"),
    }
}

fn encode_vector(v: &Value) -> Result<Symbol, LcError> {
    let o = field(v, "options").expect("options");
    let opts = EncodeOptions {
        profile: profile(text(o, "profile")),
        version: uint(o, "version").map(|n| Version::new(n as u8).expect("version in range")),
        ecc: EccLevel {
            luma: uint(o, "ecc_luma").expect("ecc_luma") as u8,
            chroma: uint(o, "ecc_chroma").expect("ecc_chroma") as u8,
        },
        compression: compression(text(o, "compression")),
        reference_border: boolean(o, "reference_border"),
        mask: uint(o, "mask").map(|n| n as u8),
    };

    let payload = text(v, "payload");
    match text(v, "input") {
        "bytes" => lc_encode::encode_bytes(&unhex(payload), &opts),
        "text" => lc_encode::encode_text(payload, &opts),
        "json" => lc_encode::encode_json(payload, &opts),
        other => panic!("unknown input kind {other}"),
    }
}

#[test]
fn golden_vectors_still_match() {
    let doc = lc_core::json::parse(VECTORS).expect("vector file parses");
    assert_eq!(uint(&doc, "schema"), Some(2), "unexpected vector schema");

    let vectors = match field(&doc, "vectors") {
        Some(Value::Array(v)) => v,
        other => panic!("expected a vectors array, got {other:?}"),
    };
    assert!(!vectors.is_empty(), "vector file is empty");

    let mut checked = 0usize;
    let mut drifted: Vec<String> = Vec::new();

    for v in vectors {
        let name = text(v, "name");
        let sym = match encode_vector(v) {
            Ok(s) => s,
            Err(e) => {
                drifted.push(format!("{name}: no longer encodes at all ({e})"));
                continue;
            }
        };
        let cells = sym.modules.cells();

        // The resolved fields matter as much as the grid: a vector that still produces the
        // right CRC from a different version or mask would mean two bugs cancelling.
        let resolved = field(v, "resolved").expect("resolved");
        let want_version = uint(resolved, "version").expect("version") as u8;
        let want_mask = uint(resolved, "mask").expect("mask") as u8;
        let want_side = uint(resolved, "side").expect("side") as u16;

        if sym.format.version.get() != want_version {
            drifted.push(format!(
                "{name}: version {} -> {}",
                want_version,
                sym.format.version.get()
            ));
        }
        if sym.format.mask != want_mask {
            drifted.push(format!("{name}: mask {want_mask} -> {}", sym.format.mask));
        }
        if sym.side() != want_side {
            drifted.push(format!("{name}: side {want_side} -> {}", sym.side()));
        }

        let want_crc = u32::from_str_radix(text(v, "crc32"), 16).expect("crc hex");
        let got_crc = crc32_iso_hdlc(cells);
        if got_crc != want_crc {
            // Where the full grid is present, say *where* it diverged. A single differing
            // module points at masking or mapping; a wholesale difference points at the
            // palette, the ECC tables or the container.
            let detail = match field(v, "modules") {
                Some(Value::Text(hexed)) => {
                    let want = unhex(hexed);
                    if want.len() != cells.len() {
                        format!(" ({} modules -> {})", want.len(), cells.len())
                    } else {
                        let diffs: Vec<usize> =
                            (0..want.len()).filter(|&i| want[i] != cells[i]).collect();
                        let side = sym.side() as usize;
                        let first = diffs
                            .first()
                            .map(|&i| format!(" first at ({}, {})", i % side, i / side))
                            .unwrap_or_default();
                        format!(
                            " ({} of {} modules differ,{first})",
                            diffs.len(),
                            want.len()
                        )
                    }
                }
                _ => String::new(),
            };
            drifted.push(format!(
                "{name}: crc {want_crc:08x} -> {got_crc:08x}{detail}"
            ));
        }

        // The ring's payload cells are wire format too (D19): a symbol whose grid matches
        // but whose ring does not is a different symbol.
        match field(v, "ring") {
            Some(Value::Text(hexed)) => {
                let want = unhex(hexed);
                if want.len() != sym.ring.len() {
                    drifted.push(format!(
                        "{name}: ring {} -> {} cells",
                        want.len(),
                        sym.ring.len()
                    ));
                } else if want != sym.ring {
                    let n = (0..want.len()).filter(|&i| want[i] != sym.ring[i]).count();
                    drifted.push(format!("{name}: {n} of {} ring cells differ", want.len()));
                }
            }
            _ if !sym.ring.is_empty() => {
                drifted.push(format!("{name}: gained {} ring cells", sym.ring.len()));
            }
            _ => {}
        }
        checked += 1;
    }

    assert!(
        drifted.is_empty(),
        "the wire format changed in {} of {} vectors.\n\n{}\n\n\
         If this was intended, `just vectors` regenerates vectors/golden-v1.json and the \
         diff is the record of what moved. If it was not, the list above names what broke.",
        drifted.len(),
        vectors.len(),
        drifted.join("\n"),
    );
    assert_eq!(checked, vectors.len());
}

/// The file has to cover the axes it claims to, or it freezes less than it appears to.
#[test]
fn golden_vectors_cover_the_wire_format() {
    let doc = lc_core::json::parse(VECTORS).expect("vector file parses");
    let vectors = match field(&doc, "vectors") {
        Some(Value::Array(v)) => v,
        _ => panic!("expected a vectors array"),
    };

    let opt_of = |v: &Value, k: &str| -> String {
        let o = field(v, "options").expect("options");
        text(o, k).to_string()
    };

    for p in ["M", "C1", "C2", "C3"] {
        assert!(
            vectors.iter().any(|v| opt_of(v, "profile") == p),
            "no vector covers profile {p}"
        );
    }
    for mask in 0..8u64 {
        assert!(
            vectors.iter().any(|v| {
                let o = field(v, "options").expect("options");
                uint(o, "mask") == Some(mask)
            }),
            "no vector covers mask {mask}"
        );
    }
    for ecc in 0..4u64 {
        assert!(
            vectors.iter().any(|v| {
                let o = field(v, "options").expect("options");
                uint(o, "ecc_luma") == Some(ecc)
            }),
            "no vector covers ECC level {ecc}"
        );
    }
    // Both ends of §6.1's version range.
    for want in [1u64, 32] {
        assert!(
            vectors.iter().any(|v| {
                let r = field(v, "resolved").expect("resolved");
                uint(r, "version") == Some(want)
            }),
            "no vector covers version {want}"
        );
    }
}
