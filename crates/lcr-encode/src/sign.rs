//! Issuing signed packets (§10).
//!
//! The counterpart to `lcr-verify`, and deliberately built on that crate's serialisers:
//! [`lcr_verify::cose::sig_structure`] and [`lcr_verify::cwt::build`] are the *same* code
//! the verifier uses, so the two ends cannot drift into signing and checking different
//! bytes. A signing path with its own copy of `Sig_structure` is a signing path that
//! works until someone fixes a field order on one side only.
//!
//! ## Signing needs no randomness
//!
//! Both algorithms here are deterministic — Ed25519 by construction, ECDSA P-256 via
//! RFC 6979 as RustCrypto implements it. That matters beyond taste: §14.2 makes
//! `getrandom` appearing in the wasm dependency graph a *build failure*, on the grounds
//! that it means a signing path has leaked into a scanner bundle. Keeping signing
//! RNG-free means this crate can never be the cause.
//!
//! ## Keys
//!
//! [`SigningKey`] holds private key material and is `Debug`-opaque. LCR Code takes no
//! position on where issuer keys live (§10.5); if they must live in a hardware keystore,
//! that keystore signs and this module is not involved — use [`cose_sign1_from_parts`]
//! with a signature produced elsewhere.

use crate::{EncodeOptions, Symbol};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use lcr_core::{ContentType, LcrError, Version};
use lcr_verify::cwt::Claims;
use lcr_verify::{PublicKey, cose, cwt};

/// The smallest version §6.1 recommends for a signed packet.
///
/// > Encoders **SHOULD** select v5 or above when signing and **MUST NOT** apply that
/// > floor to unsigned payloads. — §6.1
///
/// This is a consequence of the ~107-byte envelope in §10.4, not of the symbology.
pub const SIGNED_VERSION_FLOOR: u8 = 5;

/// An issuer's private key.
///
/// Deliberately has no `Display`, no serialisation, and a `Debug` that prints nothing
/// but the algorithm.
#[derive(Clone)]
#[non_exhaustive]
pub enum SigningKey {
    /// Ed25519 — COSE `alg` −8. §10.3's recommendation where the key is not in hardware.
    Ed25519(Box<ed25519_dalek::SigningKey>),
    /// ECDSA P-256 / SHA-256 — COSE `alg` −7. What hardware keystores support.
    P256(Box<p256::ecdsa::SigningKey>),
}

impl core::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SigningKey::Ed25519(_) => f.write_str("SigningKey::Ed25519(<redacted>)"),
            SigningKey::P256(_) => f.write_str("SigningKey::P256(<redacted>)"),
        }
    }
}

impl SigningKey {
    /// An Ed25519 key from its 32-byte seed.
    pub fn from_ed25519_bytes(seed: &[u8; 32]) -> SigningKey {
        SigningKey::Ed25519(Box::new(ed25519_dalek::SigningKey::from_bytes(seed)))
    }

    /// A P-256 key from its 32-byte scalar. `None` if the scalar is not a valid key.
    pub fn from_p256_bytes(scalar: &[u8; 32]) -> Option<SigningKey> {
        p256::ecdsa::SigningKey::from_bytes(scalar.into())
            .ok()
            .map(|k| SigningKey::P256(Box::new(k)))
    }

    /// The matching public key — what a verifier needs, and what `kid` is derived from.
    pub fn public_key(&self) -> PublicKey {
        match self {
            SigningKey::Ed25519(k) => PublicKey::Ed25519(k.verifying_key()),
            SigningKey::P256(k) => PublicKey::P256(*k.verifying_key()),
        }
    }

    /// §10.5's 4-byte key hint for this key.
    ///
    /// Computed from the key rather than supplied by the caller, so a packet's `kid` can
    /// never disagree with the key that signed it.
    pub fn kid(&self) -> [u8; 4] {
        self.public_key().kid()
    }

    /// The COSE `alg` this key signs with.
    pub fn alg(&self) -> i64 {
        self.public_key().alg()
    }

    /// Raw signature bytes over `message`, in the fixed-width form COSE wants.
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        match self {
            SigningKey::Ed25519(k) => {
                use ed25519_dalek::Signer;
                k.sign(message).to_bytes().to_vec()
            }
            SigningKey::P256(k) => {
                use p256::ecdsa::signature::Signer;
                // RFC 6979 deterministic ECDSA, and `Signature` is already fixed-width
                // r‖s — RFC 9053 §2.1 forbids the DER form here.
                let sig: p256::ecdsa::Signature = k.sign(message);
                sig.to_bytes().to_vec()
            }
        }
    }
}

