//! CFT — CBOR File Transfer.
//!
//! Implements `docs/CFT.md`: a file is compressed as one stream, split into numbered
//! chunks, each chunk wrapped in a CBOR map, and the whole thing verified end to end by
//! a hash of the *original* file. The transport is deliberately not CFT's business —
//! LCR Code symbols are one carrier, BLE and serial are others.
//!
//! ```text
//! file → compress → one stream → chunks → CBOR → transport
//!                                                    ↓
//! file ← verify hash ← decompress ← reassemble ← CBOR
//! ```
//!
//! ## Deviations from `docs/CFT.md`
//!
//! The document is a design sketch and three of its gaps are load-bearing, so this
//! implementation resolves them rather than reproducing them. Each is marked **[CFT.md
//! §n]** where it appears in the source.
//!
//! 1. **The manifest carries the transfer ID** (key 8). In the document only chunks have
//!    one, so nothing binds a manifest to the chunks it describes and two concurrent
//!    transfers cannot be told apart.
//! 2. **The manifest carries the authoritative chunk count** (key 9). The document lets
//!    it be derived from `compressed_size / chunk_size` *and* restated in every chunk;
//!    two sources of truth that can disagree. Here the manifest wins and a chunk whose
//!    `total` disagrees is rejected.
//! 3. **Manifest and chunk are explicitly discriminated.** Both are CBOR maps keyed from
//!    zero, so the document gives a receiver no way to tell them apart. The rule here:
//!    *key 0 is a text string in a manifest and a byte string in a chunk.* That costs no
//!    bytes and cannot be spoofed by a well-formed message of the other kind. It does
//!    narrow §8's "integer or byte string" transfer ID to byte string only — which §8
//!    already calls the more robust choice.
//!
//! Two further changes, both about size:
//!
//! 4. **Enumerated values are integers, not text.** The document says integer keys keep
//!    CBOR compact and then spends ~17 bytes on `"CFT1"`, `"sha256"` and `"zstd"`. Hash
//!    and compression are small integers here. The `"CFT1"` magic stays text because it
//!    doubles as the discriminator in (3).
//! 5. **Chunk size is chosen by the caller from the carrier's capacity**, not fixed at
//!    the document's 4096. The largest LCR Code symbol (v32, profile C3) holds 4,868
//!    bytes, so 4096 only fits from v29 upward.
//!
//! ## What CFT's hash does and does not prove
//!
//! The end-to-end hash proves the reconstructed file is **the same file that was sent**.
//! It does not prove *who* sent it. An attacker who controls the transport can replace
//! the manifest and every chunk together, and the hash will check out — it is a
//! corruption detector, not an authenticator. Where origin matters, sign the manifest:
//! `lcr-verify` exists for exactly that, and a CFT manifest is a perfectly good `lcr.body`.
//!
//! ## Untrusted input
//!
//! A manifest arrives from a camera. Its declared sizes are attacker-controlled u64s, so
//! nothing here allocates from them: see [`Limits`].

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod chunk;
pub mod codec;
#[cfg(feature = "fountain")]
pub mod fountain;
pub mod manifest;
pub mod recv;
pub mod send;

pub use chunk::Chunk;
pub use codec::Compression;
#[cfg(feature = "fountain")]
pub use fountain::{Fountain, Repair};
pub use manifest::{HashAlg, Manifest};
pub use recv::{Reassembler, Status};
pub use send::{Transfer, split};

use alloc::string::String;

/// Protocol magic and version, CFT.md §4.
pub const MAGIC: &str = "CFT1";

/// Ceilings applied to anything a manifest claims.
///
/// CFT.md §3 lets a manifest declare a `u64` original size and chunk size. Both arrive
/// from an untrusted transport, so a receiver that sizes a buffer from them is one
/// hostile symbol away from an out-of-memory kill. These bounds are checked *before* any
/// allocation and again against the bytes that actually arrive.
#[derive(Copy, Clone, Debug)]
pub struct Limits {
    /// Largest original file a receiver will attempt. Default 64 MiB.
    pub max_original: u64,
    /// Largest compressed stream a receiver will hold. Default 64 MiB.
    pub max_compressed: u64,
    /// Largest single chunk payload. Default 1 MiB — far above any symbol's capacity.
    pub max_chunk: usize,
    /// Largest chunk count. Default 1,048,576.
    pub max_chunks: u32,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_original: 64 << 20,
            max_compressed: 64 << 20,
            max_chunk: 1 << 20,
            max_chunks: 1 << 20,
        }
    }
}

