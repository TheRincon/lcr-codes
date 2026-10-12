//! CWT claims (RFC 8392) as §10.2 uses them.
//!
//! Five labels matter. `cti` (claim 7) is **deliberately absent** and parsing does not
//! look for it: it exists to support revocation, which LCR Code does not have, and it is
//! by construction a stable unique identifier. Omitting it is what lets §10.6 claim that
//! no field in an LCR Code packet is a designed tracking token.

use alloc::string::String;
use alloc::vec::Vec;
use lcr_core::cbor::Value;

/// `iss` — optional; §10.5 prefers relying on `kid`.
const CLAIM_ISS: i64 = 1;
/// `exp` — REQUIRED (§10.6).
const CLAIM_EXP: i64 = 4;
/// `iat` — REQUIRED.
const CLAIM_IAT: i64 = 6;
/// `lcr.schema` — optional uint schema ID (§11).
const CLAIM_SCHEMA: i64 = -262_144;
/// `lcr.body` — REQUIRED application body.
const CLAIM_BODY: i64 = -262_145;

/// The claims carried by a signed packet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Claims {
    /// `iss`, if the issuer chose to include one.
    pub iss: Option<String>,
    /// `exp`. Absent is a rejection, not a licence to treat the packet as non-expiring.
    pub exp: Option<u64>,
    /// `iat`.
    pub iat: Option<u64>,
    /// `lcr.schema` (§11).
    pub schema: Option<u16>,
    /// `lcr.body` — the application body.
    pub body: Option<Vec<u8>>,
}

/// Parse a CWT claims map.
///
/// Returns `None` if the payload is not a map or repeats a label. Missing claims are
/// *not* an error here — [`Claims::exp`] and [`Claims::body`] being `None` is what the
/// verifier turns into `MissingExpiry` and `MalformedCose`, so that each failure keeps
/// its own name rather than collapsing into one.
pub fn parse(payload: &[u8]) -> Option<Claims> {
    let value = Value::from_slice(payload).ok()?;
    let map = value.as_map()?;

    for i in 0..map.len() {
        for j in (i + 1)..map.len() {
            if map[i].0 == map[j].0 {
                return None;
            }
        }
    }

    let mut claims = Claims::default();
    for (label, item) in map {
        let Some(label) = label.as_i64() else {
            // A non-integer label is not one of ours; §10.2 defines no text labels.
            return None;
        };
        match label {
            CLAIM_ISS => claims.iss = Some(String::from(item.as_text()?)),
            CLAIM_EXP => claims.exp = Some(as_uint(item)?),
            CLAIM_IAT => claims.iat = Some(as_uint(item)?),
            CLAIM_SCHEMA => claims.schema = Some(u16::try_from(as_uint(item)?).ok()?),
            CLAIM_BODY => claims.body = Some(item.as_bytes()?.to_vec()),
            // Unknown claims are ignored rather than rejected: §11 expects the registry
            // to grow, and an old verifier refusing a new optional claim would make
            // every addition a breaking change.
            _ => {}
        }
    }
    Some(claims)
}

/// Serialise claims in canonical label order.
pub fn build(claims: &Claims) -> Vec<u8> {
    let mut entries = Vec::new();
    if let Some(iss) = &claims.iss {
        entries.push((Value::from_i64(CLAIM_ISS), Value::Text(iss.clone())));
    }
    if let Some(exp) = claims.exp {
        entries.push((Value::from_i64(CLAIM_EXP), Value::Uint(exp)));
    }
    if let Some(iat) = claims.iat {
        entries.push((Value::from_i64(CLAIM_IAT), Value::Uint(iat)));
    }
    if let Some(schema) = claims.schema {
        entries.push((Value::from_i64(CLAIM_SCHEMA), Value::Uint(schema as u64)));
    }
    if let Some(body) = &claims.body {
        entries.push((Value::from_i64(CLAIM_BODY), Value::Bytes(body.clone())));
    }
    Value::Map(entries).to_vec()
}

fn as_uint(v: &Value) -> Option<u64> {
    match *v {
        Value::Uint(u) => Some(u),
        _ => None,
    }
}
