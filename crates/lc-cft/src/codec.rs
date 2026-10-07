//! Compression, CFT.md §5.
//!
//! ZALT will be the compression lib. Forthcoming change.

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
            // Level 19, and **not** 20–22. CFT compresses once, offline, so a slow level
            // costs nothing at transfer time and the temptation is to turn it up. Measured
            // on 2026-09-24, 19 → 22 is worth nothing at all:
            //
            //   tikz.pdf      17,992 → 17,992   ( 0.00%)
            //   kino_ticket   98,547 → 98,542   (-0.01%)
            //   5.2 MB manual  5,092,997 → 5,092,993  (+0.00%)
            //   685 KB source    168,545 →   168,543  (-0.00%)
            //
            // Four bytes on five megabytes. `--ultra` also raises the decoder's window to
            // 128 MB, which is a real cost on a phone for no measurable gain.
            Compression::Zstd => zstd::encode_all(data, 19).map_err(|_| CftError::Codec),
            #[cfg(feature = "deflate")]
            // miniz_oxide directly, not `lc_core::frame::compress`. The frame helpers are
            // sized for what one symbol carries; CFT compresses a whole file. See the
            // matching note in `decompress`.
            Compression::Deflate => Ok(miniz_oxide::deflate::compress_to_vec(data, 10)),
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
            // **Not** `lc_core::frame::decompress`: that caps output at `MAX_PAYLOAD`
            // (65,532 bytes), which is the most a single LC Code frame can hold and has
            // nothing to do with the size of a transferred file. Borrowing it silently
            // capped every CFT transfer at 64 KB.
            //
            // The limit here is the manifest's declared original size — already bounded
            // against `Limits` — plus one byte, so an overlong stream is reported as
            // `SizeMismatch` by the check below rather than as an opaque codec failure.
            Compression::Deflate => {
                let cap = usize::try_from(max_output.saturating_add(1)).unwrap_or(usize::MAX);
                miniz_oxide::inflate::decompress_to_vec_with_limit(data, cap)
                    .map_err(|_| CftError::Codec)?
            }
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
