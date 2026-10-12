//! End-to-end: sign → encode → render → decode → verify.
//!
//! The fail-closed matrix in `fail_closed.rs` works on COSE bytes directly. This file
//! puts a signed packet through an actual symbol, because that is the path that can
//! break in ways COSE-level tests cannot see — the §6.1 version floor, the §10.4 size
//! budget, and the `ctype = 0x4` dispatch in `lcr-decode`.

use lcr_core::{Compression, Packet, Profile, VerifyFailure};
use lcr_decode::Decoder;
use lcr_encode::EncodeOptions;
use lcr_encode::sign::{SIGNED_VERSION_FLOOR, SignOptions, SigningKey, encode_signed, sign};
use lcr_render::{RenderOptions, render_rgb};
use lcr_verify::{FixedClock, KeyRing, Verifier};

const NOW: u64 = 1_700_000_000;
const TTL: u64 = 3600;

fn keys() -> [SigningKey; 2] {
    [
        SigningKey::from_ed25519_bytes(&[0x11; 32]),
        SigningKey::from_p256_bytes(&[
            0x4c, 0x0b, 0x1f, 0x9a, 0x21, 0x33, 0x8e, 0x5d, 0x90, 0x11, 0x77, 0xa2, 0x3c, 0x64,
            0xd8, 0x05, 0xbe, 0x42, 0x19, 0xf7, 0x6a, 0x88, 0x30, 0xcc, 0x51, 0x0e, 0xab, 0x73,
            0x26, 0x95, 0xdf, 0x11,
        ])
        .unwrap(),
    ]
}

fn enc() -> EncodeOptions {
    EncodeOptions {
        profile: Profile::C3,
        compression: Some(Compression::None),
        ..Default::default()
    }
}

fn capture(symbol: &lcr_core::Symbol) -> (Vec<u8>, u32, u32) {
    let raster = render_rgb(
        symbol,
        &RenderOptions {
            module_px: 10,
            ..RenderOptions::screen()
        },
    );
    (raster.data, raster.width, raster.height)
}

#[test]
fn a_signed_symbol_survives_a_clean_capture() {
    for key in keys() {
        let body = b"attendee 4417, gate B";
        let symbol =
            encode_signed(&key, body, &SignOptions::expiring_in(NOW, TTL), &enc()).unwrap();
        let (data, w, h) = capture(&symbol);

        let verifier = Verifier::new(KeyRing::new().with(key.public_key()), FixedClock(NOW));
        let out = Decoder::new()
            .with_verifier(verifier)
            .decode_rgb(&data, w, h, w * 3)
            .expect("decode");

        match &out.packet {
            Packet::Verified {
                body: got,
                exp,
                kid,
                ..
            } => {
                assert_eq!(got, body);
                assert_eq!(*exp, NOW + TTL);
                assert_eq!(*kid, key.kid());
            }
            other => panic!("{key:?}: {other:?}"),
        }
        assert!(out.packet.is_attested());
    }
}

