//! Every version at full capacity, both ECC extremes. Sampling a few versions is not
//! enough: the extended range is new and the LDPC lift, interleaver and container
//! framing all change shape with size.

use lc_core::{Compression, EccLevel, Packet, Profile, Version};
use lc_decode::Decoder;
use lc_encode::EncodeOptions;
use lc_render::{RenderOptions, render_rgb};

fn attempt(v: u8, ecc: EccLevel) -> Result<usize, String> {
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
    let data: Vec<u8> = (0..cap.body_bytes).map(|i| (i * 31 + 7) as u8).collect();
    let sym = lc_encode::encode_bytes(&data, &opts).map_err(|e| format!("encode: {e}"))?;
    let r = render_rgb(
        &sym,
        &RenderOptions {
            module_px: 8,
            ..RenderOptions::screen()
        },
    );
    let out = Decoder::new()
        .decode_rgb(&r.data, r.width, r.height, r.width * 3)
        .map_err(|e| format!("decode: {e}"))?;
    match out.packet {
        Packet::Unsigned { body, .. } if body == data => Ok(data.len()),
        Packet::Unsigned { .. } => Err("payload mismatch".into()),
        other => Err(format!("{other:?}")),
    }
}

#[test]
fn every_version_round_trips_at_full_capacity() {
    let mut bad = Vec::new();
    for ecc in [EccLevel::new(0, 0).unwrap(), EccLevel::DEFAULT] {
        for v in 1..=Version::MAX.get() {
            if let Err(e) = attempt(v, ecc) {
                println!("  FAIL v{v} ecc {}/{}: {e}", ecc.luma, ecc.chroma);
                bad.push((v, ecc.luma, e));
            }
        }
    }
    assert!(bad.is_empty(), "{} version(s) failed: {bad:?}", bad.len());
}
