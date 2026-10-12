//! What a decode yields (§14.3), and why the shape is what it is.
//!
//! ## `Unverified` carries no body
//!
//! > It is not possible for a caller to obtain unverified content and forget to check a
//! > boolean, because there is no content to obtain. — §14.3
//!
//! This is the one place in the crate where an API is deliberately *less* convenient than
//! it could be. §10.8 requires that a malformed, expired, unknown-`kid` or
//! wrong-signature packet never be presented as success, and §10.7 requires signed and
//! unsigned packets to be distinguishable states. Both are enforced here by construction
//! rather than by documentation: there is no field to read, so there is nothing to
//! accidentally trust.
//!
//! [`Packet::Unsigned`] does carry a body — that is the §1.2 common case and there is
//! nothing to verify — but it is a *different variant*, so a caller cannot treat the two
//! interchangeably without saying so.

use crate::frame::ContentType;
use alloc::vec::Vec;

/// Why a signed packet was not verified (§14.3).
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum VerifyFailure {
    /// This decoder was built without verification, or configured without keys. The
    /// packet decoded fine; this decoder simply will not make trust decisions.
    NoKeysConfigured,
    /// No key matching the `kid` was available from the resolver (§10.5).
    UnknownKid,
    /// `alg` names something outside §10.3.
    UnsupportedAlg,
    /// The packet's `exp` is in the past (§10.6).
    Expired {
        /// The expiry the packet claimed.
        exp: u64,
    },
    /// No `exp` claim. §10.6 requires rejection rather than treating it as non-expiring.
    MissingExpiry,
    /// The signature did not verify against any candidate key.
    BadSignature,
    /// The COSE_Sign1 structure could not be parsed.
    MalformedCose,
    /// The verifier could not establish a trustworthy time, so `exp` cannot be checked.
    /// §10.6 requires failing closed rather than accepting regardless.
    NoTrustedClock,
}

/// The result of decoding one symbol.
#[derive(Clone, PartialEq, Debug)]
#[non_exhaustive]
pub enum Packet {
    /// Signature verified against a key supplied by the resolver (§10).
    Verified {
        /// The application body from the `lcr.body` CWT claim.
        body: Vec<u8>,
        /// The `lcr.schema` claim, if present (§11).
        schema: Option<u16>,
        /// The `exp` claim, which was checked and is in the future.
        exp: u64,
        /// The key hint the packet carried.
        kid: [u8; 4],
    },
    /// Well-formed enough to identify, but not verified. **No body**, by design.
    Unverified {
        /// Why verification did not succeed.
        reason: VerifyFailure,
        /// The key hint, when one could be read. Zero when the COSE was unparseable.
        kid: [u8; 4],
    },
    /// Decoded successfully and carries no signature — the §1.2 common case.
    Unsigned {
        /// The decompressed application body.
        body: Vec<u8>,
        /// What the body is (§9.2).
        ctype: ContentType,
    },
    /// Only the luma layer decoded, yielding a prefix of the packet (§12.5).
    ///
    /// Returned only when a caller explicitly opts in, and **never** for a signed packet:
    /// a prefix cannot be verified, and §8.1 is explicit that a verifier must not act on
    /// unverified partial data. Treat this as a retry and user-guidance signal — "move
    /// closer", "reduce glare" — rather than as content.
    Partial {
        /// The recovered prefix of the framed packet, as bytes. Not CRC-checked; there
        /// is no CRC to check until the whole frame arrives.
        prefix: Vec<u8>,
    },
}

impl Packet {
    /// The body, for the two variants that have one.
    ///
    /// Deliberately returns `None` for [`Packet::Unverified`] and [`Packet::Partial`] —
    /// there is no body to return, which is the point.
    pub fn body(&self) -> Option<&[u8]> {
        match self {
            Packet::Verified { body, .. } | Packet::Unsigned { body, .. } => Some(body),
            Packet::Unverified { .. } | Packet::Partial { .. } => None,
        }
    }

    /// Whether the packet carried a signature that verified.
    ///
    /// §10.7 requires signed and unsigned to be distinguishable states, and requires that
    /// unsigned content is never presented in a way implying attestation. This is the
    /// predicate a user interface should branch on.
    #[inline]
    pub fn is_attested(&self) -> bool {
        matches!(self, Packet::Verified { .. })
    }

    /// The key hint, when the packet carried one.
    pub fn kid(&self) -> Option<[u8; 4]> {
        match self {
            Packet::Verified { kid, .. } | Packet::Unverified { kid, .. } => Some(*kid),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn unverified_exposes_no_body() {
        let p = Packet::Unverified {
            reason: VerifyFailure::BadSignature,
            kid: [1, 2, 3, 4],
        };
        assert_eq!(p.body(), None);
        assert!(!p.is_attested());
        assert_eq!(p.kid(), Some([1, 2, 3, 4]));
    }

    #[test]
    fn partial_exposes_no_body_either() {
        let p = Packet::Partial {
            prefix: vec![1, 2, 3],
        };
        assert_eq!(p.body(), None);
        assert!(!p.is_attested());
    }

    #[test]
    fn unsigned_is_not_attested() {
        let p = Packet::Unsigned {
            body: vec![7],
            ctype: ContentType::Cbor,
        };
        assert_eq!(p.body(), Some(&[7u8][..]));
        assert!(
            !p.is_attested(),
            "unsigned content must never read as attested"
        );
    }

    #[test]
    fn only_verified_is_attested() {
        let p = Packet::Verified {
            body: vec![9],
            schema: Some(1),
            exp: 99,
            kid: [0; 4],
        };
        assert!(p.is_attested());
        assert_eq!(p.body(), Some(&[9u8][..]));
    }
}
