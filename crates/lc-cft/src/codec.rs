//! Compression, CFT.md §5.
//!
//! ## The C exception, and why it is scoped
//!
//! CFT.md makes Zstandard the codec, and this project grants it an explicit exception to
//! §14.2's "no C dependencies anywhere in the decode path". The exception is real but it
//! is *scoped*, for a mechanical reason rather than a policy one: `zstd-sys` compiles C,
//! and there is no C toolchain targeting `wasm32-unknown-unknown` in this build. So
//! `lc-wasm` must not enable the `zstd` feature — not because the rule forbids it, but
//! because the build would fail.
//!
//! Everything here goes through [`Compression`], so swapping the backing implementation —
//! for a different zstd crate, or a pure-Rust one — is a change to two match arms.

use crate::CftError;
use alloc::vec::Vec;

/// Compression applied to the whole file before chunking (CFT.md §5).
///
/// Wire values are small integers rather than the document's text strings: a manifest may
/// have to fit a small symbol, and `"zstd"` costs five bytes where `3` costs one.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub enum Compression {
    /// Store. Always available, and the honest choice for already-compressed files.
    None = 0,
    /// DEFLATE (RFC 1951), via lc-core's pure-Rust backend.
    Deflate = 1,
    /// Brotli.
    Brotli = 2,
    /// Zstandard — CFT.md's preferred codec.
    #[default]
    Zstd = 3,
}

impl Compression {
    /// Wire value.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// Inverse of [`Compression::to_u8`].
    pub const fn from_u8(v: u8) -> Result<Compression, CftError> {
        Ok(match v {
            0 => Compression::None,
            1 => Compression::Deflate,
            2 => Compression::Brotli,
            3 => Compression::Zstd,
            other => return Err(CftError::Unsupported(other)),
        })
    }

    /// Whether this build can actually use the codec.
    ///
    /// Separate from [`Compression::from_u8`] so a receiver can tell "I do not recognise
    /// this codec" from "I recognise it but was not built with it" — the second is a
    /// packaging problem the user can be told about.
    pub const fn available(self) -> bool {
        match self {
            Compression::None => true,
            Compression::Deflate => cfg!(feature = "deflate"),
            Compression::Brotli => cfg!(feature = "brotli"),
            Compression::Zstd => cfg!(feature = "zstd"),
        }
    }

    /// Compress a whole file into one stream (CFT.md §5: never per chunk).
    pub fn compress(self, data: &[u8]) -> Result<Vec<u8>, CftError> {
        match self {
            Compression::None => Ok(data.to_vec()),
            #[cfg(feature = "zstd")]
            Compression::Zstd => zstd::encode_all(data, 19).map_err(|_| CftError::Codec),
            #[cfg(feature = "deflate")]
            Compression::Deflate => lc_core::frame::compress(lc_core::Compression::Deflate, data)
                .map_err(|_| CftError::Codec),
            #[cfg(feature = "brotli")]
            Compression::Brotli => lc_core::frame::compress(lc_core::Compression::Brotli, data)
                .map_err(|_| CftError::Codec),
            other => Err(CftError::Unsupported(other.to_u8())),
        }
    }

    /// Decompress a reassembled stream.
    ///
    /// `max_output` is the decompression-bomb guard: a small stream can expand without
    /// bound, so the caller passes the manifest's declared original size (already checked
    /// against [`crate::Limits`]) and anything longer is refused rather than allocated.
    pub fn decompress(self, data: &[u8], max_output: u64) -> Result<Vec<u8>, CftError> {
        let out = match self {
            Compression::None => data.to_vec(),
            #[cfg(feature = "zstd")]
            Compression::Zstd => {
                use std::io::Read;
                let mut decoder = zstd::stream::Decoder::new(data).map_err(|_| CftError::Codec)?;
                let mut out = Vec::new();
                // `take` bounds the read at one byte past the promise, so an overlong
                // stream is detected without ever buffering it.
                decoder
                    .by_ref()
                    .take(max_output.saturating_add(1))
                    .read_to_end(&mut out)
                    .map_err(|_| CftError::Codec)?;
                out
            }
            #[cfg(feature = "deflate")]
            Compression::Deflate => lc_core::frame::decompress(lc_core::Compression::Deflate, data)
                .map_err(|_| CftError::Codec)?,
            #[cfg(feature = "brotli")]
            Compression::Brotli => lc_core::frame::decompress(lc_core::Compression::Brotli, data)
                .map_err(|_| CftError::Codec)?,
            other => return Err(CftError::Unsupported(other.to_u8())),
        };
        if out.len() as u64 > max_output {
            return Err(CftError::SizeMismatch {
                expected: max_output,
                actual: out.len() as u64,
            });
        }
        Ok(out)
    }
}
