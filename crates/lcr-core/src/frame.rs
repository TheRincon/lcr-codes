//! Payload container framing (§9).
//!
//! ```text
//! compact  (body ≤ 255 B)    | 0 | ctype:4 | cflag:2 | length:8  | body | CRC-16/CCITT   |
//! extended (body ≤ 16383 B)  | 1 | ctype:4 | cflag:2 | length:14 | body | CRC-32/ISO-HDLC|
//! ```
//!
//! Encoders **MUST** use the compact frame whenever the body fits — [`Frame::to_bits`]
//! does this automatically and there is no way to ask for the extended frame when the
//! compact one would do. §9.1's arithmetic is why: on a 10-byte payload extended framing
//! is 40% overhead and compact is 28%, and §1.2 is emphatic that an unsigned payload
//! must not pay for capability it does not use.
//!
//! ## The CRC is the decode verdict, and only that
//!
//! §9.1 draws a line this module enforces at the type level: **LDPC convergence is not
//! decode success, and CRC success is not verification.** [`Frame::from_bits`] returns
//! [`LcrError::CrcMismatch`] rather than a "probably fine" frame, and it says nothing at
//! all about whether the contents are trustworthy — that is §10's job, and a `Frame`
//! carrying `ContentType::CoseSign1` is just bytes until `lcr-verify` has looked at it.

use crate::bits::BitVec;
use crate::crc::{crc16_ccitt, crc32_iso_hdlc};
use crate::error::LcrError;
use alloc::vec::Vec;

/// Largest body carried by a compact frame.
pub const COMPACT_MAX: usize = 255;
/// Largest body carried by an extended frame.
pub const EXTENDED_MAX: usize = 16383;
/// Largest payload an encoder will frame, and the ceiling a decoder will decompress to.
///
/// The decompression limit exists so a small hostile frame cannot expand without bound.
/// It is also, therefore, the largest payload that can ever come *back*, so the encoder
/// enforces the same number: an encoder that will happily frame 87 KB when the decoder
/// refuses anything over this produces symbols that encode and then fail to read, which is
/// worse than refusing them up front.
pub const MAX_PAYLOAD: usize = EXTENDED_MAX * 4;

/// Content type (§9.2).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum ContentType {
    /// `0x0` — opaque bytes, no interpretation.
    Opaque,
    /// `0x1` — UTF-8 text.
    Text,
    /// `0x2` — JSON. Discouraged on the wire (§9.4).
    Json,
    /// `0x3` — CBOR. The typical case for unsigned structured data.
    Cbor,
    /// `0x4` — COSE_Sign1. The optional signed packet (§10).
    CoseSign1,
    /// `0x5` — multipart (§9.5).
    Multipart,
}

impl ContentType {
    /// Wire value.
    pub const fn to_u8(self) -> u8 {
        match self {
            ContentType::Opaque => 0x0,
            ContentType::Text => 0x1,
            ContentType::Json => 0x2,
            ContentType::Cbor => 0x3,
            ContentType::CoseSign1 => 0x4,
            ContentType::Multipart => 0x5,
        }
    }

    /// Inverse of [`ContentType::to_u8`]; `0x6`–`0xF` are reserved and rejected.
    pub const fn from_u8(v: u8) -> Result<ContentType, LcrError> {
        Ok(match v {
            0x0 => ContentType::Opaque,
            0x1 => ContentType::Text,
            0x2 => ContentType::Json,
            0x3 => ContentType::Cbor,
            0x4 => ContentType::CoseSign1,
            0x5 => ContentType::Multipart,
            other => return Err(LcrError::BadContentType(other)),
        })
    }

    /// True for the one content type that carries a signature (§10).
    #[inline]
    pub const fn is_signed(self) -> bool {
        matches!(self, ContentType::CoseSign1)
    }
}

/// Compression flag (§9.3).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum Compression {
    /// `00` — none.
    None,
    /// `01` — DEFLATE (RFC 1951).
    Deflate,
    /// `10` — Brotli, standard dictionary.
    Brotli,
    /// `11` — Brotli with the LCR Code preset dictionary v1.
    BrotliPreset,
}

