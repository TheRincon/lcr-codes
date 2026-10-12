//! A small, strict CBOR (RFC 8949) codec.
//!
//! §9.2 makes CBOR the typical content type, and §10 builds COSE_Sign1 and CWT on top of
//! it. Those needs are narrow — no indefinite lengths, no bignums, no half floats on the
//! encode side — and the decoder takes untrusted input from a camera, so a small
//! auditable implementation is worth more here than a general one.
//!
//! Three properties the decoder guarantees, all of them fuzz targets:
//!
//! * **Bounded recursion.** Nesting is capped at [`MAX_DEPTH`]; a symbol full of `0x9F`
//!   cannot blow the stack.
//! * **No length-driven pre-allocation.** A declared length of `2^64 − 1` allocates
//!   nothing until the bytes actually arrive.
//! * **Canonical form is checked on the way in.** Non-shortest integer encodings are
//!   rejected, so a signed packet has exactly one valid byte representation and
//!   `decode(encode(x)) == x` cannot be gamed.

use crate::error::LcrError;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// Maximum nesting depth accepted by the decoder.
pub const MAX_DEPTH: usize = 32;

/// A CBOR data item.
#[derive(Clone, PartialEq, Debug)]
pub enum Value {
    /// Major type 0.
    Uint(u64),
    /// Major type 1. Stores `n` for the encoded value `−1 − n`.
    Nint(u64),
    /// Major type 2.
    Bytes(Vec<u8>),
    /// Major type 3.
    Text(String),
    /// Major type 4.
    Array(Vec<Value>),
    /// Major type 5. Order is preserved; CWT and COSE both care.
    Map(Vec<(Value, Value)>),
    /// Major type 6.
    Tag(u64, Box<Value>),
    /// `false` / `true`.
    Bool(bool),
    /// `null`.
    Null,
    /// `undefined`.
    Undefined,
    /// Double-precision float.
    Float(f64),
}

impl Value {
    /// Signed integer view of `Uint`/`Nint`, if it fits.
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::Uint(u) => i64::try_from(u).ok(),
            Value::Nint(n) => i64::try_from(n).ok().and_then(|n| (-1i64).checked_sub(n)),
            _ => None,
        }
    }

    /// Build from a signed integer.
    pub fn from_i64(v: i64) -> Value {
        if v >= 0 {
            Value::Uint(v as u64)
        } else {
            Value::Nint((-1 - v) as u64)
        }
    }

    /// Byte-string contents, if this is one.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// Text contents, if this is one.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Map entries, if this is a map.
    pub fn as_map(&self) -> Option<&[(Value, Value)]> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    /// Array entries, if this is an array.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    /// Look up an integer-keyed map entry — the shape every CWT claim and COSE header
    /// label takes.
    pub fn get_int_key(&self, key: i64) -> Option<&Value> {
        self.as_map()?
            .iter()
            .find(|(k, _)| k.as_i64() == Some(key))
            .map(|(_, v)| v)
    }

    /// Encode to canonical CBOR bytes.
    pub fn to_vec(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Value::Uint(n) => head(out, 0, *n),
            Value::Nint(n) => head(out, 1, *n),
            Value::Bytes(b) => {
                head(out, 2, b.len() as u64);
                out.extend_from_slice(b);
            }
            Value::Text(s) => {
                head(out, 3, s.len() as u64);
                out.extend_from_slice(s.as_bytes());
            }
            Value::Array(a) => {
                head(out, 4, a.len() as u64);
                for v in a {
                    v.encode_into(out);
                }
            }
            Value::Map(m) => {
                head(out, 5, m.len() as u64);
                for (k, v) in m {
                    k.encode_into(out);
                    v.encode_into(out);
                }
            }
            Value::Tag(t, v) => {
                head(out, 6, *t);
                v.encode_into(out);
            }
            Value::Bool(false) => out.push(0xF4),
            Value::Bool(true) => out.push(0xF5),
            Value::Null => out.push(0xF6),
            Value::Undefined => out.push(0xF7),
            Value::Float(f) => {
                // Shortest form that round-trips exactly, per RFC 8949 §4.2.2.
                let f32v = *f as f32;
                if f.is_nan() {
                    out.extend_from_slice(&[0xF9, 0x7E, 0x00]);
                } else if f32v as f64 == *f {
                    out.push(0xFA);
                    out.extend_from_slice(&f32v.to_be_bytes());
                } else {
                    out.push(0xFB);
                    out.extend_from_slice(&f.to_be_bytes());
                }
            }
        }
    }

    /// Decode one item from `bytes`, requiring it to consume the whole slice.
    pub fn from_slice(bytes: &[u8]) -> Result<Value, LcrError> {
        let mut d = Decoder { b: bytes, pos: 0 };
        let v = d.item(0)?;
        if d.pos != bytes.len() {
            return Err(LcrError::BadCbor);
        }
        Ok(v)
    }

    /// Decode one item, returning it and the number of bytes consumed.
    pub fn from_prefix(bytes: &[u8]) -> Result<(Value, usize), LcrError> {
        let mut d = Decoder { b: bytes, pos: 0 };
        let v = d.item(0)?;
        Ok((v, d.pos))
    }
}

