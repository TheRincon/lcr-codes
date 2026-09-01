//! JSON ⇄ CBOR, for the §9.4 "author in JSON, transmit CBOR" boundary.
//!
//! §9.4 is explicit that JSON should not go on the wire — key strings, punctuation and
//! whitespace compress badly at the sizes LC Code works in, and CBOR is 30–50% smaller
//! before compression. But JSON is what applications have, so the crate converts at the
//! API edge rather than making callers do it.
//!
//! The parser is intentionally strict (RFC 8259, no comments, no trailing commas) and
//! bounded: nesting is capped at [`crate::cbor::MAX_DEPTH`] like the CBOR decoder, since
//! both are reachable from caller-controlled input.
//!
//! ## Number handling
//!
//! A JSON number with no fraction or exponent that fits in `i64` becomes a CBOR integer;
//! everything else becomes a float. This is what makes the size argument real — `42` is
//! one byte as a CBOR integer and three as a float — and it is lossless in the direction
//! that matters, because a JSON integer literal that fits `i64` has an exact CBOR
//! integer representation.

use crate::cbor::{MAX_DEPTH, Value};
use crate::error::LcError;
use alloc::string::String;
use alloc::vec::Vec;

/// Parse JSON text into a CBOR value.
pub fn parse(input: &str) -> Result<Value, LcError> {
    let mut p = Parser {
        b: input.as_bytes(),
        pos: 0,
    };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.pos != p.b.len() {
        return Err(LcError::BadJson(p.pos));
    }
    Ok(v)
}

/// Render a CBOR value as JSON text.
///
/// Byte strings become lowercase hex — CBOR has a type JSON does not, and hex is the
/// least surprising rendering for the diagnostic and CLI paths this serves. Round-tripping
/// JSON → CBOR → JSON is exact; CBOR → JSON → CBOR is not, by construction.
pub fn to_json(v: &Value) -> String {
    let mut s = String::new();
    write_json(&mut s, v);
    s
}

fn write_json(out: &mut String, v: &Value) {
    use core::fmt::Write;
    match v {
        Value::Uint(n) => {
            let _ = write!(out, "{n}");
        }
        Value::Nint(_) => {
            let _ = write!(out, "{}", v.as_i64().unwrap_or(i64::MIN));
        }
        Value::Float(f) => {
            if f.is_finite() {
                let _ = write!(out, "{f}");
            } else {
                out.push_str("null");
            }
        }
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Null | Value::Undefined => out.push_str("null"),
        Value::Bytes(b) => {
            out.push('"');
            for byte in b {
                let _ = write!(out, "{byte:02x}");
            }
            out.push('"');
        }
        Value::Text(s) => write_string(out, s),
        Value::Tag(_, inner) => write_json(out, inner),
        Value::Array(a) => {
            out.push('[');
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(out, e);
            }
            out.push(']');
        }
        Value::Map(m) => {
            out.push('{');
            for (i, (k, val)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                match k {
                    Value::Text(s) => write_string(out, s),
                    other => {
                        // JSON keys must be strings; render the CBOR key as one.
                        let mut tmp = String::new();
                        write_json(&mut tmp, other);
                        write_string(out, tmp.trim_matches('"'));
                    }
                }
                out.push(':');
                write_json(out, val);
            }
            out.push('}');
        }
    }
}

