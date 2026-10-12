//! Offline verification of signed LCR Code packets (§10).
//!
//! Optional by design. §1.2 expects most packets to be unsigned, and a decoder built
//! without this crate is fully conformant — it simply answers
//! `Packet::Unverified { reason: NoKeysConfigured }` for anything signed.
//!
//! ## What this crate will not do
//!
//! Three refusals, all from §10, all structural rather than configurable:
//!
//! 1. **It will not take a key from the packet.** [`KeyResolver`] is the only source of
//!    key material. Nothing here parses a certificate or a JWK out of a symbol, so
//!    "signed by a key the attacker generated" is not a state this code can reach.
//! 2. **It will not accept without a clock.** [`Clock::now`] returns `Option`, and `None`
//!    means rejection, not "skip the expiry check" (§10.6).
//! 3. **It will not return a body it did not verify.** Every failure path produces
//!    [`Packet::Unverified`], which has no body field at all (§14.3).
//!
//! ## Order of checks
//!
//! Signature first, claims afterwards. Nothing in the payload — not `exp`, not `iat`,
//! not the body — is read until the signature over it has verified, so a forged packet
//! can never influence a decision other than "reject".
//!
//! ```text
//! parse COSE        → MalformedCose
//! alg in §10.3?     → UnsupportedAlg
//! keys for kid?     → UnknownKid
//! signature valid?  → BadSignature      ← nothing below here runs on unverified bytes
//! claims parse?     → MalformedCose
//! exp present?      → MissingExpiry
//! clock available?  → NoTrustedClock
//! exp in future?    → Expired
//! body present?     → MalformedCose
//!                   → Verified
//! ```

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod clock;
pub mod cose;
pub mod cwt;
pub mod key;

#[cfg(feature = "std")]
pub use clock::SystemClock;
pub use clock::{Clock, FixedClock, NoClock};
pub use key::{ALG_EDDSA, ALG_ES256, KeyResolver, KeyRing, PublicKey};

use lcr_core::{Packet, VerifyFailure};

/// A verifier: a set of trusted keys plus a clock.
///
/// Both are supplied by the deployment. LCR Code specifies no trust list format and no
/// distribution mechanism (§10.5), and takes no position on where time comes from
/// (§10.6) — it only requires that both exist before anything is accepted.
#[derive(Clone, Debug)]
pub struct Verifier<R, C> {
    keys: R,
    clock: C,
}

impl<R: KeyResolver, C: Clock> Verifier<R, C> {
    /// Build a verifier from a key resolver and a clock.
    pub fn new(keys: R, clock: C) -> Verifier<R, C> {
        Verifier { keys, clock }
    }

    /// The trusted keys, for callers that want to inspect or extend them.
    pub fn keys(&self) -> &R {
        &self.keys
    }

    /// Verify a COSE_Sign1 body and turn it into a [`Packet`].
    ///
    /// Never returns an error: a scanner needs to tell the user *why* a packet was
    /// rejected, and §10.8 requires that no failure mode be presentable as success. Both
    /// are served by every path returning a `Packet`.
    pub fn verify_cose(&self, bytes: &[u8]) -> Packet {
        let Some(sign1) = cose::parse(bytes) else {
            return fail(VerifyFailure::MalformedCose, [0; 4]);
        };
        let kid = sign1.kid;

        if sign1.alg != ALG_ES256 && sign1.alg != ALG_EDDSA {
            return fail(VerifyFailure::UnsupportedAlg, kid);
        }

        let candidates = self.keys.keys_for(kid);
        if candidates.is_empty() {
            return fail(VerifyFailure::UnknownKid, kid);
        }

        // §10.5: a 4-byte `kid` collides by design, so try every candidate and accept if
        // any succeeds. Keys whose algorithm does not match `alg` are skipped rather
        // than failed — a collision across algorithms is just another collision.
        let signed = cose::sig_structure(sign1.protected, &sign1.payload);
        let verified = candidates
            .iter()
            .filter(|k| k.alg() == sign1.alg)
            .any(|k| k.verify(&signed, &sign1.signature));
        if !verified {
            return fail(VerifyFailure::BadSignature, kid);
        }

        // Past this line the bytes are authentic, so reading them is safe.
        let Some(claims) = cwt::parse(&sign1.payload) else {
            return fail(VerifyFailure::MalformedCose, kid);
        };
        let Some(exp) = claims.exp else {
            return fail(VerifyFailure::MissingExpiry, kid);
        };
        let Some(now) = self.clock.now() else {
            return fail(VerifyFailure::NoTrustedClock, kid);
        };
        if exp <= now {
            return fail(VerifyFailure::Expired { exp }, kid);
        }
        let Some(body) = claims.body else {
            return fail(VerifyFailure::MalformedCose, kid);
        };

        Packet::Verified {
            body,
            schema: claims.schema,
            exp,
            kid,
        }
    }
}

#[cfg(feature = "decode")]
impl<R: KeyResolver, C: Clock> lcr_decode::PacketVerifier for Verifier<R, C> {
    fn verify(&self, cose_sign1: &[u8]) -> Packet {
        self.verify_cose(cose_sign1)
    }
}

fn fail(reason: VerifyFailure, kid: [u8; 4]) -> Packet {
    Packet::Unverified { reason, kid }
}
