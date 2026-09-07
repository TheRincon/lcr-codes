//! Fuzz the §9.1 container and its decompressors (§15).
//!
//! `Frame::from_bits` is the first thing to touch a payload after LDPC, and at that point
//! the bits are attacker-shaped: the CRC is *inside* the frame, so the length field, the
//! content type and the compression flag are all parsed **before** anything has been
//! checked. A hostile symbol therefore gets to choose them freely.
//!
//! `multipart_decode` (§9.5) and `decompress` (§9.3) are reached the same way. Decompression
//! is the classic amplification surface — a short input that expands without bound — so
//! the bound `decompress` imposes is worth confirming against an adversary rather than
//! against the encoder's own output.

#![no_main]

use libfuzzer_sys::fuzz_target;

use lc_core::bits::BitVec;
use lc_core::frame::{Compression, Frame, decompress, multipart_decode};

fuzz_target!(|data: &[u8]| {
    // The container, parsed straight from bits. Everything before the CRC is unvalidated.
    let mut bits = BitVec::new();
    for byte in data {
        for i in (0..8).rev() {
            bits.push((byte >> i) & 1 == 1);
        }
    }
    if let Ok((frame, _consumed)) = Frame::from_bits(&bits) {
        // `payload` decompresses, which is where a declared length meets real bytes.
        let _ = frame.payload();
        // Re-encoding a parsed frame must not panic either.
        let _ = frame.to_bits();
    }

    // The decompressors directly, so they are reached even when framing rejects the input
    // before it gets that far.
    for cflag in [Compression::None, Compression::Deflate] {
        let _ = decompress(cflag, data);
    }

    // §9.5 multipart: a part count and per-part lengths, all attacker-chosen.
    let _ = multipart_decode(data);
});
