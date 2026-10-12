//! §10.8's fail-closed matrix, one test per way a packet can be refused.
//!
//! > Unknown `kid`, unknown `alg`, absent `exp`, expired, or malformed COSE MUST all
//! > result in rejection, never in "decoded but unverified" being presented as success.
//!
//! Every test here asserts two things: the *specific* reason, so a scanner can tell the
//! user something useful, and `body() == None`, so no path can leak content it did not
//! verify.

use lcr_core::{Packet, VerifyFailure};
use lcr_encode::sign::{SignOptions, SigningKey, sign};
use lcr_verify::cwt::Claims;
use lcr_verify::{FixedClock, KeyResolver, KeyRing, NoClock, PublicKey, Verifier, cose, cwt};

const NOW: u64 = 1_000_000;
const LATER: u64 = 2_000_000;

fn ed_key() -> SigningKey {
    SigningKey::from_ed25519_bytes(&[7u8; 32])
}

fn p256_key() -> SigningKey {
    SigningKey::from_p256_bytes(&[
        0x4c, 0x0b, 0x1f, 0x9a, 0x21, 0x33, 0x8e, 0x5d, 0x90, 0x11, 0x77, 0xa2, 0x3c, 0x64, 0xd8,
        0x05, 0xbe, 0x42, 0x19, 0xf7, 0x6a, 0x88, 0x30, 0xcc, 0x51, 0x0e, 0xab, 0x73, 0x26, 0x95,
        0xdf, 0x11,
    ])
    .expect("fixed test scalar is a valid P-256 key")
}

fn opts() -> SignOptions {
    SignOptions {
        content_type: 42,
        exp: LATER,
        iat: NOW,
        schema: Some(7),
        iss: None,
    }
}

fn verifier_for(key: &SigningKey) -> Verifier<KeyRing, FixedClock> {
    Verifier::new(KeyRing::new().with(key.public_key()), FixedClock(NOW))
}

fn assert_refused(packet: &Packet, expected: VerifyFailure) {
    match packet {
        Packet::Unverified { reason, .. } => assert_eq!(*reason, expected, "wrong reason"),
        other => panic!("expected refusal {expected:?}, got {other:?}"),
    }
    assert_eq!(packet.body(), None, "a refusal must carry no body");
    assert!(!packet.is_attested(), "a refusal must not read as attested");
}

#[test]
fn a_good_packet_verifies() {
    for key in [ed_key(), p256_key()] {
        let cose = sign(&key, b"hello", &opts()).unwrap();
        match verifier_for(&key).verify_cose(&cose) {
            Packet::Verified {
                body,
                schema,
                exp,
                kid,
            } => {
                assert_eq!(body, b"hello");
                assert_eq!(schema, Some(7));
                assert_eq!(exp, LATER);
                assert_eq!(kid, key.kid());
            }
            other => panic!("{:?} did not verify: {other:?}", key),
        }
    }
}

#[test]
fn unknown_kid_is_refused() {
    let cose = sign(&ed_key(), b"hello", &opts()).unwrap();
    // A verifier that trusts a *different* key.
    let stranger = SigningKey::from_ed25519_bytes(&[9u8; 32]);
    let v = Verifier::new(KeyRing::new().with(stranger.public_key()), FixedClock(NOW));
    assert_refused(&v.verify_cose(&cose), VerifyFailure::UnknownKid);
}

#[test]
fn an_empty_keyring_verifies_nothing() {
    let cose = sign(&ed_key(), b"hello", &opts()).unwrap();
    let v = Verifier::new(KeyRing::new(), FixedClock(NOW));
    assert_refused(&v.verify_cose(&cose), VerifyFailure::UnknownKid);
}

#[test]
fn a_tampered_body_is_refused() {
    let key = ed_key();
    let mut cose = sign(&key, b"hello", &opts()).unwrap();
    // Flip a byte inside the payload. The `kid` still resolves, so this reaches the
    // signature check rather than being caught earlier.
    let pos = cose.iter().position(|&b| b == b'h').expect("body present");
    cose[pos] = b'H';
    assert_refused(
        &verifier_for(&key).verify_cose(&cose),
        VerifyFailure::BadSignature,
    );
}

