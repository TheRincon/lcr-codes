//! Golden vectors — the wire-format freeze.
//!
//! `cargo run -p xtask -- vectors > vectors/golden-v1.json`
//!
//! ## What this is for
//!
//! §15 asks for "a frozen golden vector file at v1.0 for cross-implementation checks", and
//! it is the artifact that makes two independently built decoders comparable. Without it,
//! "device A and device B agree" is an assertion; with it, it is a test either side can run
//! alone.
//!
//! It is equally a guard against *this* implementation drifting. The palette moved on
//! 2026-09-02 (D1) and `C3`'s chroma cell on 2026-09-03 (D11); both were deliberate, both
//! silently changed every symbol this crate emits, and nothing would have noticed. From
//! here, either would turn `golden_vectors_still_match` red.
//!
//! ## What is pinned
//!
//! Each vector fixes **every** encoder input — profile, version, both ECC levels,
//! compression and mask are all explicit, never left to a heuristic. That matters because
//! §8.5's mask search and §9.3's compression choice are *searches*: another implementation
//! may legitimately explore them differently and still be conformant, so a vector that let
//! them float would test the search rather than the wire format.
//!
//! A second group deliberately does the opposite. `auto-*` vectors leave version, mask and
//! compression unset and record what this encoder picked, which pins the heuristics as
//! behaviour worth keeping. Those are the ones a conformant implementation may reasonably
//! differ on; they are labelled so a mismatch there reads as a difference rather than a bug.
//!
//! ## The canonical serialisation
//!
//! A symbol is `side × side` palette indices, row-major, one byte each, quiet zone
//! excluded — which is exactly `Symbol::modules`. An index is `l·k + h` (§4.1), so it is
//! independent of the palette's actual colours: a recalibration against real panels
//! changes what the colours *are* without changing a single vector here. That is the right
//! boundary. The wire format is the arrangement of palette slots; which sRGB triples fill
//! them is a rendering concern.
//!
//! Since D19 a symbol also carries payload on the ring's inner cells, which are outside
//! that grid: `ring` lists their palette indices in `border::payload_cells` order. It is
//! part of the wire format for the same reason the grid is — two encoders that disagree
//! about it produce symbols that decode differently.
//!
//! `crc32` is CRC-32/ISO-HDLC over that byte array — change detection, not cryptography,
//! and already in `lcr-core` for the §9.1 extended frame, so no new dependency and no
//! ambiguity about which CRC-32 is meant.

use lcr_core::crc::crc32_iso_hdlc;
use lcr_core::layout::Layout;
use lcr_core::version::{EccLevel, Profile, Version};
use lcr_core::{Compression, Symbol};
use lcr_encode::EncodeOptions;
use std::fmt::Write as _;

/// Full grids are emitted below this side length. A v32 grid is 21,025 bytes of hex per
/// vector; a v4 is 1,089. Small symbols are where a cross-implementation mismatch is
/// actually debuggable by eye, and large ones are covered by the CRC.
const FULL_GRID_MAX_SIDE: u16 = 45;

/// What was handed to the encoder.
enum Input {
    Bytes(Vec<u8>),
    Text(&'static str),
    Json(&'static str),
}

struct Case {
    name: &'static str,
    input: Input,
    opts: EncodeOptions,
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn profile_name(p: Profile) -> &'static str {
    match p {
        Profile::M => "M",
        Profile::C1 => "C1",
        Profile::C2 => "C2",
        Profile::C3 => "C3",
    }
}

fn compression_name(c: Option<Compression>) -> &'static str {
    match c {
        None => "auto",
        Some(Compression::None) => "none",
        Some(Compression::Deflate) => "deflate",
        Some(Compression::Brotli) => "brotli",
        // §9.3 `cflag 11`. Rejected by the encoder today for want of a corpus to build the
        // preset dictionary from, so no vector can carry it — but naming it here means
        // adding one later is a match arm that already exists rather than a surprise.
        Some(Compression::BrotliPreset) => "brotli-preset",
    }
}

fn opts(
    profile: Profile,
    version: Option<u8>,
    ecc: u8,
    mask: Option<u8>,
    border: bool,
) -> EncodeOptions {
    EncodeOptions {
        profile,
        version: version.map(|v| Version::new(v).expect("version in range")),
        ecc: EccLevel {
            luma: ecc,
            chroma: ecc,
        },
        compression: Some(Compression::None),
        reference_border: border,
        mask,
    }
}

/// The cases. Chosen to cover every axis the format region carries, plus the payload
/// shapes that have historically broken something.
fn cases() -> Vec<Case> {
    let mut v: Vec<Case> = Vec::new();
    let body = b"LCR Code golden vector".to_vec();

    // Every profile at one size, border on and off. The border is format bit 14 and
    // changes the illumination fit's inputs, not the grid — pinning both proves that.
    for p in [Profile::M, Profile::C1, Profile::C2, Profile::C3] {
        for border in [true, false] {
            let name: &'static str = Box::leak(
                format!(
                    "profile-{}-{}",
                    profile_name(p).to_lowercase(),
                    if border { "border" } else { "plain" }
                )
                .into_boxed_str(),
            );
            v.push(Case {
                name,
                input: Input::Bytes(body.clone()),
                opts: opts(p, Some(6), 1, Some(0), border),
            });
        }
    }

