//! Public keys, `kid` derivation, and key resolution (§10.5).
//!
//! ## The rule this module exists to enforce
//!
//! > Verifiers MUST NOT accept a public key transported in the symbol. — §10.5
//!
//! That is enforced structurally rather than by a check: [`KeyResolver`] is the *only*
//! source of key material in this crate, nothing in [`crate::cose`] returns a key, and
//! the verifier never looks at the packet for one. There is no code path to disable,
//! because there is no code path.

use alloc::vec::Vec;
use sha2::{Digest, Sha256};

/// COSE `alg` value for ECDSA P-256 with SHA-256 (§10.3).
pub const ALG_ES256: i64 = -7;
/// COSE `alg` value for EdDSA over Ed25519 (§10.3).
pub const ALG_EDDSA: i64 = -8;

/// A public key a verifier has decided to trust.
///
/// Constructed only from key material the deployment supplied out of band — never from
/// anything in a symbol.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum PublicKey {
    /// Ed25519, COSE `alg` −8.
    #[cfg(feature = "ed25519")]
    Ed25519(ed25519_dalek::VerifyingKey),
    /// ECDSA P-256 / SHA-256, COSE `alg` −7.
    #[cfg(feature = "es256")]
    P256(p256::ecdsa::VerifyingKey),
}

impl PublicKey {
    /// Parse a DER `SubjectPublicKeyInfo`, the form §10.5 derives `kid` from.
    ///
    /// Tries each compiled-in algorithm; the SPKI algorithm identifier decides.
    pub fn from_der(der: &[u8]) -> Option<PublicKey> {
        #[cfg(feature = "ed25519")]
        {
            use ed25519_dalek::pkcs8::DecodePublicKey;
            if let Ok(k) = ed25519_dalek::VerifyingKey::from_public_key_der(der) {
                return Some(PublicKey::Ed25519(k));
            }
        }
        #[cfg(feature = "es256")]
        {
            use p256::pkcs8::DecodePublicKey;
            if let Ok(k) = p256::ecdsa::VerifyingKey::from_public_key_der(der) {
                return Some(PublicKey::P256(k));
            }
        }
        let _ = der;
        None
    }

    /// The raw 32-byte Ed25519 public key.
    #[cfg(feature = "ed25519")]
    pub fn from_ed25519_bytes(bytes: &[u8; 32]) -> Option<PublicKey> {
        ed25519_dalek::VerifyingKey::from_bytes(bytes)
            .ok()
            .map(PublicKey::Ed25519)
    }

    /// A SEC1 point (compressed or uncompressed) on P-256.
    #[cfg(feature = "es256")]
    pub fn from_p256_sec1(bytes: &[u8]) -> Option<PublicKey> {
        p256::ecdsa::VerifyingKey::from_sec1_bytes(bytes)
            .ok()
            .map(PublicKey::P256)
    }

    /// DER `SubjectPublicKeyInfo` encoding — what `kid` is computed over.
    pub fn to_der(&self) -> Vec<u8> {
        match self {
            #[cfg(feature = "ed25519")]
            PublicKey::Ed25519(k) => {
                use ed25519_dalek::pkcs8::EncodePublicKey;
                k.to_public_key_der()
                    .expect("a valid Ed25519 key always encodes")
                    .as_bytes()
                    .to_vec()
            }
            #[cfg(feature = "es256")]
            PublicKey::P256(k) => {
                use p256::pkcs8::EncodePublicKey;
                // `ecdsa::VerifyingKey` is a signature-facing wrapper; SPKI encoding
                // lives on the curve's own `PublicKey`.
                p256::PublicKey::from(k)
                    .to_public_key_der()
                    .expect("a valid P-256 key always encodes")
                    .as_bytes()
                    .to_vec()
            }
        }
    }

    /// §10.5: the first 4 bytes of SHA-256 over the DER public key.
    ///
    /// A *lookup hint*, not an identifier. Collisions are expected around 65k keys
    /// (birthday bound) and are handled by trying every candidate — see
    /// [`KeyResolver::keys_for`].
    pub fn kid(&self) -> [u8; 4] {
        let digest = Sha256::digest(self.to_der());
        [digest[0], digest[1], digest[2], digest[3]]
    }

    /// The COSE `alg` this key can verify.
    pub fn alg(&self) -> i64 {
        match self {
            #[cfg(feature = "ed25519")]
            PublicKey::Ed25519(_) => ALG_EDDSA,
            #[cfg(feature = "es256")]
            PublicKey::P256(_) => ALG_ES256,
        }
    }

    /// Check a signature over `message`.
    ///
    /// Returns `false` for every failure — a malformed signature, the wrong length, the
    /// wrong curve — so that a caller cannot accidentally distinguish "invalid" from
    /// "unparseable" and treat one of them as success.
    pub fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        match self {
            #[cfg(feature = "ed25519")]
            PublicKey::Ed25519(k) => {
                let Ok(bytes) = <[u8; 64]>::try_from(signature) else {
                    return false;
                };
                // `verify_strict` rejects small-order public keys and non-canonical `s`,
                // which is what we want for a verifier taking input from a camera.
                k.verify_strict(message, &ed25519_dalek::Signature::from_bytes(&bytes))
                    .is_ok()
            }
            #[cfg(feature = "es256")]
            PublicKey::P256(k) => {
                use p256::ecdsa::signature::Verifier;
                // COSE carries ECDSA as fixed-width r‖s (RFC 9053 §2.1), never DER.
                let Ok(sig) = p256::ecdsa::Signature::from_slice(signature) else {
                    return false;
                };
                k.verify(message, &sig).is_ok()
            }
        }
    }
}

/// Where a verifier gets its keys (§10.5).
///
/// How keys arrive — pinned at build time, shipped with an application, fetched on some
/// schedule — is explicitly out of scope for LC Code. This trait is the whole interface.
pub trait KeyResolver {
    /// Every trusted key whose `kid` matches.
    ///
    /// §10.5 **requires** returning all of them and requires the verifier to try each:
    /// a 4-byte hint collides, and a collision must not become a verification failure.
    /// Returning an empty vec means "I do not know this `kid`", which fails closed as
    /// [`lc_core::VerifyFailure::UnknownKid`].
    fn keys_for(&self, kid: [u8; 4]) -> Vec<PublicKey>;
}

/// A fixed set of trusted keys, indexed on demand by `kid`.
///
/// The common deployment: ship the application with the issuer keys it accepts.
#[derive(Clone, Debug, Default)]
pub struct KeyRing {
    keys: Vec<PublicKey>,
}

impl KeyRing {
    /// An empty ring. Verifies nothing; every packet fails as `UnknownKid`.
    pub fn new() -> KeyRing {
        KeyRing { keys: Vec::new() }
    }

    /// Trust one more key.
    pub fn insert(&mut self, key: PublicKey) {
        self.keys.push(key);
    }

    /// Builder form of [`KeyRing::insert`].
    #[must_use]
    pub fn with(mut self, key: PublicKey) -> KeyRing {
        self.insert(key);
        self
    }

    /// How many keys are trusted.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the ring is empty — in which case nothing will ever verify.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

impl KeyResolver for KeyRing {
    fn keys_for(&self, kid: [u8; 4]) -> Vec<PublicKey> {
        self.keys
            .iter()
            .filter(|k| k.kid() == kid)
            .cloned()
            .collect()
    }
}

impl FromIterator<PublicKey> for KeyRing {
    fn from_iter<T: IntoIterator<Item = PublicKey>>(iter: T) -> KeyRing {
        KeyRing {
            keys: iter.into_iter().collect(),
        }
    }
}