/// What goes into the signed packet besides the body.
#[derive(Clone, Debug)]
pub struct SignOptions {
    /// COSE `content type` (protected label 3), identifying the inner body type or
    /// schema family. REQUIRED by §10.2.
    pub content_type: u64,
    /// `exp`, seconds since the Unix epoch. REQUIRED by §10.6.
    ///
    /// §10.6 is blunt about the consequence of getting this wrong: with no revocation,
    /// "the `exp` window is the entire blast radius". Choose the shortest window the use
    /// case tolerates and re-issue, rather than issuing one long-lived packet.
    pub exp: u64,
    /// `iat`, seconds since the Unix epoch.
    pub iat: u64,
    /// `lcr.schema` (§11), if the body follows a registered schema.
    pub schema: Option<u16>,
    /// `iss`. §10.5 prefers relying on `kid`; this exists for deployments that need it
    /// and costs symbol area, so it defaults to absent.
    pub iss: Option<String>,
}

impl SignOptions {
    /// Claims valid for `ttl` seconds from `now`.
    pub fn expiring_in(now: u64, ttl: u64) -> SignOptions {
        SignOptions {
            content_type: 0,
            exp: now.saturating_add(ttl),
            iat: now,
            schema: None,
            iss: None,
        }
    }
}

/// Build a signed COSE_Sign1 packet: claims, `Sig_structure`, signature, envelope.
///
/// The result is the body of a `ctype = 0x4` frame. [`encode_signed`] wraps it in a
/// symbol; this function exists separately so a caller can inspect or transport the COSE
/// bytes on their own.
pub fn sign(key: &SigningKey, body: &[u8], opts: &SignOptions) -> Result<Vec<u8>, LcrError> {
    let claims = Claims {
        iss: opts.iss.clone(),
        exp: Some(opts.exp),
        iat: Some(opts.iat),
        schema: opts.schema,
        body: Some(body.to_vec()),
    };
    let payload = cwt::build(&claims);
    let protected = cose::protected_header(key.alg(), opts.content_type, key.kid());
    let signature = key.sign(&cose::sig_structure(&protected, &payload));
    Ok(cose::assemble(&protected, &payload, &signature))
}

/// Assemble a COSE_Sign1 from a signature produced elsewhere — a hardware keystore, an
/// HSM, a remote signing service.
///
/// The caller signs `sig_structure(protected, payload)` and hands the result back. This
/// is the path §10.3 has in mind when it says to choose an algorithm "by where the key
/// lives, not by elegance".
pub fn cose_sign1_from_parts(protected: &[u8], payload: &[u8], signature: &[u8]) -> Vec<u8> {
    cose::assemble(protected, payload, signature)
}

/// The exact bytes a detached signer must sign, for use with [`cose_sign1_from_parts`].
pub fn signing_input(
    alg: i64,
    content_type: u64,
    kid: [u8; 4],
    claims: &Claims,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let payload = cwt::build(claims);
    let protected = cose::protected_header(alg, content_type, kid);
    let to_sign = cose::sig_structure(&protected, &payload);
    (protected, payload, to_sign)
}

/// Sign a body and encode it as a symbol (`ctype = 0x4`).
///
/// Applies §6.1's v5 floor unless the caller pinned a version explicitly: below v5 the
/// ~107-byte envelope of §10.4 leaves almost nothing for the body, and a caller who has
/// not thought about it is better served by a symbol that fits than by one that fails.
pub fn encode_signed(
    key: &SigningKey,
    body: &[u8],
    sign_opts: &SignOptions,
    opts: &EncodeOptions,
) -> Result<Symbol, LcrError> {
    let cose = sign(key, body, sign_opts)?;
    let symbol = crate::encode(&cose, ContentType::CoseSign1, opts)?;
    // The floor is a *minimum*, not a pin: a body too large for v5 must still get the
    // version it needs, so raise only when the natural choice came in under it.
    if opts.version.is_none() && symbol.format.version.get() < SIGNED_VERSION_FLOOR {
        let mut raised = opts.clone();
        raised.version = Some(Version::new(SIGNED_VERSION_FLOOR)?);
        return crate::encode(&cose, ContentType::CoseSign1, &raised);
    }
    Ok(symbol)
}
