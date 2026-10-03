//! CFT end to end, plus the cases `docs/CFT.md`'s gaps would have let through.

use lc_cft::chunk::{Message, parse};
use lc_cft::{CftError, Chunk, Compression, Limits, Manifest, Reassembler, Status, split};

const ID: &[u8] = b"\x8f\x13\xa9\x2c";

fn file() -> Vec<u8> {
    // Compressible, so the codec actually does something worth checking.
    (0..40_000u32).map(|i| (i / 97) as u8).collect()
}

fn deliver(t: &lc_cft::Transfer, order: impl Iterator<Item = usize>) -> Reassembler {
    let mut r = Reassembler::default();
    let msgs = t.messages();
    for i in order {
        r.accept(&msgs[i]).expect("accept");
    }
    r
}

#[test]
fn a_file_survives_a_round_trip() {
    let data = file();
    let t = split(
        &data,
        Some("example.pdf".into()),
        ID.to_vec(),
        Compression::Zstd,
        4096,
    )
    .unwrap();

    assert!(
        t.manifest.compressed_size < t.manifest.original_size,
        "zstd did nothing"
    );
    let r = deliver(&t, 0..t.message_count());
    assert_eq!(r.status(), Status::Complete);
    assert_eq!(r.finish().unwrap(), data);
}

/// CFT.md §9: "The receiver can therefore accept chunks in any order."
#[test]
fn chunks_may_arrive_in_any_order() {
    let data = file();
    let t = split(&data, None, ID.to_vec(), Compression::Zstd, 1024).unwrap();
    let n = t.message_count();

    // Reverse, which also puts every chunk ahead of the manifest.
    let r = deliver(&t, (0..n).rev());
    assert_eq!(r.status(), Status::Complete);
    assert_eq!(r.finish().unwrap(), data);
}

/// Chunks arriving before the manifest must be held, not dropped — the manifest is one
/// symbol among many and there is no reason it is scanned first.
#[test]
fn chunks_before_the_manifest_are_held_and_replayed() {
    let data = file();
    let t = split(&data, None, ID.to_vec(), Compression::Zstd, 2048).unwrap();
    let msgs = t.messages();

    let mut r = Reassembler::default();
    for m in &msgs[1..] {
        r.accept(m).unwrap();
    }
    assert!(matches!(r.status(), Status::AwaitingManifest { held } if held == msgs.len() - 1));
    r.accept(&msgs[0]).unwrap();
    assert_eq!(r.status(), Status::Complete);
    assert_eq!(r.finish().unwrap(), data);
}

/// CFT.md §10: the receiver tracks what is outstanding so the sender can re-show it.
#[test]
fn missing_chunks_are_reported() {
    let data = file();
    // Stored rather than compressed: `file()` is compressible enough that zstd leaves
    // it in a single chunk, and a one-chunk transfer cannot demonstrate a gap.
    let t = split(&data, None, ID.to_vec(), Compression::None, 4096).unwrap();
    let total = t.manifest.total_chunks;
    assert!(total > 3, "need a few chunks for this to mean anything");

    let mut r = Reassembler::default();
    r.accept(&t.manifest.to_vec()).unwrap();
    for (i, c) in t.chunks.iter().enumerate() {
        if i != 1 && i != 2 {
            r.accept(&c.to_vec()).unwrap();
        }
    }
    assert_eq!(r.missing(), vec![1, 2]);
    assert_eq!(
        r.status(),
        Status::Receiving {
            have: total - 2,
            total
        }
    );
    assert_eq!(r.finish(), Err(CftError::Incomplete));

    r.accept(&t.chunks[1].to_vec()).unwrap();
    r.accept(&t.chunks[2].to_vec()).unwrap();
    assert!(r.missing().is_empty());
    assert_eq!(r.finish().unwrap(), data);
}

/// A repeated chunk is normal on a QR transport; a *contradictory* one is not.
#[test]
fn a_duplicate_chunk_is_fine_but_a_conflicting_one_is_not() {
    let data = file();
    // Stored, so the stream is genuinely larger than one chunk.
    let t = split(&data, None, ID.to_vec(), Compression::None, 4096).unwrap();
    let mut r = Reassembler::default();
    r.accept(&t.manifest.to_vec()).unwrap();
    r.accept(&t.chunks[0].to_vec()).unwrap();
    r.accept(&t.chunks[0].to_vec()).unwrap();

    let mut forged = t.chunks[0].clone();
    forged.payload[0] ^= 0xFF;
    assert_eq!(r.accept(&forged.to_vec()), Err(CftError::ChunkConflict(0)));
}