fn head(out: &mut Vec<u8>, major: u8, arg: u64) {
    let m = major << 5;
    match arg {
        0..=23 => out.push(m | arg as u8),
        24..=0xFF => out.extend_from_slice(&[m | 24, arg as u8]),
        0x100..=0xFFFF => {
            out.push(m | 25);
            out.extend_from_slice(&(arg as u16).to_be_bytes());
        }
        0x1_0000..=0xFFFF_FFFF => {
            out.push(m | 26);
            out.extend_from_slice(&(arg as u32).to_be_bytes());
        }
        _ => {
            out.push(m | 27);
            out.extend_from_slice(&arg.to_be_bytes());
        }
    }
}

struct Decoder<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    fn byte(&mut self) -> Result<u8, LcrError> {
        let v = *self.b.get(self.pos).ok_or(LcrError::BadCbor)?;
        self.pos += 1;
        Ok(v)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], LcrError> {
        let end = self.pos.checked_add(n).ok_or(LcrError::BadCbor)?;
        let s = self.b.get(self.pos..end).ok_or(LcrError::BadCbor)?;
        self.pos = end;
        Ok(s)
    }

    /// Read the argument of a head byte, rejecting non-shortest encodings.
    fn arg(&mut self, info: u8) -> Result<u64, LcrError> {
        let v = match info {
            0..=23 => return Ok(info as u64),
            24 => {
                let v = self.byte()? as u64;
                if v < 24 {
                    return Err(LcrError::BadCbor);
                }
                v
            }
            25 => {
                let b = self.take(2)?;
                let v = u16::from_be_bytes([b[0], b[1]]) as u64;
                if v <= 0xFF {
                    return Err(LcrError::BadCbor);
                }
                v
            }
            26 => {
                let b = self.take(4)?;
                let v = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64;
                if v <= 0xFFFF {
                    return Err(LcrError::BadCbor);
                }
                v
            }
            27 => {
                let b = self.take(8)?;
                let v = u64::from_be_bytes(b.try_into().map_err(|_| LcrError::BadCbor)?);
                if v <= 0xFFFF_FFFF {
                    return Err(LcrError::BadCbor);
                }
                v
            }
            _ => return Err(LcrError::BadCbor), // 28..30 reserved, 31 = indefinite
        };
        Ok(v)
    }

    fn item(&mut self, depth: usize) -> Result<Value, LcrError> {
        if depth > MAX_DEPTH {
            return Err(LcrError::BadCbor);
        }
        let head = self.byte()?;
        let (major, info) = (head >> 5, head & 0x1F);
        Ok(match major {
            0 => Value::Uint(self.arg(info)?),
            1 => Value::Nint(self.arg(info)?),
            2 => Value::Bytes(self.blob(info)?.to_vec()),
            3 => {
                let raw = self.blob(info)?;
                Value::Text(
                    core::str::from_utf8(raw)
                        .map_err(|_| LcrError::BadCbor)?
                        .into(),
                )
            }
            4 => {
                let n = self.arg(info)?;
                // One byte minimum per element: reject a length the input cannot hold
                // before allocating for it.
                self.check_room(n)?;
                let mut a = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    a.push(self.item(depth + 1)?);
                }
                Value::Array(a)
            }
            5 => {
                let n = self.arg(info)?;
                self.check_room(n.saturating_mul(2))?;
                let mut m = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    let k = self.item(depth + 1)?;
                    let v = self.item(depth + 1)?;
                    m.push((k, v));
                }
                Value::Map(m)
            }
            6 => Value::Tag(self.arg(info)?, Box::new(self.item(depth + 1)?)),
            _ => match info {
                20 => Value::Bool(false),
                21 => Value::Bool(true),
                22 => Value::Null,
                23 => Value::Undefined,
                25 => {
                    let b = self.take(2)?;
                    Value::Float(f16_to_f64(u16::from_be_bytes([b[0], b[1]])))
                }
                26 => {
                    let b = self.take(4)?;
                    Value::Float(f32::from_be_bytes(b.try_into().unwrap()) as f64)
                }
                27 => {
                    let b = self.take(8)?;
                    Value::Float(f64::from_be_bytes(b.try_into().unwrap()))
                }
                _ => return Err(LcrError::BadCbor),
            },
        })
    }

    fn blob(&mut self, info: u8) -> Result<&'a [u8], LcrError> {
        let n = self.arg(info)?;
        let n = usize::try_from(n).map_err(|_| LcrError::BadCbor)?;
        self.take(n)
    }

    /// Reject an element count larger than the remaining input could possibly encode,
    /// so `with_capacity` is never handed an attacker-chosen number.
    fn check_room(&self, count: u64) -> Result<(), LcrError> {
        if count > (self.b.len() - self.pos) as u64 {
            Err(LcrError::BadCbor)
        } else {
            Ok(())
        }
    }
}