#[test]
fn a_tampered_protected_header_is_refused() {
    let key = ed_key();
    let good = sign(&key, b"hello", &opts()).unwrap();
    // Re-assemble with a protected header claiming a different content type. The
    // signature covers the original bytes, so this must not verify.
    let parsed = cose::parse(&good).unwrap();
    let forged = cose::protected_header(key.alg(), 99, key.kid());
    let packet = cose::assemble(&forged, &parsed.payload, &parsed.signature);
    assert_refused(
        &verifier_for(&key).verify_cose(&packet),
        VerifyFailure::BadSignature,
    );
}

#[test]
fn an_expired_packet_is_refused() {
    let key = ed_key();
    let cose = sign(&key, b"hello", &opts()).unwrap();
    // A clock past `exp`.
    let v = Verifier::new(KeyRing::new().with(key.public_key()), FixedClock(LATER + 1));
    assert_refused(&v.verify_cose(&cose), VerifyFailure::Expired { exp: LATER });
}

#[test]
fn expiry_is_exclusive_at_the_boundary() {
    let key = ed_key();
    let cose = sign(&key, b"hello", &opts()).unwrap();
    let v = Verifier::new(KeyRing::new().with(key.public_key()), FixedClock(LATER));
    assert_refused(&v.verify_cose(&cose), VerifyFailure::Expired { exp: LATER });
}

#[test]
fn a_packet_without_exp_is_refused_not_treated_as_eternal() {
    let key = ed_key();
    // Hand-build claims with no `exp` and sign them properly, so the only fault is the
    // missing claim.
    let claims = Claims {
        iss: None,
        exp: None,
        iat: Some(NOW),
        schema: None,
        body: Some(b"hello".to_vec()),
    };
    let payload = cwt::build(&claims);
    let protected = cose::protected_header(key.alg(), 0, key.kid());
    let sig = key.sign(&cose::sig_structure(&protected, &payload));
    let packet = cose::assemble(&protected, &payload, &sig);
    assert_refused(
        &verifier_for(&key).verify_cose(&packet),
        VerifyFailure::MissingExpiry,
    );
}

#[test]
fn no_trusted_clock_refuses_rather_than_skipping_the_check() {
    let key = ed_key();
    let cose = sign(&key, b"hello", &opts()).unwrap();
    let v = Verifier::new(KeyRing::new().with(key.public_key()), NoClock);
    assert_refused(&v.verify_cose(&cose), VerifyFailure::NoTrustedClock);
}

#[test]
fn an_unsupported_alg_is_refused() {
    let key = ed_key();
    let claims = Claims {
        iss: None,
        exp: Some(LATER),
        iat: Some(NOW),
        schema: None,
        body: Some(b"hello".to_vec()),
    };
    let payload = cwt::build(&claims);
    // −257 is RS256. §10.3: "No RSA."
    let protected = cose::protected_header(-257, 0, key.kid());
    let sig = key.sign(&cose::sig_structure(&protected, &payload));
    let packet = cose::assemble(&protected, &payload, &sig);
    assert_refused(
        &verifier_for(&key).verify_cose(&packet),
        VerifyFailure::UnsupportedAlg,
    );
}

#[test]
fn malformed_cose_is_refused() {
    let v = verifier_for(&ed_key());
    for bad in [
        &b""[..],
        &b"not cbor at all"[..],
        &[0xD2][..],                         // tag with nothing after it
        &[0xD2, 0x84][..],                   // array header, no items
        &[0x84, 0x40, 0xA0, 0x40, 0x40][..], // untagged COSE_Sign1
    ] {
        assert_refused(&v.verify_cose(bad), VerifyFailure::MalformedCose);
    }
}

#[test]
fn trailing_bytes_after_the_packet_are_refused() {
    let key = ed_key();
    let mut cose = sign(&key, b"hello", &opts()).unwrap();
    cose.push(0x00);
    assert_refused(
        &verifier_for(&key).verify_cose(&cose),
        VerifyFailure::MalformedCose,
    );
}

/// §10.2: "Unprotected header MUST be empty. Any header affecting verification MUST be
/// protected." An unprotected header is not covered by the signature, so anything in it
/// is attacker-controlled.
#[test]
fn a_nonempty_unprotected_header_is_refused() {
    use lcr_core::cbor::Value;
    let key = ed_key();
    let good = sign(&key, b"hello", &opts()).unwrap();
    let parsed = cose::parse(&good).unwrap();
    let packet = Value::Tag(
        18,
        Box::new(Value::Array(vec![
            Value::Bytes(parsed.protected.to_vec()),
            // A stowaway `kid` in the unprotected header.
            Value::Map(vec![(Value::Uint(4), Value::Bytes(vec![1, 2, 3, 4]))]),
            Value::Bytes(parsed.payload.clone()),
            Value::Bytes(parsed.signature.clone()),
        ])),
    )
    .to_vec();
    assert_refused(
        &verifier_for(&key).verify_cose(&packet),
        VerifyFailure::MalformedCose,
    );
}