/// Everything that can go wrong in a transfer.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum CftError {
    /// The bytes were not valid CBOR, or not the map CFT expects.
    Malformed,
    /// Key 0 was neither a text magic nor a byte-string transfer ID, so the message is
    /// neither a manifest nor a chunk.
    Indeterminate,
    /// A manifest whose magic is not [`MAGIC`].
    ///
    /// CFT.md §4: "Implementations should reject versions they do not understand rather
    /// than silently interpreting them as another version."
    UnknownVersion(String),
    /// A required field was absent.
    MissingField(u8),
    /// A field was present with the wrong CBOR type or an out-of-range value.
    BadField(u8),
    /// The compression or hash algorithm is not one this build supports.
    Unsupported(u8),
    /// A declared size exceeded [`Limits`].
    TooLarge {
        /// What was claimed.
        claimed: u64,
        /// The ceiling it broke.
        limit: u64,
    },
    /// A chunk belongs to a different transfer than the manifest.
    WrongTransfer,
    /// A chunk index at or beyond the manifest's chunk count, or a `total` that
    /// disagrees with the manifest.
    ChunkOutOfRange {
        /// The index the chunk claimed.
        index: u32,
        /// The count the manifest declared.
        total: u32,
    },
    /// The same chunk index arrived twice with different payloads.
    ChunkConflict(u32),
    /// A chunk's payload length is inconsistent with the manifest's chunk size.
    ChunkLength(u32),
    /// Reassembly or decompression was attempted before every chunk arrived.
    Incomplete,
    /// Compression or decompression failed.
    Codec,
    /// Decompressed output did not match the manifest's declared original size — the
    /// decompression-bomb guard.
    SizeMismatch {
        /// What the manifest promised.
        expected: u64,
        /// What came out.
        actual: u64,
    },
    /// The reconstructed file's hash did not match the manifest.
    ///
    /// CFT.md §11: "Only when the hashes match should the reconstructed file be
    /// considered valid."
    HashMismatch,
    /// The filename is not safe to use as a filename (see [`Manifest::safe_filename`]).
    UnsafeFilename,
}

impl core::fmt::Display for CftError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CftError::Malformed => f.write_str("not a well-formed CFT message"),
            CftError::Indeterminate => f.write_str("neither a manifest nor a chunk"),
            CftError::UnknownVersion(v) => write!(f, "unsupported CFT version {v:?}"),
            CftError::MissingField(k) => write!(f, "missing required field {k}"),
            CftError::BadField(k) => write!(f, "field {k} has the wrong type or value"),
            CftError::Unsupported(v) => write!(f, "algorithm {v} is not supported"),
            CftError::TooLarge { claimed, limit } => {
                write!(f, "declared size {claimed} exceeds limit {limit}")
            }
            CftError::WrongTransfer => f.write_str("chunk belongs to another transfer"),
            CftError::ChunkOutOfRange { index, total } => {
                write!(f, "chunk {index} outside a transfer of {total}")
            }
            CftError::ChunkConflict(i) => write!(f, "chunk {i} arrived twice, differing"),
            CftError::ChunkLength(i) => write!(f, "chunk {i} has an inconsistent length"),
            CftError::Incomplete => f.write_str("chunks are still missing"),
            CftError::Codec => f.write_str("compression failed"),
            CftError::SizeMismatch { expected, actual } => {
                write!(
                    f,
                    "decompressed {actual} bytes, manifest promised {expected}"
                )
            }
            CftError::HashMismatch => f.write_str("hash does not match the manifest"),
            CftError::UnsafeFilename => f.write_str("filename is not safe to use"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CftError {}