fn f16_to_f64(h: u16) -> f64 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1F) as i32;
    let frac = (h & 0x3FF) as f64;
    sign * match exp {
        0 => frac * 2f64.powi(-24),
        31 => {
            if frac == 0.0 {
                f64::INFINITY
            } else {
                return f64::NAN;
            }
        }
        _ => (frac + 1024.0) * 2f64.powi(exp - 25),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn roundtrip(v: Value) {
        let bytes = v.to_vec();
        assert_eq!(Value::from_slice(&bytes).unwrap(), v, "bytes {bytes:02x?}");
    }

    #[test]
    fn rfc8949_appendix_a_vectors() {
        let cases: &[(&[u8], Value)] = &[
            (&[0x00], Value::Uint(0)),
            (&[0x17], Value::Uint(23)),
            (&[0x18, 0x18], Value::Uint(24)),
            (&[0x19, 0x03, 0xE8], Value::Uint(1000)),
            (&[0x1A, 0x00, 0x0F, 0x42, 0x40], Value::Uint(1_000_000)),
            (&[0x20], Value::from_i64(-1)),
            (&[0x38, 0x63], Value::from_i64(-100)),
            (&[0x40], Value::Bytes(vec![])),
            (&[0x44, 1, 2, 3, 4], Value::Bytes(vec![1, 2, 3, 4])),
            (&[0x60], Value::Text(String::new())),
            (&[0x61, 0x61], Value::Text("a".into())),
            (&[0x80], Value::Array(vec![])),
            (
                &[0x83, 0x01, 0x02, 0x03],
                Value::Array(vec![Value::Uint(1), Value::Uint(2), Value::Uint(3)]),
            ),
            (&[0xA0], Value::Map(vec![])),
            (&[0xF4], Value::Bool(false)),
            (&[0xF5], Value::Bool(true)),
            (&[0xF6], Value::Null),
            (&[0xF7], Value::Undefined),
            (
                &[0xC1, 0x1A, 0x51, 0x4B, 0x67, 0xB0],
                Value::Tag(1, Box::new(Value::Uint(1_363_896_240))),
            ),
        ];
        for (bytes, want) in cases {
            assert_eq!(
                &Value::from_slice(bytes).unwrap(),
                want,
                "decoding {bytes:02x?}"
            );
            assert_eq!(&want.to_vec(), bytes, "encoding {want:?}");
        }
    }

    #[test]
    fn nested_structures_roundtrip() {
        roundtrip(Value::Map(vec![
            (Value::from_i64(1), Value::Text("iss".into())),
            (Value::from_i64(4), Value::Uint(1_700_000_000)),
            (Value::from_i64(-262145), Value::Bytes(vec![0xDE, 0xAD])),
            (
                Value::Text("nested".into()),
                Value::Array(vec![Value::Bool(true), Value::Null, Value::Float(1.5)]),
            ),
        ]));
    }

    #[test]
    fn negative_integers_roundtrip_through_i64() {
        for v in [-1i64, -100, -262_144, -262_145, i64::MIN + 1] {
            assert_eq!(Value::from_i64(v).as_i64(), Some(v));
            roundtrip(Value::from_i64(v));
        }
    }

    #[test]
    fn non_shortest_encodings_are_rejected() {
        // 24 encoded in two bytes when one would do.
        assert!(Value::from_slice(&[0x18, 0x17]).is_err());
        assert!(Value::from_slice(&[0x19, 0x00, 0x64]).is_err());
        assert!(Value::from_slice(&[0x1A, 0x00, 0x00, 0x03, 0xE8]).is_err());
        assert!(Value::from_slice(&[0x1B, 0, 0, 0, 0, 0, 0x0F, 0x42, 0x40]).is_err());
    }

    #[test]
    fn indefinite_lengths_are_rejected() {
        assert!(Value::from_slice(&[0x9F, 0x01, 0xFF]).is_err());
        assert!(Value::from_slice(&[0x5F, 0x41, 0x01, 0xFF]).is_err());
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        assert!(Value::from_slice(&[0x01, 0x02]).is_err());
        let (v, n) = Value::from_prefix(&[0x01, 0x02]).unwrap();
        assert_eq!((v, n), (Value::Uint(1), 1));
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let deep: Vec<u8> = core::iter::repeat_n(0x81u8, MAX_DEPTH + 5)
            .chain([0x00])
            .collect();
        assert!(Value::from_slice(&deep).is_err());
        let ok: Vec<u8> = core::iter::repeat_n(0x81u8, 8).chain([0x00]).collect();
        assert!(Value::from_slice(&ok).is_ok());
    }

    #[test]
    fn huge_declared_lengths_do_not_allocate() {
        // "array of 2^32 items" with no items following.
        assert!(Value::from_slice(&[0x9A, 0xFF, 0xFF, 0xFF, 0xFF]).is_err());
        // "byte string of 2^64-1 bytes".
        assert!(
            Value::from_slice(&[0x5B, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]).is_err()
        );
    }

    #[test]
    fn invalid_utf8_in_text_is_rejected() {
        assert!(Value::from_slice(&[0x62, 0xFF, 0xFE]).is_err());
    }

    #[test]
    fn half_floats_decode() {
        assert_eq!(
            Value::from_slice(&[0xF9, 0x3C, 0x00]).unwrap(),
            Value::Float(1.0)
        );
        assert_eq!(
            Value::from_slice(&[0xF9, 0xC0, 0x00]).unwrap(),
            Value::Float(-2.0)
        );
    }

    #[test]
    fn floats_use_the_shortest_exact_form() {
        assert_eq!(Value::Float(1.5).to_vec()[0], 0xFA);
        assert_eq!(Value::Float(1.1).to_vec()[0], 0xFB);
        roundtrip(Value::Float(1.1));
        roundtrip(Value::Float(-0.0));
    }

    #[test]
    fn int_key_lookup_finds_cwt_claims() {
        let m = Value::Map(vec![
            (Value::from_i64(4), Value::Uint(99)),
            (Value::from_i64(-262145), Value::Bytes(vec![7])),
        ]);
        assert_eq!(m.get_int_key(4), Some(&Value::Uint(99)));
        assert_eq!(
            m.get_int_key(-262145).and_then(|v| v.as_bytes()),
            Some(&[7u8][..])
        );
        assert_eq!(m.get_int_key(1), None);
    }

    #[test]
    fn truncated_input_is_an_error_not_a_panic() {
        let full = Value::Map(vec![(Value::Uint(1), Value::Text("hello".into()))]).to_vec();
        for n in 0..full.len() {
            assert!(
                Value::from_slice(&full[..n]).is_err(),
                "prefix of {n} accepted"
            );
        }
    }
}
