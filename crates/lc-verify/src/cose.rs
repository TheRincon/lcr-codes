//! COSE_Sign1 parsing and the `Sig_structure` (RFC 9052), per §10.2.
//!
//! ## Why the protected header is kept as raw bytes
//!
//! The signature covers the protected header *as it was serialised by the issuer*, not as
//! we would re-serialise it. Decoding it to a [`Value`] and encoding it again would be
//! correct only as long as both ends agree on canonical form — and a verifier that
//! re-encodes is a verifier whose accept/reject depends on its own encoder. So
//! [`Sign1::protected`] is a borrowed slice of the original bytes, and the header is
//! parsed for its *fields* separately, never for reconstruction.

use alloc::vec::Vec;
use lc_core::cbor::Value;

/// CBOR tag 18 — COSE_Sign1 (§10.2).
pub const TAG_COSE_SIGN1: u64 = 18;

/// Protected header label for `alg`.
const LABEL_ALG: i64 = 1;
/// Protected header label for `content type`.
const LABEL_CTY: i64 = 3;
/// Protected header label for `kid`.
const LABEL_KID: i64 = 4;

/// A parsed COSE_Sign1, with the protected header borrowed from the input.
#[derive(Clone, Debug)]
pub struct Sign1<'a> {
    /// The protected header's serialised bytes, exactly as received.
    pub protected: &'a [u8],
    /// `alg` from the protected header (§10.3).
    pub alg: i64,
    /// `kid` from the protected header — a 4-byte hint (§10.5).
    pub kid: [u8; 4],
    /// `content type` from the protected header. Not optional: §10.2 marks it REQUIRED
    /// and [`parse`] rejects a header without it, so there is no `None` to represent.
    pub content_type: u64,
    /// The payload bstr: a CWT claims map (§10.2).
    pub payload: Vec<u8>,
    /// The signature bstr.
    pub signature: Vec<u8>,
}

/// Parse a tagged COSE_Sign1.
///
/// Returns `None` for anything that is not exactly the structure §10.2 describes. Every
/// rejection here surfaces as `VerifyFailure::MalformedCose`, so being strict costs
/// nothing but a retry, while being lenient costs the property that a packet has one
/// valid representation.
pub fn parse(bytes: &[u8]) -> Option<Sign1<'_>> {
    let (value, consumed) = Value::from_prefix(bytes).ok()?;
    // Trailing bytes after a complete item mean the encoder and we disagree about what
    // was signed. Refuse rather than guess.
    if consumed != bytes.len() {
        return None;
    }

    // §10.2 names tag 18. RFC 9052 permits untagged where the type is known from
    // context, but `ctype = 0x4` already costs us nothing and the tag is in §10.4's
    // size budget, so require it and keep one representation.
    let Value::Tag(TAG_COSE_SIGN1, inner) = value else {
        return None;
    };
    let Value::Array(items) = *inner else {
        return None;
    };
    let [protected, unprotected, payload, signature] = items.as_slice() else {
        return None;
    };

    // §10.2: "Unprotected header MUST be empty. Any header affecting verification MUST
    // be protected." An attacker-supplied unprotected header is not covered by the
    // signature, so the only safe size for it is zero.
    match unprotected {
        Value::Map(m) if m.is_empty() => {}
        _ => return None,
    }

    let Value::Bytes(protected_bytes) = protected else {
        return None;
    };
    let Value::Bytes(payload_bytes) = payload else {
        return None;
    };
    let Value::Bytes(signature_bytes) = signature else {
        return None;
    };

    // Locate the protected header inside the input so it can be borrowed rather than
    // re-encoded. Tag 18 and a 4-element array each occupy exactly one byte under the
    // canonical form the CBOR decoder enforces, so the bstr starts at index 2; its
    // contents are the tail of that item. The equality check below makes the assumption
    // self-validating — if the offset were ever wrong we fail closed rather than verify
    // the wrong bytes.
    const PROTECTED_OFFSET: usize = 2;
    let (_, item_len) = Value::from_prefix(bytes.get(PROTECTED_OFFSET..)?).ok()?;
    let start = PROTECTED_OFFSET + item_len.checked_sub(protected_bytes.len())?;
    let protected_slice = bytes.get(start..start + protected_bytes.len())?;
    if protected_slice != protected_bytes.as_slice() {
        return None;
    }

    let header = Value::from_slice(protected_slice).ok()?;
    let map = header.as_map()?;
    // A repeated label would let two readers disagree about `alg` or `kid`.
    if has_duplicate_labels(map) {
        return None;
    }

    let alg = header.get_int_key(LABEL_ALG)?.as_i64()?;
    let kid_bytes = header.get_int_key(LABEL_KID)?.as_bytes()?;
    let kid = <[u8; 4]>::try_from(kid_bytes).ok()?;
    // §10.2 marks `content type` REQUIRED.
    let Value::Uint(content_type) = *header.get_int_key(LABEL_CTY)? else {
        return None;
    };

    Some(Sign1 {
        protected: protected_slice,
        alg,
        kid,
        content_type,
        payload: payload_bytes.clone(),
        signature: signature_bytes.clone(),
    })
}

/// Build the `Sig_structure` whose serialisation is what actually gets signed.
///
/// RFC 9052 §4.4:
///
/// ```text
/// Sig_structure = [ "Signature1", body_protected : bstr, external_aad : bstr, payload : bstr ]
/// ```
///
/// `external_aad` is always empty here — LC Code has no out-of-band context to bind, and
/// a verifier and issuer that disagreed about it would fail in a way neither could
/// diagnose.
pub fn sig_structure(protected: &[u8], payload: &[u8]) -> Vec<u8> {
    Value::Array(alloc::vec![
        Value::Text(alloc::string::String::from("Signature1")),
        Value::Bytes(protected.to_vec()),
        Value::Bytes(Vec::new()),
        Value::Bytes(payload.to_vec()),
    ])
    .to_vec()
}

/// Build a protected header for the given fields, in canonical label order.
pub fn protected_header(alg: i64, content_type: u64, kid: [u8; 4]) -> Vec<u8> {
    Value::Map(alloc::vec![
        (Value::from_i64(LABEL_ALG), Value::from_i64(alg)),
        (Value::from_i64(LABEL_CTY), Value::Uint(content_type)),
        (Value::from_i64(LABEL_KID), Value::Bytes(kid.to_vec())),
    ])
    .to_vec()
}

/// Assemble a tagged COSE_Sign1 from its parts.
pub fn assemble(protected: &[u8], payload: &[u8], signature: &[u8]) -> Vec<u8> {
    Value::Tag(
        TAG_COSE_SIGN1,
        alloc::boxed::Box::new(Value::Array(alloc::vec![
            Value::Bytes(protected.to_vec()),
            Value::Map(Vec::new()),
            Value::Bytes(payload.to_vec()),
            Value::Bytes(signature.to_vec()),
        ])),
    )
    .to_vec()
}

fn has_duplicate_labels(map: &[(Value, Value)]) -> bool {
    for i in 0..map.len() {
        for j in (i + 1)..map.len() {
            if map[i].0 == map[j].0 {
                return true;
            }
        }
    }
    false
}
