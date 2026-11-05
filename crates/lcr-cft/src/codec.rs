//! Compression, CFT.md §5.
//!
//! Zstandard is `zalt-codec`: pure Rust, `no_std` and free of `unsafe`, so the codec CFT.md
//! prefers is available in every build, the browser's included. It reads and writes standard
//! RFC 8878 frames, so the format is still zstd and any other implementation interoperates.

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
    /// DEFLATE (RFC 1951), via lcr-core's pure-Rust backend.
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
            // Level 19, the highest there is: CFT compresses once, offline, so a slow level
            // costs nothing at transfer time. Against libzstd at the same level, measured on
            // 2026-11-03, it comes out 0–2.5% larger on source, JSON, logs and wasm.
            //
            // `Strategy::Compress` has to be asked for; the default only stores. No content
            // checksum, because the manifest's SHA-256 already covers the file.
            Compression::Zstd => zalt_codec::encode::compress(
                data,
                &zalt_codec::encode::EncodeParams {
                    level: zalt_codec::encode::level::LEVEL_MAX,
                    strategy: zalt_codec::encode::Strategy::Compress,
                    ..Default::default()
                },
            )
            .map_err(|_| CftError::Codec),
            #[cfg(feature = "deflate")]
            // miniz_oxide directly, not `lcr_core::frame::compress`. The frame helpers are
            // sized for what one symbol carries; CFT compresses a whole file. See the
            // matching note in `decompress`.
            Compression::Deflate => Ok(miniz_oxide::deflate::compress_to_vec(data, 10)),
            #[cfg(feature = "brotli")]
            Compression::Brotli => lcr_core::frame::compress(lcr_core::Compression::Brotli, data)
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
            Compression::Zstd => decompress_zstd(data, max_output)?,
            #[cfg(feature = "deflate")]
            // **Not** `lcr_core::frame::decompress`: that caps output at `MAX_PAYLOAD`
            // (65,532 bytes), which is the most a single LCR Code frame can hold and has
            // nothing to do with the size of a transferred file.
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
            Compression::Brotli => lcr_core::frame::decompress(lcr_core::Compression::Brotli, data)
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

/// The largest window a received stream may declare: 8 MiB, RFC 8878's interop floor and the
/// most any level of our own encoder uses. Checked before the window is allocated, so a
/// hostile manifest's stream cannot ask a phone for gigabytes in one header.
#[cfg(feature = "zstd")]
const MAX_WINDOW_LOG: u8 = 23;

/// Decode a whole stream, bounded by `max_output`.
///
/// **A loop, not `zalt_codec::decode::decompress`.** Its `max_decoded_size` counts per frame
/// and RFC 8878 allows frames to be concatenated, so a 43 KB stream of 1,024 frames decodes
/// to 1 GiB under a 1 MiB limit (docs/zalt-decoded-size-gap.md). Each call here returns at a
/// frame boundary with that frame capped, so checking the running total after each one
/// holds memory to about twice `max_output`.
///
/// An overlong stream is `SizeMismatch`, as with the other codecs, not an opaque codec error.
#[cfg(feature = "zstd")]
fn decompress_zstd(data: &[u8], max_output: u64) -> Result<Vec<u8>, CftError> {
    use zalt_codec::decode::{ScalarDecoder, Status};

    let cap = max_output.saturating_add(1);
    let too_long = || CftError::SizeMismatch {
        expected: max_output,
        actual: cap,
    };
    let mut decoder = ScalarDecoder::new(zalt_codec::params::DecodeLimits {
        max_window_log: MAX_WINDOW_LOG,
        max_decoded_size: Some(cap),
    });
    let mut out = Vec::new();
    let mut cursor = 0;
    loop {
        let progress = decoder
            .decompress(&data[cursor..], &mut out)
            .map_err(|e| match e {
                zalt_codec::Error::TooLarge => too_long(),
                _ => CftError::Codec,
            })?;
        cursor += progress.consumed;
        if out.len() as u64 > max_output {
            return Err(too_long());
        }
        match progress.status {
            Status::FrameComplete if cursor >= data.len() => return Ok(out),
            Status::FrameComplete => {}
            // Ended on a skippable frame: complete, unless there were never any frames.
            Status::NeedMoreInput if progress.consumed == 0 => {
                return if cursor > 0 && decoder.is_between_frames() {
                    Ok(out)
                } else {
                    Err(CftError::Codec)
                };
            }
            Status::NeedMoreInput => {}
        }
    }
}