impl Compression {
    /// Wire value.
    pub const fn to_u8(self) -> u8 {
        match self {
            Compression::None => 0b00,
            Compression::Deflate => 0b01,
            Compression::Brotli => 0b10,
            Compression::BrotliPreset => 0b11,
        }
    }

    /// Inverse of [`Compression::to_u8`].
    pub const fn from_u8(v: u8) -> Compression {
        match v & 3 {
            0b00 => Compression::None,
            0b01 => Compression::Deflate,
            0b10 => Compression::Brotli,
            _ => Compression::BrotliPreset,
        }
    }
}

/// A framed payload, as it sits on the wire.
///
/// `body` is the **post-compression** byte string, exactly what the length field counts
/// and the CRC covers. Use [`Frame::payload`] to get the application bytes back.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Frame {
    /// What the body is.
    pub ctype: ContentType,
    /// How the body is compressed.
    pub cflag: Compression,
    /// Wire body.
    pub body: Vec<u8>,
}

impl Frame {
    /// Frame an already-final body without compressing it.
    pub fn new(ctype: ContentType, body: Vec<u8>) -> Result<Frame, LcrError> {
        if body.len() > EXTENDED_MAX {
            return Err(LcrError::TooLarge(body.len()));
        }
        Ok(Frame {
            ctype,
            cflag: Compression::None,
            body,
        })
    }

    /// True when this frame will use the compact format.
    #[inline]
    pub fn is_compact(&self) -> bool {
        self.body.len() <= COMPACT_MAX
    }

    /// Total framed size in bits, including header and CRC.
    pub fn encoded_bits(&self) -> usize {
        if self.is_compact() {
            15 + self.body.len() * 8 + 16
        } else {
            21 + self.body.len() * 8 + 32
        }
    }

    /// Framing overhead in bits for a body of `n` bytes — §9.1's 3.9 / 6.6 byte figures.
    pub const fn overhead_bits(n: usize) -> usize {
        if n <= COMPACT_MAX { 15 + 16 } else { 21 + 32 }
    }

    /// Serialise to the bit stream that the layer split consumes (§8.1).
    pub fn to_bits(&self) -> Result<BitVec, LcrError> {
        let n = self.body.len();
        if n > EXTENDED_MAX {
            return Err(LcrError::TooLarge(n));
        }
        let compact = self.is_compact();
        let mut b = BitVec::with_capacity(self.encoded_bits());
        b.push(!compact);
        b.push_bits(self.ctype.to_u8() as u32, 4);
        b.push_bits(self.cflag.to_u8() as u32, 2);
        b.push_bits(n as u32, if compact { 8 } else { 14 });
        b.push_bytes(&self.body);

        // §9.1 pinned: the CRC covers header||body zero-padded to a byte boundary.
        let covered = b.as_bytes().to_vec();
        if compact {
            b.push_bits(crc16_ccitt(&covered) as u32, 16);
        } else {
            b.push_bits(crc32_iso_hdlc(&covered), 32);
        }
        Ok(b)
    }

    /// Parse a frame from the head of a bit stream, returning it and the bits consumed.
    ///
    /// A CRC failure is [`LcrError::CrcMismatch`] and nothing is returned — a caller
    /// cannot accidentally use a body that failed its integrity check.
    pub fn from_bits(bits: &BitVec) -> Result<(Frame, usize), LcrError> {
        if bits.len() < 15 {
            return Err(LcrError::BadFrame);
        }
        let extended = bits.get(0);
        let ctype = ContentType::from_u8(bits.read_bits(1, 4) as u8)?;
        let cflag = Compression::from_u8(bits.read_bits(5, 2) as u8);
        let (len_bits, crc_bits) = if extended { (14u32, 32u32) } else { (8, 16) };
        let header_bits = 7 + len_bits as usize;
        if bits.len() < header_bits {
            return Err(LcrError::BadFrame);
        }
        let n = bits.read_bits(7, len_bits) as usize;
        let total = header_bits + n * 8 + crc_bits as usize;
        if bits.len() < total {
            return Err(LcrError::BadFrame);
        }

        let body = bits.to_bytes_from(header_bits, n);
        let mut covered = BitVec::with_capacity(header_bits + n * 8);
        for i in 0..header_bits + n * 8 {
            covered.push(bits.get(i));
        }
        let want = bits.read_bits(header_bits + n * 8, crc_bits);
        let got = if extended {
            crc32_iso_hdlc(covered.as_bytes())
        } else {
            crc16_ccitt(covered.as_bytes()) as u32
        };
        if got != want {
            return Err(LcrError::CrcMismatch);
        }
        Ok((Frame { ctype, cflag, body }, total))
    }