/// §1.2: a decoder built without verification is fully conformant. It must say so
/// rather than pretending the packet is fine.
#[test]
fn a_decoder_without_keys_says_so_and_yields_no_body() {
    let key = &keys()[0];
    let symbol = encode_signed(key, b"hello", &SignOptions::expiring_in(NOW, TTL), &enc()).unwrap();
    let (data, w, h) = capture(&symbol);

    let out = Decoder::new()
        .decode_rgb(&data, w, h, w * 3)
        .expect("decode");
    match &out.packet {
        Packet::Unverified { reason, .. } => {
            assert_eq!(*reason, VerifyFailure::NoKeysConfigured);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(out.packet.body(), None);
    assert!(!out.packet.is_attested());
}

/// An expired packet must still fail after a real capture — the decode succeeding says
/// nothing about whether the packet should be trusted.
#[test]
fn an_expired_packet_fails_after_a_clean_decode() {
    let key = &keys()[0];
    let symbol = encode_signed(key, b"hello", &SignOptions::expiring_in(NOW, TTL), &enc()).unwrap();
    let (data, w, h) = capture(&symbol);

    let verifier = Verifier::new(
        KeyRing::new().with(key.public_key()),
        FixedClock(NOW + TTL + 1),
    );
    let out = Decoder::new()
        .with_verifier(verifier)
        .decode_rgb(&data, w, h, w * 3)
        .expect("decode");

    assert_eq!(
        out.packet,
        Packet::Unverified {
            reason: VerifyFailure::Expired { exp: NOW + TTL },
            kid: key.kid(),
        }
    );
}

/// §6.1: "Encoders SHOULD select v5 or above when signing."
#[test]
fn the_version_floor_applies_when_signing() {
    let key = &keys()[0];
    let symbol = encode_signed(key, b"x", &SignOptions::expiring_in(NOW, TTL), &enc()).unwrap();
    assert!(
        symbol.format.version.get() >= SIGNED_VERSION_FLOOR,
        "signed symbol came out at v{}",
        symbol.format.version.get()
    );
}

/// The same clause: "and MUST NOT apply that floor to unsigned payloads."
#[test]
fn the_version_floor_does_not_apply_to_unsigned_payloads() {
    let symbol = lcr_encode::encode_bytes(b"x", &enc()).unwrap();
    assert!(
        symbol.format.version.get() < SIGNED_VERSION_FLOOR,
        "the floor leaked into an unsigned symbol"
    );
}

/// The floor is a minimum, not a pin: a body too large for v5 must still get a bigger
/// symbol rather than failing.
#[test]
fn the_floor_does_not_cap_larger_signed_payloads() {
    let key = &keys()[0];
    let body = vec![0xA5u8; 400];
    let symbol = encode_signed(key, &body, &SignOptions::expiring_in(NOW, TTL), &enc()).unwrap();
    assert!(
        symbol.format.version.get() > SIGNED_VERSION_FLOOR,
        "a 400-byte body should need more than v{SIGNED_VERSION_FLOOR}"
    );

    let (data, w, h) = capture(&symbol);
    let verifier = Verifier::new(KeyRing::new().with(key.public_key()), FixedClock(NOW));
    let out = Decoder::new()
        .with_verifier(verifier)
        .decode_rgb(&data, w, h, w * 3)
        .expect("decode");
    assert_eq!(out.packet.body(), Some(&body[..]));
}

/// §10.4 budgets the signed envelope at ~107 bytes, of which 64 is irreducible
/// signature. Measure it rather than trusting the table — if the envelope grows, the
/// v5 floor and the capacity guidance both stop being right.
#[test]
fn the_envelope_matches_the_size_budget() {
    for key in keys() {
        let cose = sign(&key, b"", &SignOptions::expiring_in(NOW, TTL)).unwrap();
        let overhead = cose.len();
        // Printed so a drift shows up in `--nocapture` before it trips the bound.
        println!("{key:?} empty-body envelope: {overhead} bytes");
        assert!(
            (95..=115).contains(&overhead),
            "{key:?} envelope is {overhead} bytes; §10.4 budgets ~103 before the frame"
        );
        // The signature alone must be 64 of those.
        assert!(overhead > 64);
    }
}

/// Signed and unsigned must be distinguishable states (§10.7), all the way out to what
/// a scanner sees.
#[test]
fn signed_and_unsigned_are_distinguishable() {
    let key = &keys()[0];
    let signed = encode_signed(key, b"hello", &SignOptions::expiring_in(NOW, TTL), &enc()).unwrap();
    let unsigned = lcr_encode::encode_bytes(b"hello", &enc()).unwrap();

    let verifier = Verifier::new(KeyRing::new().with(key.public_key()), FixedClock(NOW));
    let decoder = Decoder::new().with_verifier(verifier);

    let (d, w, h) = capture(&signed);
    let a = decoder.decode_rgb(&d, w, h, w * 3).unwrap().packet;
    let (d, w, h) = capture(&unsigned);
    let b = decoder.decode_rgb(&d, w, h, w * 3).unwrap().packet;

    assert!(a.is_attested());
    assert!(
        !b.is_attested(),
        "unsigned content must never read as attested"
    );
    assert_eq!(a.body(), b.body(), "same body, different attestation state");
}
