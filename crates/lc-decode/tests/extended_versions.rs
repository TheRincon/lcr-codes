//! Versions above 32 — ring-only, because §7's format word has five version bits.
//!
//! D21 moved the version out of the format word and into the §6.4 ring's de Bruijn
//! position sequence, which a decoder reads from the geometry. That removed the only
//! thing tying the symbol to a 32-version ceiling, so the range is now bounded by
//! `Version::MAX` and, above roughly v58, by §9.1's 14-bit body length instead.

use lc_core::{Compression, EccLevel, Packet, Profile, Version};
use lc_decode::Decoder;
use lc_encode::EncodeOptions;
use lc_render::{RenderOptions, render_rgb};

fn noise(n: usize) -> Vec<u8> {
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 24) as u8
        })
        .collect()
}

fn roundtrip(v: u8, ecc: EccLevel) -> (usize, usize) {
    let version = Version::new(v).unwrap();
    let opts = EncodeOptions {
        profile: Profile::C3,
        version: Some(version),
        ecc,
        compression: Some(Compression::None),
        reference_border: true,
        mask: None,
    };
    let cap = lc_encode::capacity_for_symbol(version, Profile::C3, ecc, true);
    let data = noise(cap.body_bytes);

    let sym = lc_encode::encode_bytes(&data, &opts).expect("encode");
    let raster = render_rgb(
        &sym,
        &RenderOptions {
            module_px: 8,
            ..RenderOptions::screen()
        },
    );
    let out = Decoder::new()
        .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
        .unwrap_or_else(|e| panic!("v{v} decode: {e}"));
    match out.packet {
        Packet::Unsigned { body, .. } => {
            assert_eq!(body, data, "v{v} payload mismatch");
            assert_eq!(
                out.diagnostics.format.version.get(),
                v,
                "v{v} reported the wrong version"
            );
        }
        other => panic!("v{v}: {other:?}"),
    }
    (version.side() as usize, data.len())
}

#[test]
fn extended_versions_round_trip_at_full_capacity() {
    for v in [33u8, 40, 48, 56, 64] {
        let (side, bytes) = roundtrip(v, EccLevel::DEFAULT);
        println!("  v{v:<3} {side}x{side} modules — {bytes} bytes");
    }
}

/// The headline: the most a single symbol can carry.
#[test]
fn the_largest_single_symbol() {
    let ecc = EccLevel::new(0, 0).unwrap();
    let (side, bytes) = roundtrip(64, ecc);
    println!("  v64 {side}x{side} — {bytes} bytes in one symbol");
    assert!(
        bytes >= 16_000,
        "expected the §9.1 container cap to bind, got {bytes}"
    );
}

/// Above `MAX_CLASSIC` the classic format word cannot express the version, so the ring
/// is not optional. Silently truncating would produce a *valid* codeword for a different
/// symbol, which is the one failure mode this field must never have.
#[test]
#[should_panic(expected = "needs the §6.4 ring")]
fn a_classic_symbol_cannot_signal_an_extended_version() {
    let fmt = lc_core::FormatInfo {
        version: Version::new(40).unwrap(),
        profile: Profile::C3,
        ecc: EccLevel::DEFAULT,
        mask: 0,
        reference_border: false,
    };
    let _ = fmt.to_codeword();
}