/// §10.5's one non-negotiable rule: "Verifiers MUST NOT accept a public key transported
/// in the symbol."
///
/// The attack: generate a keypair, sign arbitrary content, ship the public key alongside.
/// This must fail as `UnknownKid` — the verifier never looks in the packet for key
/// material, so the embedded key is just inert bytes in the body.
#[test]
fn a_key_carried_in_the_packet_is_never_trusted() {
    let attacker = SigningKey::from_ed25519_bytes(&[0xAB; 32]);
    let embedded = attacker.public_key().to_der();
    let cose = sign(&attacker, &embedded, &opts()).unwrap();

    // A verifier configured with an honest key, which is not the attacker's.
    let honest = ed_key();
    let v = Verifier::new(KeyRing::new().with(honest.public_key()), FixedClock(NOW));
    assert_refused(&v.verify_cose(&cose), VerifyFailure::UnknownKid);

    // And with no keys at all, so there is no chance of an accidental match.
    let v = Verifier::new(KeyRing::new(), FixedClock(NOW));
    assert_refused(&v.verify_cose(&cose), VerifyFailure::UnknownKid);
}

/// §10.5: a 4-byte `kid` collides by design, and a verifier "MUST attempt verification
/// against each and accept if any succeeds".
#[test]
fn every_key_sharing_a_kid_is_tried() {
    struct Colliding(Vec<PublicKey>);
    impl KeyResolver for Colliding {
        fn keys_for(&self, _kid: [u8; 4]) -> Vec<PublicKey> {
            self.0.clone()
        }
    }

    let signer = ed_key();
    let cose = sign(&signer, b"hello", &opts()).unwrap();

    // The real key is last, behind two decoys and a different algorithm.
    let resolver = Colliding(vec![
        SigningKey::from_ed25519_bytes(&[1u8; 32]).public_key(),
        p256_key().public_key(),
        SigningKey::from_ed25519_bytes(&[2u8; 32]).public_key(),
        signer.public_key(),
    ]);
    let v = Verifier::new(resolver, FixedClock(NOW));
    assert!(
        v.verify_cose(&cose).is_attested(),
        "a colliding kid must not become a verification failure"
    );
}

#[test]
fn a_signature_of_the_wrong_length_is_refused() {
    let key = ed_key();
    let good = sign(&key, b"hello", &opts()).unwrap();
    let parsed = cose::parse(&good).unwrap();
    for sig in [vec![], vec![0u8; 63], vec![0u8; 65], vec![0u8; 128]] {
        let packet = cose::assemble(parsed.protected, &parsed.payload, &sig);
        assert_refused(
            &verifier_for(&key).verify_cose(&packet),
            VerifyFailure::BadSignature,
        );
    }
}

#[test]
fn kid_is_four_bytes_of_sha256_over_the_der_key() {
    use sha2::{Digest, Sha256};
    for key in [ed_key(), p256_key()] {
        let der = key.public_key().to_der();
        let want = Sha256::digest(&der);
        assert_eq!(key.kid(), want[..4], "kid must follow §10.5");
    }
}

#[test]
fn the_two_algorithms_have_different_kids_and_algs() {
    let (ed, p) = (ed_key(), p256_key());
    assert_eq!(ed.alg(), -8);
    assert_eq!(p.alg(), -7);
    assert_ne!(ed.kid(), p.kid());
}

/// A key of the right algorithm but the wrong identity must not verify — guards against
/// a resolver bug where `keys_for` ignores the `kid` and returns everything.
#[test]
fn a_wrong_key_of_the_right_algorithm_is_refused() {
    struct All(Vec<PublicKey>);
    impl KeyResolver for All {
        fn keys_for(&self, _kid: [u8; 4]) -> Vec<PublicKey> {
            self.0.clone()
        }
    }
    let cose = sign(&ed_key(), b"hello", &opts()).unwrap();
    let v = Verifier::new(
        All(vec![
            SigningKey::from_ed25519_bytes(&[3u8; 32]).public_key(),
        ]),
        FixedClock(NOW),
    );
    assert_refused(&v.verify_cose(&cose), VerifyFailure::BadSignature);
}