    /// The application bytes: the body with compression undone.
    pub fn payload(&self) -> Result<Vec<u8>, LcrError> {
        decompress(self.cflag, &self.body)
    }

    /// Frame `data`, compressing only if that actually makes the symbol smaller.
    ///
    /// §9.3 warns that at these sizes a general-purpose compressor often *expands* its
    /// input, so this tries every codec the build supports and keeps the smallest result,
    /// including no compression at all.
    pub fn compressed(ctype: ContentType, data: &[u8]) -> Result<Frame, LcrError> {
        // The length field counts the **post-compression** body, so what has to fit
        // `EXTENDED_MAX` is the candidate, not the input. Building the uncompressed frame
        // first and letting `Frame::new` reject it meant a payload was turned away for the
        // size it arrived at rather than the size it would occupy: a 50 KB document that
        // deflates to 3 KB was refused, though the frame it would have produced is legal
        // and well inside a symbol's capacity.
        if data.len() > MAX_PAYLOAD {
            return Err(LcrError::TooLarge(data.len()));
        }
        let mut best = (data.len() <= EXTENDED_MAX).then(|| Frame {
            ctype,
            cflag: Compression::None,
            body: data.to_vec(),
        });
        for cflag in [Compression::Deflate, Compression::Brotli] {
            let Ok(candidate) = compress(cflag, data) else {
                continue;
            };
            if candidate.len() <= EXTENDED_MAX
                && best.as_ref().is_none_or(|b| candidate.len() < b.body.len())
            {
                best = Some(Frame {
                    ctype,
                    cflag,
                    body: candidate,
                });
            }
        }
        best.ok_or(LcrError::TooLarge(data.len()))
    }
}

/// Encode a §9.5 multipart body from typed parts.
pub fn multipart_encode(parts: &[(ContentType, Vec<u8>)]) -> Result<Vec<u8>, LcrError> {
    if parts.len() > 255 {
        return Err(LcrError::TooLarge(parts.len()));
    }
    let mut b = BitVec::new();
    b.push_bits(parts.len() as u32, 8);
    for (ctype, body) in parts {
        if body.len() > EXTENDED_MAX {
            return Err(LcrError::TooLarge(body.len()));
        }
        b.push_bits(ctype.to_u8() as u32, 4);
        b.push_bits(body.len() as u32, 14);
        b.push_bytes(body);
    }
    Ok(b.into_bytes())
}

/// Decode a §9.5 multipart body.
pub fn multipart_decode(body: &[u8]) -> Result<Vec<(ContentType, Vec<u8>)>, LcrError> {
    let bits = BitVec::from_bytes(body);
    if bits.len() < 8 {
        return Err(LcrError::BadFrame);
    }
    let count = bits.read_bits(0, 8) as usize;
    let mut pos = 8usize;
    let mut out = Vec::with_capacity(count.min(256));
    for _ in 0..count {
        if pos + 18 > bits.len() {
            return Err(LcrError::BadFrame);
        }
        let ctype = ContentType::from_u8(bits.read_bits(pos, 4) as u8)?;
        let n = bits.read_bits(pos + 4, 14) as usize;
        pos += 18;
        if pos + n * 8 > bits.len() {
            return Err(LcrError::BadFrame);
        }
        out.push((ctype, bits.to_bytes_from(pos, n)));
        pos += n * 8;
    }
    Ok(out)
}

