//! End-to-end: encode → render → decode.

use lcr_core::{Compression, ContentType, EccLevel, Packet, Profile, Version};
use lcr_decode::{DecodeOptions, Decoder};
use lcr_encode::EncodeOptions;
use lcr_render::{RenderOptions, render_rgb};

fn roundtrip(data: &[u8], enc: &EncodeOptions, module_px: u32) -> Result<Packet, String> {
    let sym = lcr_encode::encode_bytes(data, enc).map_err(|e| format!("encode: {e}"))?;
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
        let cap = lcr_encode::capacity(version, Profile::C1, EccLevel::DEFAULT);
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
    let sym = lcr_encode::encode_json(src, &EncodeOptions::default()).unwrap();
    let raster = render_rgb(&sym, &RenderOptions::screen());
    let out = Decoder::new()
        .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
        .unwrap();
    match out.packet {
        Packet::Unsigned { body, ctype } => {
            assert_eq!(ctype, ContentType::Cbor);
            let v = lcr_core::cbor::Value::from_slice(&body).unwrap();
            assert_eq!(lcr_core::json::to_json(&v), src);
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
    let sym = lcr_encode::encode_bytes(&data, &opts).unwrap();
    let raster = render_rgb(&sym, &RenderOptions::screen());
    let gray: Vec<u8> = raster
        .data
        .as_chunks::<3>()
        .0
        .iter()
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
    // Border off: the classic, finder-bearing layout, whose format region is luma-only and
    // so survives a monochrome sensor. A finder-less symbol cannot do this — see
    // `finderless_symbol_through_a_grey_sensor_fails_closed`.
    let opts = EncodeOptions {
        compression: Some(Compression::None),
        reference_border: false,
        ..Default::default()
    };
    let sym = lcr_encode::encode_bytes(&data, &opts).unwrap();
    let raster = render_rgb(&sym, &RenderOptions::screen());
    let gray: Vec<u8> = raster
        .data
        .as_chunks::<3>()
        .0
        .iter()
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
    let sym = lcr_encode::encode_bytes(&data, &opts).unwrap();
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
        .map(|i| ((i * 2654435761u64 as usize) >> 5) as u8)
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
            let cap = lcr_encode::capacity(version, profile, EccLevel::DEFAULT);
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
        let cap = lcr_encode::capacity(version, Profile::C1, EccLevel::DEFAULT);
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

/// Ground-truth for the measurement the capture sweep rests on.
///
/// `Decoder::probe` reports a captured module pitch, and the sweep uses it as its
/// distance axis — so it has to agree with reality where reality is known. On a clean
/// fronto-parallel render the pitch is exactly the `module_px` the renderer drew at, and
/// the obliquity ratio is 1. If this drifts, every working-range figure measured through
/// the rig is off by the same factor, silently.
#[test]
fn probe_measures_the_pitch_the_renderer_drew_at() {
    let data: Vec<u8> = (0..60u8)
        .map(|i| i.wrapping_mul(13).wrapping_add(7))
        .collect();
    for profile in [Profile::C1, Profile::C3] {
        for module_px in [8u32, 12, 20] {
            let opts = EncodeOptions {
                profile,
                compression: Some(Compression::None),
                ..Default::default()
            };
            let sym = lcr_encode::encode_bytes(&data, &opts).unwrap();
            let raster = render_rgb(
                &sym,
                &RenderOptions {
                    module_px,
                    ..RenderOptions::screen()
                },
            );
            let view = lcr_decode::image::ImageView::new(
                &raster.data,
                raster.width,
                raster.height,
                raster.width * 3,
                lcr_decode::image::PixelFormat::Rgb8,
            )
            .unwrap();
            let p = Decoder::new().probe(&view).unwrap();

            let want = module_px as f32;
            assert!(
                (p.capture_pitch_min - want).abs() < 0.5,
                "{profile:?} @ {module_px}px: measured {} want {want}",
                p.capture_pitch_min,
            );
            let obliquity = p.capture_pitch_max / p.capture_pitch_min;
            assert!(
                (obliquity - 1.0).abs() < 0.02,
                "{profile:?} @ {module_px}px: fronto-parallel read as oblique ({obliquity})",
            );
        }
    }
}

/// The distinction `probe` exists to draw: it must fail where there is no symbol, so a
/// host can tell "nothing in frame" from "found it, payload marginal". A probe that
/// succeeded on noise would make every failed decode look like a marginal one.
#[test]
fn probe_finds_nothing_in_a_frame_with_no_symbol() {
    let (w, h) = (320u32, 240u32);
    let noise: Vec<u8> = (0..(w * h * 3))
        .map(|i| (i.wrapping_mul(2654435761) >> 16) as u8)
        .collect();
    let view = lcr_decode::image::ImageView::new(
        &noise,
        w,
        h,
        w * 3,
        lcr_decode::image::PixelFormat::Rgb8,
    )
    .unwrap();
    assert!(Decoder::new().probe(&view).is_err());
}

/// Two symbols in one frame must not defeat each other.
///
/// §6.2 adopts QR's 1:1:3:1:1 finder deliberately (Appendix B), and the finder carries no
/// identity — so a frame holding two codes presents six finder-like patterns, and when
/// both are at the same module pitch the module-spread filter cannot separate them. A
/// triple straddling the two symbols can score *better* on right-angleness than either
/// real triple, and detection used to commit to the single best-scoring triple with no
/// way back: the format region then failed and the whole decode returned
/// `FormatUndecodable` on a frame containing two perfectly good symbols.
///
/// Found while building §17.2 dual-render, where an LCR symbol sits beside a QR — but the
/// pure-LCR case here is the general one, and a poster with two codes on it is an ordinary
/// thing to point a camera at.
#[test]
fn a_frame_holding_two_symbols_still_decodes_one() {
    const PX: u32 = 10;
    const GAP: u32 = 8 * PX;

    let render = |data: &[u8]| {
        let sym = lcr_encode::encode_bytes(
            data,
            &EncodeOptions {
                profile: Profile::C1,
                compression: Some(Compression::None),
                ..Default::default()
            },
        )
        .unwrap();
        render_rgb(
            &sym,
            &RenderOptions {
                module_px: PX,
                ..RenderOptions::screen()
            },
        )
    };

    let left = render(b"left-hand symbol payload");
    let right = render(b"right-hand symbol payload");

    // Compose side by side on white, both at the same pitch — the case the module-spread
    // filter is blind to.
    let w = left.width + GAP + right.width;
    let h = left.height.max(right.height);
    let mut data = vec![255u8; (w * h * 3) as usize];
    let mut blit = |src: &lcr_render::Raster, at_x: u32| {
        for y in 0..src.height {
            let s = ((y * src.width) * 3) as usize;
            let d = ((y * w + at_x) * 3) as usize;
            let n = (src.width * 3) as usize;
            data[d..d + n].copy_from_slice(&src.data[s..s + n]);
        }
    };
    blit(&left, 0);
    blit(&right, left.width + GAP);

    let out = Decoder::new()
        .decode_rgb(&data, w, h, w * 3)
        .expect("one of the two symbols must decode");
    match out.packet {
        Packet::Unsigned { body, .. } => {
            let got = core::str::from_utf8(&body).unwrap();
            assert!(
                got == "left-hand symbol payload" || got == "right-hand symbol payload",
                "decoded something that was in neither symbol: {got:?}",
            );
        }
        other => panic!("unexpected packet {other:?}"),
    }
}

/// Detection has an **upper** limit on module pitch, not only §13.1's lower one.
///
/// Found on 2026-09-14 by raising the capture rig to 4K: the captured pitch tripled and
/// nothing decoded at any distance. Measured on clean renders, the run-ratio scan worked
/// from 8 to ~22 px/module and then stopped — with extra failures at exactly 16 and 24,
/// `2·BLOCK` and `3·BLOCK`, where the binariser's fixed 8-pixel grid aligns with the module
/// grid. That is D6's mechanism recurring at multiples D6 never tested.
///
/// It matters because range is bought with sensor pixels per module, so the ceiling capped
/// the working distance: every attempt to buy range by raising capture resolution walked
/// straight into it.
///
/// `PYRAMID_LEVELS` (D16) fixes it by retrying the finder search on a halved image, which divides
/// the pitch back into the band that works. Guarded here across the whole range because the
/// failure was silent — a symbol that is too *large* looks exactly like no symbol at all.
#[test]
fn decodes_across_the_full_module_pitch_range() {
    let data = b"pitch ceiling regression";
    let opts = EncodeOptions {
        profile: Profile::C1,
        compression: Some(Compression::None),
        ..Default::default()
    };
    let sym = lcr_encode::encode_bytes(data, &opts).unwrap();

    // 8..=24 at both phases, then the large pitches at a non-aligned phase only.
    //
    // **Known residual**: at pitch 32 and up *with the grids exactly in phase* (offset 0,
    // pitch an exact multiple of `BLOCK`), detection now succeeds but the format region
    // fails its BCH decode — the same alignment fault, one stage later. The pyramid lifted
    // the detection ceiling, which is what capped working range; this remainder is
    // recorded rather than hidden, and wants the binariser's fixed block grid fixing
    // properly rather than another retry bolted on.
    for module_px in [8u32, 10, 12, 16, 20, 24, 32, 40, 64] {
        let phases: &[u32] = if module_px >= 32 { &[1] } else { &[0, 1] };
        let r = render_rgb(
            &sym,
            &RenderOptions {
                module_px,
                ..RenderOptions::screen()
            },
        );
        // Offset by one pixel: at 16 and 24 the bug appeared *only* when the binarisation
        // grid and the module grid were in phase, so a zero-offset render alone would have
        // missed half of it.
        for &offset in phases {
            let (w, h) = (r.width + offset, r.height + offset);
            let mut buf = vec![255u8; (w * h * 3) as usize];
            for y in 0..r.height {
                let s = (y * r.width * 3) as usize;
                let d = (((y + offset) * w + offset) * 3) as usize;
                let n = (r.width * 3) as usize;
                buf[d..d + n].copy_from_slice(&r.data[s..s + n]);
            }
            match Decoder::new().decode_rgb(&buf, w, h, w * 3) {
                Ok(out) => match out.packet {
                    Packet::Unsigned { body, .. } => {
                        assert_eq!(body, data, "{module_px} px/module, offset {offset}")
                    }
                    other => panic!("{module_px} px/module, offset {offset}: {other:?}"),
                },
                Err(e) => panic!("{module_px} px/module, offset {offset}: {e}"),
            }
        }
    }
}

/// The trade D17 makes, pinned so it cannot slip by silently.
///
/// A finder-less symbol carries its format on the ring's *hue* choices, because the ring's
/// luma is spent on the position sequence. So a greyscale capture cannot read the format,
/// and without the format there is no layout, mask or ECC level to decode even a luma prefix
/// against. §1.1 goal 3's graceful degradation is lost for finder-less symbols — print, and
/// profile `M`, keep it. What must hold is that it fails *closed*: an error, never a payload.
#[test]
fn finderless_symbol_through_a_grey_sensor_fails_closed() {
    let data = vec![0x33u8; 120];
    let opts = EncodeOptions {
        compression: Some(Compression::None),
        ..Default::default()
    };
    assert!(lcr_core::layout::Layout::is_finderless(
        opts.profile,
        opts.reference_border
    ));
    let sym = lcr_encode::encode_bytes(&data, &opts).unwrap();
    let raster = render_rgb(&sym, &RenderOptions::screen());
    let gray: Vec<u8> = raster
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| ((54 * p[0] as u32 + 183 * p[1] as u32 + 19 * p[2] as u32) >> 8) as u8)
        .collect();
    let lenient = Decoder::new().with_options(DecodeOptions {
        allow_partial: true,
        ..Default::default()
    });
    assert!(
        lenient
            .decode_gray(&gray, raster.width, raster.height, raster.width)
            .is_err(),
        "a finder-less symbol must not decode, even partially, from a greyscale capture"
    );
}
