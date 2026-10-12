//! The manifest, CFT.md §3.

use crate::{CftError, Compression, Limits, MAGIC};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use lcr_core::cbor::Value;
use sha2::{Digest, Sha256};

/// Field keys. Integers, per CFT.md §3's compactness argument.
mod key {
    /// Protocol magic — and the manifest/chunk discriminator.
    pub const MAGIC: i64 = 0;
    pub const FILENAME: i64 = 1;
    pub const ORIGINAL_SIZE: i64 = 2;
    pub const COMPRESSED_SIZE: i64 = 3;
    pub const HASH_ALG: i64 = 4;
    pub const HASH: i64 = 5;
    pub const CHUNK_SIZE: i64 = 6;
    pub const COMPRESSION: i64 = 7;
    /// **[CFT.md §3]** Added: the document gives only chunks a transfer ID.
    pub const TRANSFER_ID: i64 = 8;
    /// **[CFT.md §3]** Added: authoritative chunk count.
    pub const TOTAL_CHUNKS: i64 = 9;
    /// **[CFT.md §3]** Added: the whole compressed stream, when it fits beside the
    /// metadata. Makes a small transfer one message instead of two.
    pub const INLINE: i64 = 10;
}

/// Hash algorithm for the end-to-end check (CFT.md §11).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub enum HashAlg {
    /// SHA-256. The document's example and the only algorithm this version defines.
    #[default]
    Sha256 = 0,
}

impl HashAlg {
    /// Wire value.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// Inverse of [`HashAlg::to_u8`].
    pub const fn from_u8(v: u8) -> Result<HashAlg, CftError> {
        match v {
            0 => Ok(HashAlg::Sha256),
            other => Err(CftError::Unsupported(other)),
        }
    }

    /// Digest length in bytes.
    pub const fn digest_len(self) -> usize {
        match self {
            HashAlg::Sha256 => 32,
        }
    }

    /// Hash a complete file.
    pub fn digest(self, data: &[u8]) -> Vec<u8> {
        match self {
            HashAlg::Sha256 => Sha256::digest(data).to_vec(),
        }
    }
}

/// What a receiver needs before it can make sense of any chunk.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Manifest {
    /// Identifies every chunk of this transfer (CFT.md §8).
    ///
    /// **[CFT.md §8]** Narrowed to a byte string. The document allows "integer or byte
    /// string"; requiring bytes is both what §8 calls "more robust" and what makes the
    /// manifest/chunk discriminator in [`crate::chunk`] unambiguous.
    pub transfer_id: Vec<u8>,
    /// Original filename, as the sender described it. Untrusted — see
    /// [`Manifest::safe_filename`].
    pub filename: Option<String>,
    /// Size of the original, uncompressed file.
    pub original_size: u64,
    /// Size of the compressed stream, which is what gets chunked.
    pub compressed_size: u64,
    /// Which hash the digest below is.
    pub hash_alg: HashAlg,
    /// Digest of the **original, uncompressed** file (CFT.md §12).
    pub hash: Vec<u8>,
    /// Payload bytes per chunk. The final chunk may be shorter.
    pub chunk_size: u32,
    /// How the stream was compressed.
    pub compression: Compression,
    /// How many chunks there are. Authoritative; chunks restating it must agree.
    pub total_chunks: u32,
    /// The entire compressed stream, carried in the manifest itself.
    ///
    /// **Not in CFT.md.** The document always sends a manifest *and* at least one chunk,
    /// so the smallest possible transfer costs two messages — two symbols, two scans —
    /// even for a file that would fit in one. Worse, the obvious workaround is to send
    /// the file as raw bytes with no manifest at all, which silently drops the filename
    /// and the end-to-end hash: the two things §3 and §11 exist to provide.
    ///
    /// When present, `total_chunks` is 1, there are no separate chunk messages, and the
    /// length must equal `compressed_size`. Everything else — hash, filename, verify —
    /// works exactly as it does for a chunked transfer.
    pub inline: Option<Vec<u8>>,
}

