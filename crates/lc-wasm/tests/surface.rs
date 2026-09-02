//! The component's logic layer, exercised natively.
//!
//! `lc-wasm`'s logic is ordinary Rust with no binding types in any signature, so it tests
//! on the host without a wasm runtime. What this does *not* cover is the generated
//! Component Model glue in `component.rs` — canonical ABI lifting and lowering, and the
//! `accumulator` resource's lifetime. Those need a runtime host.
//!
//! The split is deliberate and is why the error paths are testable at all: the previous
//! `wasm-bindgen` surface returned `JsError`, which panics when constructed outside a JS
//! engine, so every failure branch could only be reached from a browser.

use lc_core::{Compression, ContentType, Profile};
use lc_decode::image::PixelFormat;
use lc_encode::EncodeOptions as CoreEncodeOptions;
use lc_render::{Raster, RenderOptions, render_rgb};
use lc_wasm::{DecodedPacket, EncodeOptions, WasmError};

const JSON: &str = r#"{"name":"Fred","loc":"45.00,55.0"}"#;

fn render(json: &str, profile: Profile) -> Raster {
    let opts = CoreEncodeOptions {
        profile,
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    let sym = lc_encode::encode_json(json, &opts).unwrap();
    render_rgb(
        &sym,
        &RenderOptions {
            module_px: 10,
            ..RenderOptions::screen()
        },
    )
}

fn to_rgba(r: &Raster) -> Vec<u8> {
    let mut out = Vec::with_capacity((r.width * r.height * 4) as usize);
    for px in r.data.as_chunks::<3>().0 {
        out.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    out
}

fn to_gray(r: &Raster) -> Vec<u8> {
    r.data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| ((54 * p[0] as u32 + 183 * p[1] as u32 + 19 * p[2] as u32) >> 8) as u8)
        .collect()
}

// ---------------------------------------------------------------- decode

#[test]
fn decodes_rgba_as_a_host_would_supply_it() {
    let r = render(JSON, Profile::C1);
    let rgba = to_rgba(&r);
    let out =
        lc_wasm::decode(&rgba, r.width, r.height, r.width * 4, PixelFormat::Rgba8).expect("decode");

    match &out.packet {
        DecodedPacket::Unsigned { ctype, .. } => assert_eq!(*ctype, ContentType::Cbor),
        other => panic!("{other:?}"),
    }
    // §9.4's boundary in reverse: the host wants JSON back out.
    assert_eq!(out.packet.as_json().as_deref(), Some(JSON));
}

#[test]
fn decodes_rgb() {
    let r = render(JSON, Profile::C1);
    let out = lc_wasm::decode(&r.data, r.width, r.height, r.width * 3, PixelFormat::Rgb8)
        .expect("decode");
    assert_eq!(out.packet.as_json().as_deref(), Some(JSON));
}

/// §1.1 goal 3: the luma layer is the whole payload for `M`, so a mono sensor reads it
/// completely rather than partially.
#[test]
fn a_monochrome_symbol_decodes_through_a_grey_sensor() {
    let m = render(JSON, Profile::M);
    let out = lc_wasm::decode(&to_gray(&m), m.width, m.height, m.width, PixelFormat::Gray8)
        .expect("mono decode");
    assert_eq!(out.packet.as_json().as_deref(), Some(JSON));
}

/// The chroma layer is unrecoverable, so the CRC cannot pass. §9.1 is explicit that LDPC
/// convergence alone is not decode success — this must not report a result.
#[test]
fn a_colour_symbol_through_a_grey_sensor_fails_closed() {
    let c = render(JSON, Profile::C1);
    let out = lc_wasm::decode(&to_gray(&c), c.width, c.height, c.width, PixelFormat::Gray8);
    assert!(
        out.is_err(),
        "a C1 symbol read through grey must not report success"
    );
}

#[test]
fn diagnostics_describe_a_clean_capture() {
    let r = render(JSON, Profile::C1);
    let out = lc_wasm::decode(&r.data, r.width, r.height, r.width * 3, PixelFormat::Rgb8).unwrap();
    let d = &out.diagnostics;

    assert_eq!(d.version, 2, "the §6.1 worked example is a v2 symbol");
    assert_eq!(d.profile, Profile::C1);
    assert!(d.chroma_recovered);
    assert!(d.alignment_found);
    assert!(d.snr_db > 10.0, "clean render SNR {}", d.snr_db);
    assert!(d.illum_residual_rms < 0.05);
    assert_eq!(d.frames_combined, 1);
}

#[test]
fn absent_and_malformed_input_are_errors_not_panics() {
    let px = |d: &[u8], w: u32, h: u32| lc_wasm::decode(d, w, h, w * 4, PixelFormat::Rgba8);

    // Blank frame: no symbol to find.
    assert!(matches!(
        px(&vec![255u8; 200 * 200 * 4], 200, 200),
        Err(WasmError::Lc(_))
    ));
    // Buffer smaller than the stated geometry.
    assert!(matches!(px(&[0u8; 16], 100, 100), Err(WasmError::Lc(_))));
    // Zero dimensions.
    assert!(matches!(px(&[0u8; 4], 0, 0), Err(WasmError::Lc(_))));
}

/// The §10.8 guarantee has to survive the type mapping: a variant with no payload cannot
/// be talked into yielding one.
#[test]
fn unverified_and_partial_expose_no_body() {
    assert_eq!(DecodedPacket::Unverified.body(), None);
    assert_eq!(DecodedPacket::Unverified.as_json(), None);

    let partial = DecodedPacket::Partial {
        prefix: vec![1, 2, 3],
    };
    assert_eq!(partial.body(), None, "a prefix is a diagnostic, not a body");
    assert_eq!(partial.as_json(), None);
}

#[test]
fn as_json_answers_only_for_cbor() {
    let text = DecodedPacket::Unsigned {
        ctype: ContentType::Text,
        body: b"hello".to_vec(),
    };
    assert_eq!(text.as_json(), None);
    assert_eq!(text.body(), Some(&b"hello"[..]));
}

// ---------------------------------------------------------------- accumulator

/// §12.4's multi-frame LLR combining, which the spec calls "the single best use of a live
/// camera preview".
#[test]
fn accumulator_combines_frames() {
    let r = render(JSON, Profile::C1);
    let rgba = to_rgba(&r);

    let mut acc = lc_wasm::Accumulator::new();
    assert_eq!(acc.frames(), 0);

    for _ in 0..3 {
        acc.push(&rgba, r.width, r.height, r.width * 4, PixelFormat::Rgba8)
            .expect("push");
    }
    assert_eq!(acc.frames(), 3);

    let out = acc.try_decode().expect("combined decode");
    assert_eq!(out.packet.as_json().as_deref(), Some(JSON));

    acc.reset();
    assert_eq!(acc.frames(), 0);
}

#[test]
fn accumulator_with_no_frames_is_an_error_not_a_panic() {
    let acc = lc_wasm::Accumulator::new();
    assert_eq!(acc.try_decode().err(), Some(WasmError::NoFrames));
}

// ---------------------------------------------------------------- encode

#[test]
fn encodes_json_and_decodes_it_back() {
    let out = lc_wasm::encode_json(JSON, &EncodeOptions::default(), 10).expect("encode");
    assert_eq!(out.version, 2);
    assert_eq!(out.side, 25);
    assert_eq!(out.pixels.len(), (out.width * out.height * 3) as usize);

    let back = lc_wasm::decode(
        &out.pixels,
        out.width,
        out.height,
        out.width * 3,
        PixelFormat::Rgb8,
    )
    .expect("round-trip");
    assert_eq!(back.packet.as_json().as_deref(), Some(JSON));
}

#[test]
fn encodes_text_and_bytes() {
    let opts = EncodeOptions::default();

    let t = lc_wasm::encode_text("hello, world", &opts, 10).expect("text");
    let back =
        lc_wasm::decode(&t.pixels, t.width, t.height, t.width * 3, PixelFormat::Rgb8).unwrap();
    assert_eq!(back.packet.body(), Some(&b"hello, world"[..]));

    let data: Vec<u8> = (0..64u8).collect();
    let b = lc_wasm::encode_bytes(&data, &opts, 10).expect("bytes");
    let back =
        lc_wasm::decode(&b.pixels, b.width, b.height, b.width * 3, PixelFormat::Rgb8).unwrap();
    assert_eq!(back.packet.body(), Some(&data[..]));
}

#[test]
fn encodes_every_profile() {
    for profile in [Profile::M, Profile::C1, Profile::C2, Profile::C3] {
        let opts = EncodeOptions {
            profile,
            ..Default::default()
        };
        let out =
            lc_wasm::encode_json(JSON, &opts, 10).unwrap_or_else(|e| panic!("{profile:?}: {e}"));
        let back = lc_wasm::decode(
            &out.pixels,
            out.width,
            out.height,
            out.width * 3,
            PixelFormat::Rgb8,
        )
        .unwrap_or_else(|e| panic!("{profile:?} round-trip: {e}"));
        assert_eq!(back.packet.as_json().as_deref(), Some(JSON), "{profile:?}");
    }
}

#[test]
fn encodes_svg() {
    let svg = lc_wasm::encode_json_svg(JSON, &EncodeOptions::default(), 10).expect("svg");
    assert!(svg.starts_with("<svg"), "{}", &svg[..40.min(svg.len())]);
    assert!(svg.contains("</svg>"));
}

/// §13.1 point 3 makes 8 device pixels per module normative, and D6 records that the
/// decoder genuinely works at exactly 8 now. Below it, refusing beats emitting a symbol
/// no conforming reader is expected to read.
#[test]
fn refuses_a_module_pitch_below_the_normative_floor() {
    let opts = EncodeOptions::default();
    for px in [0u32, 1, 7] {
        assert_eq!(
            lc_wasm::encode_json(JSON, &opts, px).err(),
            Some(WasmError::ModulePitchTooSmall(px))
        );
    }
    assert!(
        lc_wasm::encode_json(JSON, &opts, 8).is_ok(),
        "8 is the floor, not below it"
    );
}

#[test]
fn rejects_options_outside_their_ranges() {
    let bad_version = EncodeOptions {
        version: Some(99),
        ..Default::default()
    };
    assert!(lc_wasm::encode_json(JSON, &bad_version, 10).is_err());

    let bad_ecc = EncodeOptions {
        ecc_luma: 9,
        ..Default::default()
    };
    assert!(lc_wasm::encode_json(JSON, &bad_ecc, 10).is_err());
}

#[test]
fn reports_its_version() {
    assert_eq!(lc_wasm::version(), env!("CARGO_PKG_VERSION"));
}
