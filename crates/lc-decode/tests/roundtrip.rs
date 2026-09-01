//! End-to-end: encode → render → decode.

use lc_core::{Compression, ContentType, EccLevel, Packet, Profile, Version};
use lc_decode::{DecodeOptions, Decoder};
use lc_encode::EncodeOptions;
use lc_render::{RenderOptions, render_rgb};

fn roundtrip(data: &[u8], enc: &EncodeOptions, module_px: u32) -> Result<Packet, String> {
    let sym = lc_encode::encode_bytes(data, enc).map_err(|e| format!("encode: {e}"))?;
    let raster = render_rgb(
        &sym,
        &RenderOptions {
            module_px,
            ..RenderOptions::screen()
        },
    );
    let decoder = Decoder::new();
    decoder
        .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
        .map(|out| out.packet)
        .map_err(|e| {
            format!(
                "decode v{} {:?}: {e}",
                sym.format.version.get(),
                enc.profile
            )
        })
}

#[test]
fn clean_capture_roundtrips_every_profile() {
    let data: Vec<u8> = (0..80u8)
        .map(|i| i.wrapping_mul(37).wrapping_add(11))
        .collect();
    for profile in Profile::ALL {
        let opts = EncodeOptions {
            profile,
            compression: Some(Compression::None),
            ..Default::default()
        };
        match roundtrip(&data, &opts, 10) {
            Ok(Packet::Unsigned { body, ctype }) => {
                assert_eq!(body, data, "{profile:?}");
                assert_eq!(ctype, ContentType::Opaque);
            }
            Ok(other) => panic!("{profile:?} gave {other:?}"),
            Err(e) => panic!("{e}"),
        }
    }
}

#[test]
fn roundtrips_across_the_version_range() {
    for v in [1u8, 2, 3, 6, 10, 14, 20, 32] {
        let version = Version::new(v).unwrap();
        let cap = lc_encode::capacity(version, Profile::C1, EccLevel::DEFAULT);
        let n = cap.body_bytes.min(600);
        let data: Vec<u8> = (0..n).map(|i| (i * 17 + 5) as u8).collect();
        let opts = EncodeOptions {
            version: Some(version),
            compression: Some(Compression::None),
            ..Default::default()
        };
        match roundtrip(&data, &opts, 10) {
            Ok(Packet::Unsigned { body, .. }) => assert_eq!(body, data, "v{v}"),
            Ok(other) => panic!("v{v} gave {other:?}"),
            Err(e) => panic!("{e}"),
        }
    }
}