    // Every ECC level. These select the §8.2 rate pair, so they exercise four different
    // LDPC code lengths over identical input.
    for ecc in 0..4u8 {
        let name: &'static str = Box::leak(format!("ecc-{ecc}").into_boxed_str());
        v.push(Case {
            name,
            input: Input::Bytes(body.clone()),
            opts: opts(Profile::C1, Some(6), ecc, Some(0), true),
        });
    }

    // Every mask. §8.5 applies these to the luma layer only, and the format region
    // carries the choice in 3 bits, so all eight must round-trip.
    for mask in 0..8u8 {
        let name: &'static str = Box::leak(format!("mask-{mask}").into_boxed_str());
        v.push(Case {
            name,
            input: Input::Bytes(body.clone()),
            opts: opts(Profile::C1, Some(4), 1, Some(mask), true),
        });
    }

    // Version sweep, including both ends of the range.
    for ver in [1u8, 2, 4, 8, 16, 32] {
        let name: &'static str = Box::leak(format!("version-{ver}").into_boxed_str());
        // v1 at C1 holds 20 bytes; keep the payload inside the smallest case.
        v.push(Case {
            name,
            input: Input::Bytes(b"lcr golden".to_vec()),
            opts: opts(Profile::C1, Some(ver), 1, Some(2), true),
        });
    }

