//! Crate-wide error type.

use core::fmt;

/// Everything that can go wrong inside LCR Code, across encode and decode.
///
/// Kept as one flat enum rather than per-crate errors: the WASM and C boundaries both
/// have to flatten it anyway, and a scanner wants a single discriminant to map to user
/// guidance.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LcrError {
    /// Version outside `1..=32` (§6.1).
    BadVersion(u32),
    /// ECC level outside `0..=3` (§8.2).
    BadEccLevel(u8),
    /// Profile bits did not decode to a known profile (§7).
    BadProfile(u8),
    /// Mask id outside `0..=7` (§8.5).
    BadMask(u8),
    /// Payload does not fit the requested version/profile/ECC combination.
    ///
    /// Carries `(needed_bits, available_bits)` so a caller can size up without a search.
    Capacity {
        /// Bits the framed payload needs.
        needed: usize,
        /// Bits the chosen version/profile/ECC combination provides.
        available: usize,
    },
    /// No version in `1..=32` can hold this payload at the requested settings.
    TooLarge(usize),
    /// Frame header is malformed or truncated (§9.1).
    BadFrame,
    /// Frame CRC did not match (§9.1). This is the authoritative decode reject.
    CrcMismatch,
    /// Content type byte is in the reserved range (§9.2).
    BadContentType(u8),
    /// Compression flag names a scheme this build does not support (§9.3).
    UnsupportedCompression(u8),
    /// Compression or decompression failed.
    CompressionFailed,
    /// Format region failed BCH decode in both copies (§7).
    FormatUndecodable,
    /// LDPC decoding did not converge for the named layer.
    EccFailure {
        /// `true` for the luma layer, `false` for chroma.
        luma: bool,
    },
    /// No symbol was located in the supplied image.
    NotFound,
    /// Finders were located but the geometry is not a plausible symbol.
    BadGeometry,
    /// Supplied pixel buffer is too small for the stated width/height/stride.
    ShortBuffer,
    /// CBOR input is malformed.
    BadCbor,
    /// JSON input is malformed, with a byte offset.
    BadJson(usize),
    /// A signed packet was encountered but this build has no verification support.
    VerifyUnavailable,
    /// Signature verification failed; see `lcr-verify` for the detailed reason.
    VerifyFailed,
    /// Requested feature is recognised by the format but not implemented here.
    Unsupported(&'static str),
}

impl fmt::Display for LcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LcrError::BadVersion(v) => write!(f, "version {v} outside 1..=32"),
            LcrError::BadEccLevel(l) => write!(f, "ECC level {l} outside 0..=3"),
            LcrError::BadProfile(p) => write!(f, "unknown profile bits {p:#04b}"),
            LcrError::BadMask(m) => write!(f, "mask id {m} outside 0..=7"),
            LcrError::Capacity { needed, available } => {
                write!(f, "payload needs {needed} bits, symbol holds {available}")
            }
            LcrError::TooLarge(n) => write!(f, "{n} bytes exceeds the largest LCR Code symbol"),
            LcrError::BadFrame => write!(f, "malformed container frame"),
            LcrError::CrcMismatch => write!(f, "frame CRC mismatch"),
            LcrError::BadContentType(t) => write!(f, "reserved content type {t:#x}"),
            LcrError::UnsupportedCompression(c) => {
                write!(f, "unsupported compression flag {c:#04b}")
            }
            LcrError::CompressionFailed => write!(f, "compression codec failed"),
            LcrError::FormatUndecodable => write!(f, "both format region copies failed BCH decode"),
            LcrError::EccFailure { luma } => {
                write!(
                    f,
                    "{} layer LDPC did not converge",
                    if *luma { "luma" } else { "chroma" }
                )
            }
            LcrError::NotFound => write!(f, "no LCR Code symbol found"),
            LcrError::BadGeometry => write!(f, "finder geometry is not a plausible symbol"),
            LcrError::ShortBuffer => write!(f, "pixel buffer shorter than width*height*stride"),
            LcrError::BadCbor => write!(f, "malformed CBOR"),
            LcrError::BadJson(off) => write!(f, "malformed JSON at byte {off}"),
            LcrError::VerifyUnavailable => write!(f, "signed packet, but this build cannot verify"),
            LcrError::VerifyFailed => write!(f, "signature verification failed"),
            LcrError::Unsupported(what) => write!(f, "unsupported: {what}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for LcrError {}