#[test]
fn json_roundtrips_through_cbor() {
    let src = r#"{"name":"Fred","loc":"45.00,55.0"}"#;
    let sym = lc_encode::encode_json(src, &EncodeOptions::default()).unwrap();
    let raster = render_rgb(&sym, &RenderOptions::screen());
    let out = Decoder::new()
        .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
        .unwrap();
    match out.packet {
        Packet::Unsigned { body, ctype } => {
            assert_eq!(ctype, ContentType::Cbor);
            let v = lc_core::cbor::Value::from_slice(&body).unwrap();
            assert_eq!(lc_core::json::to_json(&v), src);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn reference_border_roundtrips() {
    let data = vec![0xA5u8; 60];
    let opts = EncodeOptions {
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    assert!(matches!(
        roundtrip(&data, &opts, 10),
        Ok(Packet::Unsigned { .. })
    ));
}

#[test]
fn every_mask_roundtrips() {
    let data = vec![0u8; 100];
    for mask in 0..8u8 {
        let opts = EncodeOptions {
            mask: Some(mask),
            compression: Some(Compression::None),
            ..Default::default()
        };
        match roundtrip(&data, &opts, 10) {
            Ok(Packet::Unsigned { body, .. }) => assert_eq!(body, data, "mask {mask}"),
            Ok(other) => panic!("mask {mask} gave {other:?}"),
            Err(e) => panic!("{e}"),
        }
    }
}

#[test]
fn every_ecc_level_roundtrips() {
    let data = vec![7u8; 40];
    for level in 0..4u8 {
        let opts = EncodeOptions {
            ecc: EccLevel {
                luma: level,
                chroma: level,
            },
            compression: Some(Compression::None),
            ..Default::default()
        };
        match roundtrip(&data, &opts, 10) {
            Ok(Packet::Unsigned { body, .. }) => assert_eq!(body, data, "level {level}"),
            Ok(other) => panic!("level {level} gave {other:?}"),
            Err(e) => panic!("{e}"),
        }
    }
}

/// §1.1 goal 3 / §12.5: a monochrome sensor must still yield the luma layer. For the
/// `M` profile that is the whole packet.
#[test]
fn monochrome_capture_decodes_the_m_profile() {
    let data = vec![0x5Au8; 40];
    let opts = EncodeOptions {
        profile: Profile::M,
        compression: Some(Compression::None),
        ..Default::default()
    };
    let sym = lc_encode::encode_bytes(&data, &opts).unwrap();
    let raster = render_rgb(&sym, &RenderOptions::screen());
    let gray: Vec<u8> = raster
        .data
        .chunks_exact(3)
        .map(|p| ((54 * p[0] as u32 + 183 * p[1] as u32 + 19 * p[2] as u32) >> 8) as u8)
        .collect();
    let out = Decoder::new()
        .decode_gray(&gray, raster.width, raster.height, raster.width)
        .unwrap();
    assert!(matches!(out.packet, Packet::Unsigned { ref body, .. } if *body == data));
}

/// A colour symbol read by a monochrome sensor: chroma is gone, so only a prefix
/// survives. §12.5 says this MAY be surfaced, flagged, for an unsigned packet.
#[test]
fn monochrome_capture_of_a_colour_symbol_yields_a_partial() {
    let data = vec![0x33u8; 120];
    let opts = EncodeOptions {
        compression: Some(Compression::None),
        ..Default::default()
    };
    let sym = lc_encode::encode_bytes(&data, &opts).unwrap();
    let raster = render_rgb(&sym, &RenderOptions::screen());
    let gray: Vec<u8> = raster
        .data
        .chunks_exact(3)
        .map(|p| ((54 * p[0] as u32 + 183 * p[1] as u32 + 19 * p[2] as u32) >> 8) as u8)
        .collect();

    let strict = Decoder::new();
    assert!(
        strict
            .decode_gray(&gray, raster.width, raster.height, raster.width)
            .is_err(),
        "a partial must not be returned unless the caller opts in"
    );

    let lenient = Decoder::new().with_options(DecodeOptions {
        allow_partial: true,
        ..Default::default()
    });
    let out = lenient
        .decode_gray(&gray, raster.width, raster.height, raster.width)
        .unwrap();
    match out.packet {
        Packet::Partial { prefix } => {
            assert!(!prefix.is_empty());
            assert!(!out.diagnostics.chroma_recovered);
        }
        other => panic!("expected Partial, got {other:?}"),
    }
}

#[test]
fn diagnostics_describe_a_clean_capture() {
    let data = vec![1u8; 50];
    let opts = EncodeOptions {
        reference_border: true,
        compression: Some(Compression::None),
        ..Default::default()
    };
    let sym = lc_encode::encode_bytes(&data, &opts).unwrap();
    let raster = render_rgb(&sym, &RenderOptions::screen());
    let out = Decoder::new()
        .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
        .unwrap();
    let d = &out.diagnostics;
    assert_eq!(d.format.version, sym.format.version);
    assert_eq!(d.format.profile, sym.format.profile);
    assert_eq!(d.format.mask, sym.format.mask);
    assert!(d.format.reference_border);
    assert!(d.chroma_recovered);
    assert!(d.format_margin > 0.0);
    assert!(
        d.alignment_found,
        "v{} should have an alignment marker",
        sym.format.version.get()
    );
    assert!(
        d.estimated_snr_db > 10.0,
        "clean render SNR {}",
        d.estimated_snr_db
    );
    assert!(
        d.illum_residual_rms < 0.05,
        "residual {}",
        d.illum_residual_rms
    );
    assert_eq!(d.frames_combined, 1);
}

#[test]
fn no_symbol_is_reported_not_guessed() {
    let blank = vec![255u8; 200 * 200 * 3];
    assert!(Decoder::new().decode_rgb(&blank, 200, 200, 600).is_err());
    let noise: Vec<u8> = (0..200 * 200 * 3)
        .map(|i| (i * 2654435761u64 as usize >> 5) as u8)
        .collect();
    assert!(Decoder::new().decode_rgb(&noise, 200, 200, 600).is_err());
}

/// Degenerate payloads are the case masking exists for, and the case that broke.
///
/// LDPC is linear and systematic, so an all-zero body encodes to an all-zero codeword;
/// the rendered luma layer is then the mask pattern itself, unmodulated by any data. For
/// mask 0 that is a perfect module-pitch checkerboard — which scores *best* on the §8.5
/// penalty function (no runs, no 2×2 blocks, an exact 50/50 split) and so is what the
/// search reliably picks.
///
/// That made the checkerboard the detector's problem, and it failed it: a uniform
/// `1:1:1:1:1` run sequence satisfied the 1:1:3:1:1 ratio test, so every scan line across
/// the payload registered finders. v32 produced 5841 spurious candidates, estimated the
/// wrong version, and failed to decode at all.
#[test]
fn degenerate_payloads_roundtrip_across_versions_and_profiles() {
    for v in [1u8, 3, 8, 14, 20, 24, 28, 32] {
        let version = Version::new(v).unwrap();
        for profile in [Profile::M, Profile::C1, Profile::C2, Profile::C3] {
            let cap = lc_encode::capacity(version, profile, EccLevel::DEFAULT);
            if cap.body_bytes < 16 {
                continue;
            }
            let n = cap.body_bytes - 8;
            for (label, data) in [
                ("zeros", vec![0x00u8; n]),
                ("ones", vec![0xFFu8; n]),
                ("0x5A", vec![0x5Au8; n]),
            ] {
                let opts = EncodeOptions {
                    version: Some(version),
                    profile,
                    compression: Some(Compression::None),
                    ..Default::default()
                };
                match roundtrip(&data, &opts, 10) {
                    Ok(Packet::Unsigned { body, .. }) => {
                        assert_eq!(body, data, "v{v} {profile:?} {label}")
                    }
                    Ok(other) => panic!("v{v} {profile:?} {label} gave {other:?}"),
                    Err(e) => panic!("v{v} {profile:?} {label}: {e}"),
                }
            }
        }
    }
}

/// §13.1 point 3 makes "≥ 8 device pixels per module" normative for a conforming display
/// application, so the decoder has to work at exactly 8 (decisions D6).
///
/// It did not. `BLOCK` is also 8, so at that pitch the binarisation grid coincided with
/// the module grid, no block ever straddled a module edge, and every block was judged to
/// have no contrast. Each then inherited a threshold from an already-computed neighbour —
/// a recurrence, and a monotone one, so a single dark block dragged everything below-right
/// of it down. A v32 symbol that is 47.7% dark binarised to 3.2% dark and the finder scan
/// returned nothing at all.
///
/// Every other test renders at 10 or 12 — the §13.1 *preferred* range — so nothing
/// exercised the floor the spec actually mandates.
#[test]
fn decodes_at_the_normative_minimum_module_pitch() {
    for v in [1u8, 5, 14, 32] {
        let version = Version::new(v).unwrap();
        let cap = lc_encode::capacity(version, Profile::C1, EccLevel::DEFAULT);
        let n = cap.body_bytes.min(600);
        let data: Vec<u8> = (0..n).map(|i| (i * 17 + 5) as u8).collect();
        let opts = EncodeOptions {
            version: Some(version),
            compression: Some(Compression::None),
            ..Default::default()
        };
        for module_px in [8u32, 9, 10] {
            match roundtrip(&data, &opts, module_px) {
                Ok(Packet::Unsigned { body, .. }) => assert_eq!(body, data, "v{v} @ {module_px}px"),
                Ok(other) => panic!("v{v} @ {module_px}px gave {other:?}"),
                Err(e) => panic!("v{v} @ {module_px}px: {e}"),
            }
        }
    }
}