impl Manifest {
    /// Serialise to CBOR.
    pub fn to_vec(&self) -> Vec<u8> {
        let mut entries =
            alloc::vec![(Value::from_i64(key::MAGIC), Value::Text(MAGIC.to_string()),)];
        if let Some(name) = &self.filename {
            entries.push((Value::from_i64(key::FILENAME), Value::Text(name.clone())));
        }
        entries.extend([
            (
                Value::from_i64(key::ORIGINAL_SIZE),
                Value::Uint(self.original_size),
            ),
            (
                Value::from_i64(key::COMPRESSED_SIZE),
                Value::Uint(self.compressed_size),
            ),
            (
                Value::from_i64(key::HASH_ALG),
                Value::Uint(self.hash_alg.to_u8() as u64),
            ),
            (Value::from_i64(key::HASH), Value::Bytes(self.hash.clone())),
            (
                Value::from_i64(key::CHUNK_SIZE),
                Value::Uint(self.chunk_size as u64),
            ),
            (
                Value::from_i64(key::COMPRESSION),
                Value::Uint(self.compression.to_u8() as u64),
            ),
            (
                Value::from_i64(key::TRANSFER_ID),
                Value::Bytes(self.transfer_id.clone()),
            ),
            (
                Value::from_i64(key::TOTAL_CHUNKS),
                Value::Uint(self.total_chunks as u64),
            ),
        ]);
        if let Some(inline) = &self.inline {
            entries.push((Value::from_i64(key::INLINE), Value::Bytes(inline.clone())));
        }
        Value::Map(entries).to_vec()
    }

    /// Parse a manifest, enforcing `limits` on everything it claims.
    ///
    /// Sizes are checked *before* anything is allocated from them, which is the whole
    /// reason [`Limits`] exists — a `u64` from a camera is not a capacity hint.
    pub fn from_slice(bytes: &[u8], limits: &Limits) -> Result<Manifest, CftError> {
        let value = Value::from_slice(bytes).map_err(|_| CftError::Malformed)?;
        let map = value.as_map().ok_or(CftError::Malformed)?;
        if has_duplicate_keys(map) {
            return Err(CftError::Malformed);
        }

        // CFT.md §4: reject versions we do not understand rather than guessing.
        match value.get_int_key(key::MAGIC) {
            Some(Value::Text(m)) if m == MAGIC => {}
            Some(Value::Text(other)) => return Err(CftError::UnknownVersion(other.clone())),
            Some(_) => return Err(CftError::Indeterminate),
            None => return Err(CftError::MissingField(key::MAGIC as u8)),
        }

        let filename = match value.get_int_key(key::FILENAME) {
            Some(Value::Text(t)) => Some(t.clone()),
            Some(_) => return Err(CftError::BadField(key::FILENAME as u8)),
            None => None,
        };

        let original_size = uint(&value, key::ORIGINAL_SIZE)?;
        bound(original_size, limits.max_original)?;
        let compressed_size = uint(&value, key::COMPRESSED_SIZE)?;
        bound(compressed_size, limits.max_compressed)?;

        let hash_alg = HashAlg::from_u8(small(&value, key::HASH_ALG)?)?;
        let hash = match value.get_int_key(key::HASH) {
            Some(Value::Bytes(b)) if b.len() == hash_alg.digest_len() => b.clone(),
            Some(_) => return Err(CftError::BadField(key::HASH as u8)),
            None => return Err(CftError::MissingField(key::HASH as u8)),
        };

        let chunk_size = uint(&value, key::CHUNK_SIZE)?;
        if chunk_size == 0 || chunk_size > limits.max_chunk as u64 {
            return Err(CftError::BadField(key::CHUNK_SIZE as u8));
        }

        let compression = Compression::from_u8(small(&value, key::COMPRESSION)?)?;

        let transfer_id = match value.get_int_key(key::TRANSFER_ID) {
            Some(Value::Bytes(b)) if !b.is_empty() => b.clone(),
            Some(_) => return Err(CftError::BadField(key::TRANSFER_ID as u8)),
            None => return Err(CftError::MissingField(key::TRANSFER_ID as u8)),
        };

        let total_chunks = uint(&value, key::TOTAL_CHUNKS)?;
        if total_chunks == 0 || total_chunks > limits.max_chunks as u64 {
            return Err(CftError::BadField(key::TOTAL_CHUNKS as u8));
        }

        let inline = match value.get_int_key(key::INLINE) {
            Some(Value::Bytes(b)) => Some(b.clone()),
            Some(_) => return Err(CftError::BadField(key::INLINE as u8)),
            None => None,
        };

        let manifest = Manifest {
            transfer_id,
            filename,
            original_size,
            compressed_size,
            hash_alg,
            hash,
            chunk_size: chunk_size as u32,
            compression,
            total_chunks: total_chunks as u32,
            inline,
        };
        manifest.check_self_consistent()?;
        Ok(manifest)
    }

