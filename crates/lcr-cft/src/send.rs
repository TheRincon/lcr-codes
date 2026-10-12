//! The sending half: compress, chunk, describe (CFT.md §§5–6).

use crate::{CftError, Chunk, Compression, HashAlg, Manifest};
use alloc::string::String;
use alloc::vec::Vec;

/// A prepared transfer: one manifest plus its chunks, ready for any transport.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Transfer {
    /// Describes the whole transfer.
    pub manifest: Manifest,
    /// The chunks, in index order.
    pub chunks: Vec<Chunk>,
}

impl Transfer {
    /// Every message, manifest first — the order a receiver most wants them in.
    ///
    /// A receiver can take them in any order (CFT.md §9), but a manifest that arrives
    /// first lets the receiver validate each chunk as it lands rather than hoarding
    /// unvalidated ones.
    pub fn messages(&self) -> Vec<Vec<u8>> {
        let mut out = Vec::with_capacity(self.chunks.len() + 1);
        out.push(self.manifest.to_vec());
        out.extend(self.chunks.iter().map(Chunk::to_vec));
        out
    }

    /// How many messages the transfer takes, manifest included.
    pub fn message_count(&self) -> usize {
        self.chunks.len() + 1
    }
}

/// Compress `file` and split it into chunks of at most `chunk_size` bytes.
///
/// `transfer_id` identifies this transfer (CFT.md §8); it should be random enough not to
/// collide with a concurrent or repeated one. CFT has no opinion about where randomness
/// comes from, and this crate deliberately pulls in no RNG — verification-side code in
/// this workspace is kept free of `getrandom` (§14.2) and a sender that wants a random ID
/// already has a source.
///
/// **[CFT.md §7]** `chunk_size` is a parameter rather than the document's fixed 4096. For
/// an LCR Code carrier it is the symbol's capacity, which tops out at 4,868 bytes (v32,
/// profile C3) — so 4096 is only reachable from v29 upward, and a smaller version needs a
/// smaller chunk.
pub fn split(
    file: &[u8],
    filename: Option<String>,
    transfer_id: Vec<u8>,
    compression: Compression,
    chunk_size: u32,
) -> Result<Transfer, CftError> {
    if chunk_size == 0 || transfer_id.is_empty() {
        return Err(CftError::Malformed);
    }
    if !compression.available() {
        return Err(CftError::Unsupported(compression.to_u8()));
    }

    // CFT.md §12: the hash covers the *original* file, so the check spans compression,
    // chunking, transport, reassembly and decompression in one comparison.
    let hash_alg = HashAlg::default();
    let hash = hash_alg.digest(file);

    // CFT.md §5: one stream for the whole file, chunked afterwards. Chunks are therefore
    // not independently decompressible, which is the point — per-chunk compression would
    // cost most of the ratio at these sizes.
    let stream = compression.compress(file)?;

    // One message when the whole stream fits beside the metadata: same filename, same
    // hash, same verification — one symbol instead of two.
    if stream.len() <= chunk_size as usize {
        return Ok(Transfer {
            manifest: Manifest {
                transfer_id,
                filename,
                original_size: file.len() as u64,
                compressed_size: stream.len() as u64,
                hash_alg,
                hash,
                chunk_size,
                compression,
                total_chunks: 1,
                inline: Some(stream),
            },
            chunks: Vec::new(),
        });
    }

    let chunks: Vec<Chunk> = stream
        .chunks(chunk_size as usize)
        .enumerate()
        .map(|(i, payload)| Chunk {
            transfer_id: transfer_id.clone(),
            index: i as u32,
            total: 0, // filled in below, once the count is known
            payload: payload.to_vec(),
        })
        .collect();

    let total = u32::try_from(chunks.len()).map_err(|_| CftError::TooLarge {
        claimed: chunks.len() as u64,
        limit: u32::MAX as u64,
    })?;
    // An empty file still needs one (empty) chunk, so the manifest's count and the
    // chunk list never disagree and `implied_chunks` stays exact.
    let chunks = if chunks.is_empty() {
        alloc::vec![Chunk {
            transfer_id: transfer_id.clone(),
            index: 0,
            total: 1,
            payload: Vec::new(),
        }]
    } else {
        chunks.into_iter().map(|c| Chunk { total, ..c }).collect()
    };
    let total = chunks.len() as u32;

    let manifest = Manifest {
        transfer_id,
        filename,
        original_size: file.len() as u64,
        compressed_size: stream.len() as u64,
        hash_alg,
        hash,
        chunk_size,
        compression,
        total_chunks: total,
        inline: None,
    };

    Ok(Transfer { manifest, chunks })
}