/// Compress with the named codec.
pub fn compress(cflag: Compression, data: &[u8]) -> Result<Vec<u8>, LcrError> {
    match cflag {
        Compression::None => Ok(data.to_vec()),
        #[cfg(feature = "deflate")]
        Compression::Deflate => Ok(miniz_oxide::deflate::compress_to_vec(data, 10)),
        #[cfg(not(feature = "deflate"))]
        Compression::Deflate => Err(LcrError::UnsupportedCompression(cflag.to_u8())),
        #[cfg(feature = "brotli")]
        Compression::Brotli => {
            let mut out = Vec::new();
            let mut input = data;
            brotli::BrotliCompress(
                &mut input,
                &mut out,
                &brotli::enc::BrotliEncoderParams {
                    quality: 11,
                    lgwin: 22,
                    ..Default::default()
                },
            )
            .map_err(|_| LcrError::CompressionFailed)?;
            Ok(out)
        }
        #[cfg(not(feature = "brotli"))]
        Compression::Brotli => Err(LcrError::UnsupportedCompression(cflag.to_u8())),
        Compression::BrotliPreset => Err(LcrError::UnsupportedCompression(cflag.to_u8())),
    }
}

/// Reverse [`compress`].
pub fn decompress(cflag: Compression, data: &[u8]) -> Result<Vec<u8>, LcrError> {
    match cflag {
        Compression::None => Ok(data.to_vec()),
        #[cfg(feature = "deflate")]
        Compression::Deflate => {
            miniz_oxide::inflate::decompress_to_vec_with_limit(data, MAX_PAYLOAD)
                .map_err(|_| LcrError::CompressionFailed)
        }
        #[cfg(not(feature = "deflate"))]
        Compression::Deflate => Err(LcrError::UnsupportedCompression(cflag.to_u8())),
        #[cfg(feature = "brotli")]
        Compression::Brotli => {
            let mut out = Vec::new();
            let mut input = data;
            brotli::BrotliDecompress(&mut input, &mut out)
                .map_err(|_| LcrError::CompressionFailed)?;
            Ok(out)
        }
        #[cfg(not(feature = "brotli"))]
        Compression::Brotli => Err(LcrError::UnsupportedCompression(cflag.to_u8())),
        Compression::BrotliPreset => Err(LcrError::UnsupportedCompression(cflag.to_u8())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn compact_overhead_matches_spec_9_1() {
        // §9.1: 15 bits + 2 bytes ≈ 3.9 bytes.
        assert_eq!(Frame::overhead_bits(10), 31);
        assert!((Frame::overhead_bits(10) as f32 / 8.0 - 3.875).abs() < 1e-6);
        // Extended: 21 bits + 4 bytes ≈ 6.6 bytes.
        assert_eq!(Frame::overhead_bits(300), 53);
        assert!((Frame::overhead_bits(300) as f32 / 8.0 - 6.625).abs() < 1e-6);
    }

    /// §9.1's "40% versus 28%" is overhead as a fraction of the *framed total*, not of
    /// the body. Both readings are defensible; this pins the one that reproduces the
    /// spec's numbers, so nobody has to re-derive it from the prose again.
    #[test]
    fn ten_byte_payload_costs_28_percent_not_40() {
        let frac = |bits: usize| bits as f32 / (bits + 80) as f32;
        assert!(
            (frac(Frame::overhead_bits(10)) - 0.28).abs() < 0.005,
            "{}",
            frac(31)
        );
        assert!((frac(53) - 0.40).abs() < 0.005, "{}", frac(53));
    }

    #[test]
    fn frames_roundtrip_at_every_size_boundary() {
        for n in [0usize, 1, 10, 254, 255, 256, 1000, EXTENDED_MAX] {
            let body: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
            let f = Frame::new(ContentType::Cbor, body.clone()).unwrap();
            let bits = f.to_bits().unwrap();
            assert_eq!(bits.len(), f.encoded_bits());
            let (got, used) = Frame::from_bits(&bits).unwrap();
            assert_eq!(got, f, "n={n}");
            assert_eq!(used, bits.len());
            assert_eq!(got.is_compact(), n <= COMPACT_MAX);
        }
    }

    #[test]
    fn the_compact_frame_is_used_whenever_it_fits() {
        assert!(
            Frame::new(ContentType::Opaque, vec![0; 255])
                .unwrap()
                .is_compact()
        );
        assert!(
            !Frame::new(ContentType::Opaque, vec![0; 256])
                .unwrap()
                .is_compact()
        );
    }

    #[test]
    fn oversized_bodies_are_rejected() {
        assert!(matches!(
            Frame::new(ContentType::Opaque, vec![0; EXTENDED_MAX + 1]),
            Err(LcrError::TooLarge(_))
        ));
    }

    #[test]
    fn a_single_bit_flip_is_caught_by_the_crc() {
        let f = Frame::new(ContentType::Text, b"hello world".to_vec()).unwrap();
        let bits = f.to_bits().unwrap();
        for i in 0..bits.len() {
            let mut damaged = bits.clone();
            damaged.set(i, !damaged.get(i));
            match Frame::from_bits(&damaged) {
                Err(_) => {}
                Ok((got, _)) => panic!("flip at bit {i} produced a valid frame {got:?}"),
            }
        }
    }

    #[test]
    fn reserved_content_types_are_rejected() {
        for v in 6..=15u8 {
            assert!(matches!(
                ContentType::from_u8(v),
                Err(LcrError::BadContentType(_))
            ));
        }
        for c in [
            ContentType::Opaque,
            ContentType::Text,
            ContentType::Json,
            ContentType::Cbor,
            ContentType::CoseSign1,
            ContentType::Multipart,
        ] {
            assert_eq!(ContentType::from_u8(c.to_u8()).unwrap(), c);
        }
    }

    #[test]
    fn truncated_frames_are_rejected_not_panicked() {
        let f = Frame::new(ContentType::Cbor, vec![1, 2, 3, 4, 5]).unwrap();
        let bits = f.to_bits().unwrap();
        for n in 0..bits.len() {
            let mut t = bits.clone();
            t.truncate(n);
            assert!(Frame::from_bits(&t).is_err(), "truncation to {n} accepted");
        }
    }

    #[test]
    fn multipart_roundtrips() {
        let parts = vec![
            (ContentType::CoseSign1, vec![1u8, 2, 3]),
            (ContentType::Cbor, vec![4u8; 300]),
            (ContentType::Text, b"note".to_vec()),
        ];
        let body = multipart_encode(&parts).unwrap();
        assert_eq!(multipart_decode(&body).unwrap(), parts);
    }

    #[test]
    fn multipart_rejects_truncation() {
        let body = multipart_encode(&[(ContentType::Cbor, vec![1, 2, 3])]).unwrap();
        for n in 0..body.len() {
            assert!(
                multipart_decode(&body[..n]).is_err(),
                "truncation to {n} accepted"
            );
        }
    }

    #[cfg(feature = "deflate")]
    #[test]
    fn compression_roundtrips_and_is_only_used_when_it_helps() {
        // Highly compressible: expect the codec to win.
        let big = vec![b'a'; 2000];
        let f = Frame::compressed(ContentType::Text, &big).unwrap();
        assert_ne!(f.cflag, Compression::None);
        assert!(f.body.len() < big.len() / 4);
        assert_eq!(f.payload().unwrap(), big);

        // Short and incompressible: §9.3 warns this is the common case at LCR Code sizes.
        let small: Vec<u8> = (0..20u8)
            .map(|i| i.wrapping_mul(97).wrapping_add(13))
            .collect();
        let f = Frame::compressed(ContentType::Opaque, &small).unwrap();
        assert_eq!(
            f.cflag,
            Compression::None,
            "compressor expanded a short payload"
        );
        assert_eq!(f.payload().unwrap(), small);
    }

    #[test]
    fn the_preset_dictionary_is_not_silently_accepted() {
        assert!(matches!(
            decompress(Compression::BrotliPreset, &[1, 2, 3]),
            Err(LcrError::UnsupportedCompression(0b11))
        ));
    }
}