    // Content types (§9.2). `Text` frames as UTF-8, `Json` converts to CBOR first (§9.4),
    // so these pin the container as well as the grid.
    v.push(Case {
        name: "ctype-text",
        input: Input::Text("hello, LCR Code"),
        opts: opts(Profile::C1, Some(4), 1, Some(0), true),
    });
    v.push(Case {
        name: "ctype-json-to-cbor",
        input: Input::Json(r#"{"name":"Fred","loc":"45.00,55.0"}"#),
        opts: opts(Profile::C1, Some(4), 1, Some(0), true),
    });

    // DEFLATE, forced. Compressible input, or the encoder's own choice would be no
    // compression and the vector would pin nothing.
    v.push(Case {
        name: "compression-deflate",
        input: Input::Bytes(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_vec()),
        opts: EncodeOptions {
            compression: Some(Compression::Deflate),
            ..opts(Profile::C1, Some(5), 1, Some(0), true)
        },
    });

    // Payload shapes that have broken something before.
    //
    // An all-zero body is the case that once defeated detection outright: LDPC is linear
    // and systematic, so it encodes to an all-zero codeword, and at mask 0 the rendered
    // luma layer became a perfect module-pitch checkerboard that the finder scan read as a
    // finder on every scan line. Frozen here so it can never regress quietly.
    v.push(Case {
        name: "degenerate-all-zero",
        input: Input::Bytes(vec![0u8; 32]),
        opts: opts(Profile::C1, Some(8), 1, Some(0), true),
    });
    v.push(Case {
        name: "degenerate-all-ones",
        input: Input::Bytes(vec![0xffu8; 32]),
        opts: opts(Profile::C1, Some(8), 1, Some(0), true),
    });
    v.push(Case {
        name: "empty-payload",
        input: Input::Bytes(Vec::new()),
        opts: opts(Profile::C1, Some(2), 1, Some(0), true),
    });

    // The heuristics, pinned as behaviour rather than as wire format. Version, mask and
    // compression are all left unset; what the encoder picks is recorded.
    v.push(Case {
        name: "auto-everything-short",
        input: Input::Json(r#"{"a":1}"#),
        opts: EncodeOptions {
            profile: Profile::C1,
            compression: None,
            reference_border: true,
            ..Default::default()
        },
    });
    v.push(Case {
        name: "auto-everything-long",
        input: Input::Json(
            r#"{"note":"a longer document, repeated. a longer document, repeated. a longer document, repeated."}"#,
        ),
        opts: EncodeOptions {
            profile: Profile::C1,
            compression: None,
            reference_border: true,
            ..Default::default()
        },
    });

    v
}

fn encode(case: &Case) -> Result<Symbol, lcr_core::LcrError> {
    match &case.input {
        Input::Bytes(b) => lcr_encode::encode_bytes(b, &case.opts),
        Input::Text(t) => lcr_encode::encode_text(t, &case.opts),
        Input::Json(j) => lcr_encode::encode_json(j, &case.opts),
    }
}

pub fn report() -> String {
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str("  \"format\": \"lcr-code-golden-vectors\",\n");
    s.push_str("  \"schema\": 3,\n");
    s.push_str("  \"spec\": \"v0.4\",\n");
    s.push_str(
        "  \"note\": \"Canonical form is side*side palette indices, row-major, one byte \
         each, quiet zone excluded. An index is l*k + h, so these are independent of the \
         palette's sRGB values. crc32 is CRC-32/ISO-HDLC over that array. `ring` is one palette index per ring          payload cell (D19), in border::payload_cells order, and is wire format too.          Regenerate with \
         `just vectors`; a change here is a wire-format change.\",\n",
    );
    s.push_str("  \"vectors\": [\n");

    let all = cases();
    let mut rows: Vec<String> = Vec::new();
    for case in &all {
        let sym = match encode(case) {
            Ok(sym) => sym,
            Err(e) => panic!("vector {:?} failed to encode: {e}", case.name),
        };
        let cells = sym.modules.cells();
        let crc = crc32_iso_hdlc(cells);
        let side = sym.side();

        let mut r = String::new();
        let _ = write!(r, "    {{\n      \"name\": \"{}\",\n", case.name);
        let (kind, payload) = match &case.input {
            Input::Bytes(b) => ("bytes", format!("\"{}\"", hex(b))),
            Input::Text(t) => ("text", format!("\"{}\"", json_escape(t))),
            Input::Json(j) => ("json", format!("\"{}\"", json_escape(j))),
        };
        let _ = write!(
            r,
            "      \"input\": \"{kind}\",\n      \"payload\": {payload},\n"
        );
        let _ = writeln!(
            r,
            "      \"options\": {{ \"profile\": \"{}\", \"version\": {}, \"ecc_luma\": {}, \
             \"ecc_chroma\": {}, \"mask\": {}, \"compression\": \"{}\", \
             \"reference_border\": {} }},",
            profile_name(case.opts.profile),
            case.opts
                .version
                .map_or("null".to_string(), |v| v.get().to_string()),
            case.opts.ecc.luma,
            case.opts.ecc.chroma,
            case.opts.mask.map_or("null".to_string(), |m| m.to_string()),
            compression_name(case.opts.compression),
            case.opts.reference_border,
        );
        let _ = writeln!(
            r,
            "      \"resolved\": {{ \"version\": {}, \"mask\": {}, \"side\": {} }},",
            sym.format.version.get(),
            sym.format.mask,
            side,
        );
        let _ = write!(r, "      \"crc32\": \"{crc:08x}\"");
        if side <= FULL_GRID_MAX_SIDE {
            let _ = write!(r, ",\n      \"modules\": \"{}\"", hex(cells));
        }
        if !sym.ring.is_empty() {
            let _ = write!(r, ",\n      \"ring\": \"{}\"", hex(&sym.ring));
        }
        // The format codeword is wire format too, and until schema 3 nothing here froze it:
        // it rides on the ring's *outer* cells, which are computed at render time and never
        // stored on `Symbol`, so a change to the format word — D17's, or D21's wider profile
        // field — passed this file silently.
        let cw = if Layout::is_finderless(sym.format.profile, sym.format.reference_border) {
            sym.format.to_ring_codeword()
        } else {
            sym.format.to_codeword()
        };
        let _ = write!(r, ",\n      \"format\": \"{cw:08x}\"");
        r.push_str("\n    }");
        rows.push(r);
    }

    s.push_str(&rows.join(",\n"));
    s.push_str("\n  ]\n}\n");
    s
}