    /// The chunk count the sizes imply — which must equal [`Manifest::total_chunks`].
    ///
    /// **[CFT.md §3/§6]** The document carries this information twice over without saying
    /// which wins. Checking them against each other on parse means a receiver never has
    /// to choose.
    pub fn implied_chunks(&self) -> u64 {
        if self.chunk_size == 0 {
            return 0;
        }
        // An empty stream still takes one (empty) chunk: a transfer of zero chunks has
        // no terminating condition a receiver could ever satisfy.
        self.compressed_size.div_ceil(self.chunk_size as u64).max(1)
    }

    fn check_self_consistent(&self) -> Result<(), CftError> {
        if self.implied_chunks() != self.total_chunks as u64 {
            return Err(CftError::BadField(key::TOTAL_CHUNKS as u8));
        }
        if let Some(inline) = &self.inline {
            // An inline transfer is one message by definition, and the bytes it carries
            // must be the whole stream it claims.
            if self.total_chunks != 1 || inline.len() as u64 != self.compressed_size {
                return Err(CftError::BadField(key::INLINE as u8));
            }
        }
        Ok(())
    }

    /// Expected payload length of chunk `index`, the last one being short.
    pub fn chunk_len(&self, index: u32) -> Option<usize> {
        if index >= self.total_chunks {
            return None;
        }
        let start = index as u64 * self.chunk_size as u64;
        let remaining = self.compressed_size.saturating_sub(start);
        Some(remaining.min(self.chunk_size as u64) as usize)
    }

    /// The filename, if it is safe to use as one.
    ///
    /// **Not in CFT.md, and it needs to be.** §3 carries a filename straight from the
    /// sender and the document never says what a receiver may do with it. A receiver that
    /// writes to it unchecked accepts `../../etc/passwd`, an absolute path, a Windows
    /// drive letter, or an embedded NUL. This returns `None` for anything that is not a
    /// single, plain, relative path component:
    ///
    /// * empty, `.` or `..`
    /// * contains `/`, `\`, or a NUL / control character
    /// * begins with `-` (so it cannot be read as a command-line flag)
    ///
    /// Callers that want a filename anyway should generate their own and treat this one
    /// as a display hint.
    pub fn safe_filename(&self) -> Option<&str> {
        let name = self.filename.as_deref()?;
        if name.is_empty() || name == "." || name == ".." {
            return None;
        }
        if name.starts_with('-') {
            return None;
        }
        if name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
        {
            return None;
        }
        Some(name)
    }

    /// Check a reconstructed file against the manifest (CFT.md §11).
    ///
    /// Size first, then digest: a mismatched length is the cheaper rejection and says
    /// something more specific about what went wrong.
    pub fn verify(&self, file: &[u8]) -> Result<(), CftError> {
        if file.len() as u64 != self.original_size {
            return Err(CftError::SizeMismatch {
                expected: self.original_size,
                actual: file.len() as u64,
            });
        }
        if self.hash_alg.digest(file) != self.hash {
            return Err(CftError::HashMismatch);
        }
        Ok(())
    }
}

fn uint(value: &Value, k: i64) -> Result<u64, CftError> {
    match value.get_int_key(k) {
        Some(Value::Uint(u)) => Ok(*u),
        Some(_) => Err(CftError::BadField(k as u8)),
        None => Err(CftError::MissingField(k as u8)),
    }
}

fn small(value: &Value, k: i64) -> Result<u8, CftError> {
    u8::try_from(uint(value, k)?).map_err(|_| CftError::BadField(k as u8))
}

fn bound(claimed: u64, limit: u64) -> Result<(), CftError> {
    if claimed > limit {
        return Err(CftError::TooLarge { claimed, limit });
    }
    Ok(())
}

pub(crate) fn has_duplicate_keys(map: &[(Value, Value)]) -> bool {
    for i in 0..map.len() {
        for j in (i + 1)..map.len() {
            if map[i].0 == map[j].0 {
                return true;
            }
        }
    }
    false
}