fn write_string(out: &mut String, s: &str) {
    use core::fmt::Write;
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn err<T>(&self) -> Result<T, LcError> {
        Err(LcError::BadJson(self.pos))
    }

    fn ws(&mut self) {
        while matches!(self.b.get(self.pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Result<(), LcError> {
        if self.b.get(self.pos) == Some(&c) {
            self.pos += 1;
            Ok(())
        } else {
            self.err()
        }
    }

    fn lit(&mut self, word: &[u8]) -> Result<(), LcError> {
        if self.b[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(())
        } else {
            self.err()
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, LcError> {
        if depth > MAX_DEPTH {
            return self.err();
        }
        match self.b.get(self.pos) {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Value::Text(self.string()?)),
            Some(b't') => {
                self.lit(b"true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.lit(b"false")?;
                Ok(Value::Bool(false))
            }
            Some(b'n') => {
                self.lit(b"null")?;
                Ok(Value::Null)
            }
            Some(c) if *c == b'-' || c.is_ascii_digit() => self.number(),
            _ => self.err(),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, LcError> {
        self.eat(b'{')?;
        let mut m = Vec::new();
        self.ws();
        if self.b.get(self.pos) == Some(&b'}') {
            self.pos += 1;
            return Ok(Value::Map(m));
        }
        loop {
            self.ws();
            let k = Value::Text(self.string()?);
            self.ws();
            self.eat(b':')?;
            self.ws();
            m.push((k, self.value(depth + 1)?));
            self.ws();
            match self.b.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Map(m));
                }
                _ => return self.err(),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, LcError> {
        self.eat(b'[')?;
        let mut a = Vec::new();
        self.ws();
        if self.b.get(self.pos) == Some(&b']') {
            self.pos += 1;
            return Ok(Value::Array(a));
        }
        loop {
            self.ws();
            a.push(self.value(depth + 1)?);
            self.ws();
            match self.b.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(a));
                }
                _ => return self.err(),
            }
        }
    }

    fn string(&mut self) -> Result<String, LcError> {
        self.eat(b'"')?;
        let mut s = String::new();
        loop {
            let c = *self.b.get(self.pos).ok_or(LcError::BadJson(self.pos))?;
            self.pos += 1;
            match c {
                b'"' => return Ok(s),
                b'\\' => {
                    let e = *self.b.get(self.pos).ok_or(LcError::BadJson(self.pos))?;
                    self.pos += 1;
                    match e {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'b' => s.push('\u{8}'),
                        b'f' => s.push('\u{c}'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' => s.push(self.unicode_escape()?),
                        _ => return self.err(),
                    }
                }
                c if c < 0x20 => return self.err(),
                c if c < 0x80 => s.push(c as char),
                _ => {
                    // Multi-byte UTF-8: find the character boundary and copy it whole.
                    let start = self.pos - 1;
                    let len = match c {
                        0xC0..=0xDF => 2,
                        0xE0..=0xEF => 3,
                        0xF0..=0xF7 => 4,
                        _ => return self.err(),
                    };
                    let end = start + len;
                    let raw = self.b.get(start..end).ok_or(LcError::BadJson(start))?;
                    let text = core::str::from_utf8(raw).map_err(|_| LcError::BadJson(start))?;
                    s.push_str(text);
                    self.pos = end;
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, LcError> {
        let raw = self
            .b
            .get(self.pos..self.pos + 4)
            .ok_or(LcError::BadJson(self.pos))?;
        let mut v = 0u32;
        for &d in raw {
            v = v * 16 + (d as char).to_digit(16).ok_or(LcError::BadJson(self.pos))?;
        }
        self.pos += 4;
        Ok(v)
    }

    fn unicode_escape(&mut self) -> Result<char, LcError> {
        let hi = self.hex4()?;
        // Surrogate pair.
        if (0xD800..0xDC00).contains(&hi) {
            self.eat(b'\\')?;
            self.eat(b'u')?;
            let lo = self.hex4()?;
            if !(0xDC00..0xE000).contains(&lo) {
                return self.err();
            }
            let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
            return char::from_u32(cp).ok_or(LcError::BadJson(self.pos));
        }
        char::from_u32(hi).ok_or(LcError::BadJson(self.pos))
    }

    fn number(&mut self) -> Result<Value, LcError> {
        let start = self.pos;
        if self.b.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        // RFC 8259 forbids leading zeros: `0` is a number, `01` is two tokens.
        let digits_start = self.pos;
        while self.b.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
        }
        let int_end = self.pos;
        if int_end - digits_start > 1 && self.b[digits_start] == b'0' {
            return self.err();
        }
        let mut is_float = false;
        if self.b.get(self.pos) == Some(&b'.') {
            is_float = true;
            self.pos += 1;
            let frac_start = self.pos;
            while self.b.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
            if self.pos == frac_start {
                return self.err();
            }
        }
        if matches!(self.b.get(self.pos), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.b.get(self.pos), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            let exp_start = self.pos;
            while self.b.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
            if self.pos == exp_start {
                return self.err();
            }
        }
        if int_end == start || (int_end == start + 1 && self.b[start] == b'-') {
            return self.err();
        }
        let text =
            core::str::from_utf8(&self.b[start..self.pos]).map_err(|_| LcError::BadJson(start))?;
        if !is_float {
            if let Ok(i) = text.parse::<i64>() {
                return Ok(Value::from_i64(i));
            }
        }
        text.parse::<f64>()
            .map(Value::Float)
            .map_err(|_| LcError::BadJson(start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn spec_9_4_example_shrinks_as_advertised() {
        // §6.1 quotes this object at 37 bytes of JSON and ~28 of CBOR. The literal is
        // 34 bytes and encodes to 26 — the direction and magnitude of §9.4's claim hold,
        // and the pinned figures here are the measured ones.
        let src = r#"{"name":"Fred","loc":"45.00,55.0"}"#;
        assert_eq!(src.len(), 34);
        let cbor = parse(src).unwrap().to_vec();
        assert!(
            cbor.len() < src.len(),
            "CBOR {} vs JSON {}",
            cbor.len(),
            src.len()
        );
        assert_eq!(cbor.len(), 26);
    }

    #[test]
    fn json_roundtrips_through_cbor() {
        for src in [
            "{}",
            "[]",
            "null",
            "true",
            "0",
            "-1",
            "[1,2,3]",
            r#"{"a":1,"b":[true,false,null]}"#,
            r#"{"nested":{"deep":{"deeper":[1,2,{"x":-5}]}}}"#,
        ] {
            let v = parse(src).unwrap();
            assert_eq!(to_json(&v), src, "roundtrip of {src}");
            assert_eq!(Value::from_slice(&v.to_vec()).unwrap(), v);
        }
    }

    #[test]
    fn integers_stay_integers() {
        assert_eq!(parse("42").unwrap(), Value::Uint(42));
        assert_eq!(parse("-42").unwrap(), Value::from_i64(-42));
        assert_eq!(parse("42").unwrap().to_vec().len(), 2);
        // A float would cost five bytes; the integer path is why §9.4's saving is real.
        assert_eq!(parse("42.0").unwrap(), Value::Float(42.0));
        assert_eq!(parse("1e3").unwrap(), Value::Float(1000.0));
    }

    #[test]
    fn escapes_and_unicode() {
        assert_eq!(parse(r#""a\nb""#).unwrap(), Value::Text("a\nb".into()));
        assert_eq!(parse(r#""é""#).unwrap(), Value::Text("é".into()));
        assert_eq!(parse(r#""😀""#).unwrap(), Value::Text("😀".into()));
        assert_eq!(
            parse("\"héllo 😀\"").unwrap(),
            Value::Text("héllo 😀".into())
        );
        assert_eq!(
            to_json(&Value::Text("a\"b\\c\nd".into())),
            r#""a\"b\\c\nd""#
        );
    }

    #[test]
    fn malformed_input_is_rejected_with_an_offset() {
        for bad in [
            "",
            "{",
            "[",
            "{\"a\"}",
            "{\"a\":}",
            "[1,]",
            "{,}",
            "tru",
            "01",
            "-",
            "1.",
            "\"unterminated",
            "{\"a\":1}}",
            "[1 2]",
            "{'a':1}",
            "1e",
            "1e+",
            ".5",
            "+1",
        ] {
            assert!(
                matches!(parse(bad), Err(LcError::BadJson(_))),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let deep = "[".repeat(MAX_DEPTH + 5) + &"]".repeat(MAX_DEPTH + 5);
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn whitespace_is_tolerated_everywhere() {
        let v = parse(" {\n \"a\" : [ 1 , 2 ] \t} ").unwrap();
        assert_eq!(
            v,
            Value::Map(vec![(
                Value::Text("a".into()),
                Value::Array(vec![Value::Uint(1), Value::Uint(2)])
            )])
        );
    }

    #[test]
    fn byte_strings_render_as_hex() {
        assert_eq!(to_json(&Value::Bytes(vec![0xDE, 0xAD])), "\"dead\"");
    }
}