/// CFT.md §11/§12: the hash is what makes reconstruction trustworthy, and it covers the
/// original file, so corruption anywhere in the pipeline surfaces here.
#[test]
fn a_corrupted_payload_fails_the_end_to_end_hash() {
    let data = file();
    let t = split(&data, None, ID.to_vec(), Compression::None, 4096).unwrap();

    let mut r = Reassembler::default();
    r.accept(&t.manifest.to_vec()).unwrap();
    for (i, c) in t.chunks.iter().enumerate() {
        let mut c = c.clone();
        if i == 2 {
            c.payload[10] ^= 0x01; // same length, so only the hash can catch it
        }
        r.accept(&c.to_vec()).unwrap();
    }
    assert_eq!(r.status(), Status::Complete);
    assert_eq!(r.finish(), Err(CftError::HashMismatch));
}

// ---------------------------------------------------------------- the gaps

/// **[CFT.md §3/§6]** The document gives no way to tell a manifest from a chunk. The rule
/// implemented here is that key 0 is text in one and bytes in the other.
#[test]
fn a_manifest_and_a_chunk_are_never_confused() {
    // chunk_size below the stream length, so this really does produce chunks rather
    // than an inline manifest.
    let t = split(b"hello", None, ID.to_vec(), Compression::None, 2).unwrap();
    let limits = Limits::default();
    assert!(!t.chunks.is_empty());

    assert!(matches!(
        parse(&t.manifest.to_vec(), &limits).unwrap(),
        Message::Manifest(_)
    ));
    assert!(matches!(
        parse(&t.chunks[0].to_vec(), &limits).unwrap(),
        Message::Chunk(_)
    ));
    // Parsing one as the other must fail rather than produce nonsense.
    assert_eq!(
        Chunk::from_slice(&t.manifest.to_vec(), &limits),
        Err(CftError::Indeterminate)
    );
    assert_eq!(
        Manifest::from_slice(&t.chunks[0].to_vec(), &limits),
        Err(CftError::Indeterminate)
    );
}

/// **[CFT.md §3]** The document's manifest has no transfer ID, so nothing binds it to its
/// chunks. With one, a chunk from another transfer is refused.
#[test]
fn a_chunk_from_another_transfer_is_refused() {
    let a = split(b"aaaaaaaa", None, b"AAAA".to_vec(), Compression::None, 4).unwrap();
    let b = split(b"bbbbbbbb", None, b"BBBB".to_vec(), Compression::None, 4).unwrap();

    let mut r = Reassembler::default();
    r.accept(&a.manifest.to_vec()).unwrap();
    assert_eq!(
        r.accept(&b.chunks[0].to_vec()),
        Err(CftError::WrongTransfer)
    );
}

/// **[CFT.md §6]** The chunk count appears in the manifest and in every chunk. The
/// manifest wins; a chunk claiming otherwise is not part of this transfer.
#[test]
fn a_chunk_disagreeing_about_the_total_is_refused() {
    let t = split(b"0123456789abcdef", None, ID.to_vec(), Compression::None, 4).unwrap();
    let mut r = Reassembler::default();
    r.accept(&t.manifest.to_vec()).unwrap();

    let mut lying = t.chunks[0].clone();
    lying.total += 1;
    assert_eq!(
        r.accept(&lying.to_vec()),
        Err(CftError::ChunkOutOfRange {
            index: 0,
            total: t.manifest.total_chunks
        })
    );
}

/// A manifest whose declared count disagrees with its own sizes is self-contradictory.
#[test]
fn a_self_inconsistent_manifest_is_refused() {
    let t = split(b"0123456789abcdef", None, ID.to_vec(), Compression::None, 4).unwrap();
    let mut m = t.manifest.clone();
    m.total_chunks += 3;
    assert!(matches!(
        Manifest::from_slice(&m.to_vec(), &Limits::default()),
        Err(CftError::BadField(9))
    ));
}

