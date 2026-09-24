//! CFT carried over LC Code symbols.
//!
//! CFT.md §13 treats QR as "simply another transport"; this is the same argument with LC
//! Code in that slot, and it exercises the layering the document describes in §15: the
//! symbol's own LDPC and CRC protect the *representation*, and CFT's end-to-end hash
//! verifies the *reconstruction*. Two different jobs, and this test is where you can see
//! both doing them.
//!
//! Note what CFT does **not** need to know: nothing here appears in a manifest or chunk.
//! No version, no profile, no mask — §17's rule that transport concepts stay out of the
//! format.

use lc_cft::{Compression, Reassembler, Status, split};
use lc_core::{Compression as LcCompression, EccLevel, Packet, Profile, Version};
use lc_decode::Decoder;
use lc_encode::EncodeOptions;
use lc_render::{RenderOptions, render_rgb};

const VERSION: u8 = 10;
const PROFILE: Profile = Profile::C3;

/// Payload bytes per chunk, from the carrier's capacity rather than CFT.md §7's fixed
/// 4096 — which no symbol below v29 at C3 could hold.
fn chunk_size(opts: &EncodeOptions) -> u32 {
    let cap = lc_encode::capacity_for_symbol(
        Version::new(VERSION).unwrap(),
        PROFILE,
        opts.ecc,
        opts.reference_border,
    );
    // Leave room for the chunk's own CBOR wrapper: map header, transfer id, index,
    // total, and the payload's bstr header. 32 bytes is comfortable at these sizes.
    (cap.body_bytes - 32) as u32
}

/// Deterministic pseudorandom bytes.
///
/// It has to be genuinely incompressible: zstd flattens any patterned sequence into a
/// single chunk, and a one-chunk transfer cannot exercise anything this file is about.
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

fn opts() -> EncodeOptions {
    EncodeOptions {
        profile: PROFILE,
        version: Some(Version::new(VERSION).unwrap()),
        ecc: EccLevel::DEFAULT,
        // The symbol carries CBOR that is already a zstd stream, so a second pass is
        // wasted work — CFT.md §5 compresses once, before chunking.
        compression: Some(LcCompression::None),
        ..Default::default()
    }
}

#[test]
fn a_file_travels_as_a_sequence_of_symbols() {
    let opts = opts();
    let data = noise(3_000);

    let transfer = split(
        &data,
        Some("payload.bin".into()),
        b"\x01\x02\x03\x04".to_vec(),
        Compression::Zstd,
        chunk_size(&opts),
    )
    .unwrap();
    assert!(
        transfer.manifest.total_chunks >= 2,
        "want a genuinely multi-symbol transfer, got {}",
        transfer.manifest.total_chunks
    );

    // Sender: one symbol per CFT message.
    let symbols: Vec<_> = transfer
        .messages()
        .iter()
        .map(|m| lc_encode::encode_cbor(m, &opts).expect("encode"))
        .collect();

    // Receiver: scan each symbol, feed whatever comes out to the reassembler. It does
    // not need to be told which symbol was the manifest.
    let decoder = Decoder::new();
    let mut rx = Reassembler::default();
    for symbol in &symbols {
        let raster = render_rgb(
            symbol,
            &RenderOptions {
                module_px: 8,
                ..RenderOptions::screen()
            },
        );
        let out = decoder
            .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
            .expect("decode");
        let Packet::Unsigned { body, .. } = out.packet else {
            panic!("unexpected packet kind");
        };
        rx.accept(&body).expect("accept");
    }

    assert_eq!(rx.status(), Status::Complete);
    assert_eq!(rx.finish().unwrap(), data);
    assert_eq!(rx.manifest().unwrap().safe_filename(), Some("payload.bin"));
}

/// CFT.md §10 with a real carrier: skip a symbol, see exactly which one is wanted, show
/// it, finish. This is the "rescan only those" loop the document describes.
#[test]
fn a_skipped_symbol_is_named_and_can_be_rescanned() {
    let opts = opts();
    let data = noise(3_000);
    let transfer = split(
        &data,
        None,
        b"\xAA\xBB".to_vec(),
        Compression::Zstd,
        chunk_size(&opts),
    )
    .unwrap();

    let messages = transfer.messages();
    let symbols: Vec<_> = messages
        .iter()
        .map(|m| lc_encode::encode_cbor(m, &opts).expect("encode"))
        .collect();

    let decoder = Decoder::new();
    let scan = |symbol: &lc_core::Symbol| {
        let raster = render_rgb(
            symbol,
            &RenderOptions {
                module_px: 8,
                ..RenderOptions::screen()
            },
        );
        let out = decoder
            .decode_rgb(&raster.data, raster.width, raster.height, raster.width * 3)
            .expect("decode");
        match out.packet {
            Packet::Unsigned { body, .. } => body,
            other => panic!("unexpected {other:?}"),
        }
    };

    // Miss the second chunk entirely.
    let skipped = 2usize;
    let mut rx = Reassembler::default();
    for (i, symbol) in symbols.iter().enumerate() {
        if i != skipped {
            rx.accept(&scan(symbol)).unwrap();
        }
    }

    // The receiver knows precisely what it is short of — index 1, since message 0 was
    // the manifest.
    assert_eq!(rx.missing(), vec![(skipped - 1) as u32]);
    assert!(matches!(rx.status(), Status::Receiving { .. }));
    assert!(rx.finish().is_err());

    rx.accept(&scan(&symbols[skipped])).unwrap();
    assert_eq!(rx.status(), Status::Complete);
    assert_eq!(rx.finish().unwrap(), data);
}