/// CFT.md §4: reject versions we do not understand rather than guessing.
#[test]
fn an_unknown_version_is_refused() {
    use lc_core::cbor::Value;
    let bytes = Value::Map(vec![(Value::Uint(0), Value::Text("CFT9".into()))]).to_vec();
    assert_eq!(
        Manifest::from_slice(&bytes, &Limits::default()),
        Err(CftError::UnknownVersion("CFT9".into()))
    );
}

/// **Not in CFT.md.** A filename arrives from the sender and the document never says what
/// a receiver may do with it.
#[test]
fn hostile_filenames_are_not_offered_as_filenames() {
    for bad in [
        "../../etc/passwd",
        "/etc/passwd",
        "..",
        ".",
        "",
        "a/b",
        "a\\b",
        "-rf",
        "bad\0name",
        "bad\nname",
    ] {
        let t = split(b"x", Some(bad.into()), ID.to_vec(), Compression::None, 8).unwrap();
        assert_eq!(t.manifest.safe_filename(), None, "accepted {bad:?}");
    }
    let t = split(
        b"x",
        Some("report.pdf".into()),
        ID.to_vec(),
        Compression::None,
        8,
    )
    .unwrap();
    assert_eq!(t.manifest.safe_filename(), Some("report.pdf"));
}

/// **Not in CFT.md.** Declared sizes are attacker-controlled u64s.
#[test]
fn an_oversized_manifest_is_refused_before_anything_is_allocated() {
    let t = split(b"x", None, ID.to_vec(), Compression::None, 8).unwrap();
    let mut m = t.manifest.clone();
    m.original_size = u64::MAX;
    let limits = Limits::default();
    assert!(matches!(
        Manifest::from_slice(&m.to_vec(), &limits),
        Err(CftError::TooLarge { .. })
    ));

    let mut m = t.manifest.clone();
    m.compressed_size = 1 << 40;
    assert!(matches!(
        Manifest::from_slice(&m.to_vec(), &limits),
        Err(CftError::TooLarge { .. })
    ));
}

/// A truncated chunk has to be caught where it happens, not as a hash failure after the
/// whole transfer has been collected.
#[test]
fn a_short_chunk_is_refused_on_arrival() {
    let t = split(b"0123456789abcdef", None, ID.to_vec(), Compression::None, 4).unwrap();
    let mut r = Reassembler::default();
    r.accept(&t.manifest.to_vec()).unwrap();

    let mut short = t.chunks[0].clone();
    short.payload.pop();
    assert_eq!(r.accept(&short.to_vec()), Err(CftError::ChunkLength(0)));
}

/// A decompression bomb must be refused by the declared-size ceiling rather than
/// allocated and then noticed.
#[test]
fn a_decompression_bomb_is_bounded_by_the_declared_size() {
    let big = vec![0u8; 8 << 20];
    let stream = Compression::Zstd.compress(&big).unwrap();
    assert!(stream.len() < 4096, "zeros should compress hard");

    // The manifest lies about how big the original was.
    assert!(matches!(
        Compression::Zstd.decompress(&stream, 1024),
        Err(CftError::SizeMismatch { .. })
    ));
    // Told the truth, it decompresses.
    assert_eq!(
        Compression::Zstd
            .decompress(&stream, big.len() as u64)
            .unwrap()
            .len(),
        big.len()
    );
}

#[test]
fn an_empty_file_round_trips() {
    let t = split(
        b"",
        Some("empty.bin".into()),
        ID.to_vec(),
        Compression::Zstd,
        64,
    )
    .unwrap();
    assert_eq!(t.manifest.total_chunks, 1);
    let r = deliver(&t, 0..t.message_count());
    assert_eq!(r.finish().unwrap(), b"");
}

#[test]
fn every_codec_round_trips() {
    let data = file();
    for c in [Compression::None, Compression::Deflate, Compression::Zstd] {
        if !c.available() {
            continue;
        }
        let t = split(&data, None, ID.to_vec(), c, 4096).unwrap();
        let r = deliver(&t, 0..t.message_count());
        assert_eq!(r.finish().unwrap(), data, "{c:?}");
    }
}

/// A manifest that names a codec this build lacks should say so, rather than failing
/// later with something unhelpful.
#[test]
fn an_unavailable_codec_is_named() {
    let t = split(b"hello", None, ID.to_vec(), Compression::None, 8).unwrap();
    let mut m = t.manifest.clone();
    m.compression = Compression::Brotli;
    let parsed = Manifest::from_slice(&m.to_vec(), &Limits::default()).unwrap();
    let mut r = Reassembler::default();
    if !Compression::Brotli.available() {
        assert_eq!(
            r.accept_manifest(parsed),
            Err(CftError::Unsupported(Compression::Brotli.to_u8()))
        );
    }
}

/// Garbage must not be read as either kind of message.
#[test]
fn garbage_is_refused() {
    let limits = Limits::default();
    for bad in [
        &b""[..],
        &b"not cbor"[..],
        &[0xA0][..],
        &[0x82, 0x01, 0x02][..],
    ] {
        assert!(parse(bad, &limits).is_err(), "accepted {bad:?}");
    }
}

/// A file larger than one LC Code *frame* must still transfer.
///
/// Regression: `codec.rs` reused `lc_core::frame::decompress`, whose output cap is
/// `MAX_PAYLOAD` (65,532) — the most a single symbol can hold. Borrowing it silently
/// limited every CFT transfer to 64 KB, and the earlier round-trip tests all used files
/// under that, so nothing caught it until a real PDF came through.
#[test]
fn a_file_larger_than_one_frame_round_trips() {
    // Compressible, so the *stream* stays small while the original is well over the cap.
    let data: Vec<u8> = (0..200_000u32).map(|i| (i / 31) as u8).collect();
    assert!(
        data.len() > 65_532,
        "must exceed MAX_PAYLOAD to be the regression"
    );

    for c in [Compression::None, Compression::Deflate, Compression::Zstd] {
        if !c.available() {
            continue;
        }
        let t = split(&data, None, ID.to_vec(), c, 2048).unwrap();
        let r = deliver(&t, 0..t.message_count());
        assert_eq!(r.finish().unwrap(), data, "{c:?}");
    }
}

/// A stream that fits beside the metadata travels as **one** message, and still carries
/// the filename and the end-to-end hash.
///
/// The alternative — sending raw bytes with no manifest — costs exactly the two things
/// CFT exists to provide, which is how "received.bin" happens.
#[test]
fn a_small_file_is_one_self_contained_message() {
    let data = b"%PDF-1.4 a small document that compresses well ".repeat(40);
    let t = split(
        &data,
        Some("invoice.pdf".into()),
        ID.to_vec(),
        Compression::Zstd,
        4096,
    )
    .unwrap();

    assert_eq!(t.message_count(), 1, "should be a single message");
    assert!(t.chunks.is_empty());
    assert_eq!(t.manifest.total_chunks, 1);
    assert!(t.manifest.inline.is_some());

    let mut r = Reassembler::default();
    let st = r.accept(&t.messages()[0]).unwrap();
    assert_eq!(st, Status::Complete, "one message completes the transfer");
    assert_eq!(r.manifest().unwrap().safe_filename(), Some("invoice.pdf"));
    assert_eq!(r.finish().unwrap(), data);
}

/// An inline manifest still verifies: corrupt the stream and the hash must catch it.
#[test]
fn an_inline_transfer_is_still_hash_checked() {
    let data = b"the quick brown fox ".repeat(20);
    let t = split(&data, None, ID.to_vec(), Compression::None, 4096).unwrap();
    let mut m = t.manifest.clone();
    m.inline.as_mut().unwrap()[5] ^= 0xFF;

    let mut r = Reassembler::default();
    r.accept(&m.to_vec()).unwrap();
    assert_eq!(r.finish(), Err(CftError::HashMismatch));
}

/// A manifest claiming inline data that disagrees with its own `compressed_size`, or
/// that claims more than one chunk, is self-contradictory.
#[test]
fn an_inconsistent_inline_manifest_is_refused() {
    let t = split(b"hello world", None, ID.to_vec(), Compression::None, 4096).unwrap();

    let mut m = t.manifest.clone();
    m.inline.as_mut().unwrap().push(0);
    assert!(matches!(
        Manifest::from_slice(&m.to_vec(), &Limits::default()),
        Err(CftError::BadField(10))
    ));
}
