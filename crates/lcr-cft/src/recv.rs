//! The receiving half: collect, reassemble, decompress, verify (CFT.md §§9–12).

use crate::chunk::{Message, parse};
use crate::{CftError, Chunk, Limits, Manifest};
use alloc::vec::Vec;

/// Where a transfer has got to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Status {
    /// No manifest yet, so arriving chunks cannot be validated or counted.
    AwaitingManifest {
        /// Chunks held so far, which will be re-validated once a manifest lands.
        held: usize,
    },
    /// Manifest in hand, chunks still missing (CFT.md §10).
    Receiving {
        /// How many chunks have arrived.
        have: u32,
        /// How many there are in total.
        total: u32,
    },
    /// Every chunk has arrived; call [`Reassembler::finish`].
    Complete,
}

/// Collects CFT messages in any order and reconstructs the file.
///
/// CFT.md §9: chunks may arrive in any order, so this keeps a slot per index rather than
/// a queue. §10: nothing assumes they all arrive — [`Reassembler::missing`] is what a
/// QR-transport receiver displays so the holder knows which symbols to show again.
///
/// With the `fountain` feature it also takes fountain repair symbols (CFT.md §21) — see
/// [`crate::fountain`]. [`Status`] then counts chunks known by either route, so a caller
/// does not need to know which kind of transfer it is watching. Without the feature a
/// repair symbol is rejected as an out-of-range chunk, which is how any receiver that
/// predates §21 treats one.
#[derive(Clone, Debug)]
pub struct Reassembler {
    pub(crate) limits: Limits,
    pub(crate) manifest: Option<Manifest>,
    pub(crate) slots: Vec<Option<Vec<u8>>>,
    /// Chunks that arrived before the manifest did.
    orphans: Vec<Chunk>,
    pub(crate) have: u32,
    /// The fountain decoder's state.
    #[cfg(feature = "fountain")]
    pub(crate) peel: crate::fountain::Peel,
}

impl Reassembler {
    /// A receiver with the given limits.
    pub fn new(limits: Limits) -> Reassembler {
        Reassembler {
            limits,
            manifest: None,
            slots: Vec::new(),
            orphans: Vec::new(),
            have: 0,
            #[cfg(feature = "fountain")]
            peel: Default::default(),
        }
    }

    /// The manifest, once one has arrived.
    pub fn manifest(&self) -> Option<&Manifest> {
        self.manifest.as_ref()
    }

    /// Feed one CFT message — manifest or chunk, in any order, and with the `fountain`
    /// feature a repair symbol too.
    ///
    /// The kind is decided by [`crate::chunk::parse`], so a caller scanning symbols does
    /// not have to know which it just read.
    pub fn accept(&mut self, bytes: &[u8]) -> Result<Status, CftError> {
        // Checked before `parse`, whose `Message` predates repair symbols and would read
        // one as an out-of-range chunk.
        #[cfg(feature = "fountain")]
        if crate::fountain::is_repair(bytes) {
            return self.accept_repair(crate::Repair::from_slice(bytes, &self.limits)?);
        }
        match parse(bytes, &self.limits)? {
            Message::Manifest(m) => self.accept_manifest(m),
            Message::Chunk(c) => self.accept_chunk(c),
        }
    }

    /// Take a manifest.
    ///
    /// A second, identical manifest is fine — a QR transport will show it repeatedly. A
    /// second, *different* one is a different transfer and is refused, because silently
    /// switching would discard chunks the caller believes are still held.
    pub fn accept_manifest(&mut self, manifest: Manifest) -> Result<Status, CftError> {
        if let Some(existing) = &self.manifest {
            if *existing != manifest {
                return Err(CftError::WrongTransfer);
            }
            return Ok(self.status());
        }
        if !manifest.compression.available() {
            return Err(CftError::Unsupported(manifest.compression.to_u8()));
        }
        self.slots = alloc::vec![None; manifest.total_chunks as usize];
        #[cfg(feature = "fountain")]
        self.peel.start(manifest.total_chunks);
        // An inline manifest carries the stream itself, so the transfer is complete the
        // moment it arrives.
        if let Some(inline) = manifest.inline.clone() {
            self.slots[0] = Some(inline);
            self.have = 1;
        }
        self.manifest = Some(manifest);

        // Re-offer anything that arrived early, now that it can be validated.
        for chunk in core::mem::take(&mut self.orphans) {
            self.accept_chunk(chunk)?;
        }
        #[cfg(feature = "fountain")]
        self.replay_repairs()?;
        Ok(self.status())
    }

    /// Take a chunk.
    pub fn accept_chunk(&mut self, chunk: Chunk) -> Result<Status, CftError> {
        let Some(manifest) = &self.manifest else {
            // Hold it. Without a manifest there is nothing to validate it against, and
            // guessing the transfer from the first chunk seen is how you end up merging
            // two transfers. Once per index: a camera reads each symbol on several frames,
            // and keeping every read grows without bound until the manifest turns up.
            let held = self
                .orphans
                .iter()
                .any(|o| o.index == chunk.index && o.transfer_id == chunk.transfer_id);
            if !held && self.orphans.len() < self.limits.max_chunks as usize {
                self.orphans.push(chunk);
            }
            return Ok(self.status());
        };

        if chunk.transfer_id != manifest.transfer_id {
            return Err(CftError::WrongTransfer);
        }
        // **[CFT.md §6]** The manifest's count is authoritative; a chunk restating a
        // different one is not a chunk of this transfer.
        if chunk.total != manifest.total_chunks || chunk.index >= manifest.total_chunks {
            return Err(CftError::ChunkOutOfRange {
                index: chunk.index,
                total: manifest.total_chunks,
            });
        }
        // The final chunk is short and every other is exactly `chunk_size`. Checking
        // here means a truncated payload is caught now rather than as a hash failure
        // after the whole transfer has been collected.
        if manifest.chunk_len(chunk.index) != Some(chunk.payload.len()) {
            return Err(CftError::ChunkLength(chunk.index));
        }

        match &self.slots[chunk.index as usize] {
            Some(existing) if *existing == chunk.payload => {}
            Some(_) => return Err(CftError::ChunkConflict(chunk.index)),
            None => self.learn(chunk.index, chunk.payload)?,
        }
        Ok(self.status())
    }

    /// Record chunk `index`, newly known. With the `fountain` feature this also resolves
    /// any held repair symbol it was the last unknown of.
    fn learn(&mut self, index: u32, bytes: Vec<u8>) -> Result<(), CftError> {
        #[cfg(feature = "fountain")]
        return self.learn_and_peel(index, bytes);
        #[cfg(not(feature = "fountain"))]
        self.store(index, bytes).map(|_| ())
    }

    /// Put chunk `index` in its slot unless it is already there. Returns whether it was new.
    ///
    /// `bytes` may be longer than the chunk — a fountain-recovered chunk comes at the full
    /// `chunk_size` — in which case the excess must be the zero padding a short final
    /// chunk was XORed with, and is dropped.
    pub(crate) fn store(&mut self, index: u32, mut bytes: Vec<u8>) -> Result<bool, CftError> {
        if self.slots[index as usize].is_some() {
            return Ok(false);
        }
        let len = self
            .manifest
            .as_ref()
            .and_then(|m| m.chunk_len(index))
            .ok_or(CftError::Incomplete)?;
        if bytes.len() < len || bytes[len..].iter().any(|&b| b != 0) {
            return Err(CftError::ChunkLength(index));
        }
        bytes.truncate(len);
        self.slots[index as usize] = Some(bytes);
        self.have += 1;
        Ok(true)
    }

    /// Current state.
    pub fn status(&self) -> Status {
        #[cfg(feature = "fountain")]
        let early_repairs = self.peel.orphans();
        #[cfg(not(feature = "fountain"))]
        let early_repairs = 0;
        match &self.manifest {
            None => Status::AwaitingManifest {
                held: self.orphans.len() + early_repairs,
            },
            Some(m) if self.have == m.total_chunks => Status::Complete,
            Some(m) => Status::Receiving {
                have: self.have,
                total: m.total_chunks,
            },
        }
    }

    /// Indices still outstanding (CFT.md §10).
    ///
    /// Empty before a manifest arrives — without one there is no way to know how many
    /// chunks to expect, which is a different state from "none missing".
    pub fn missing(&self) -> Vec<u32> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_none())
            .map(|(i, _)| i as u32)
            .collect()
    }

    /// Reassemble, decompress and verify (CFT.md §11).
    ///
    /// The order is deliberate and is the whole point of the format: bytes are only
    /// returned after the digest of the reconstructed file matches what the manifest
    /// promised about the *original* one.
    pub fn finish(&self) -> Result<Vec<u8>, CftError> {
        let manifest = self.manifest.as_ref().ok_or(CftError::Incomplete)?;
        if self.have != manifest.total_chunks {
            return Err(CftError::Incomplete);
        }

        let mut stream = Vec::with_capacity(manifest.compressed_size as usize);
        for slot in &self.slots {
            stream.extend_from_slice(slot.as_ref().ok_or(CftError::Incomplete)?);
        }
        if stream.len() as u64 != manifest.compressed_size {
            return Err(CftError::SizeMismatch {
                expected: manifest.compressed_size,
                actual: stream.len() as u64,
            });
        }

        // `original_size` was bounded against `Limits` when the manifest was parsed, so
        // passing it as the decompression ceiling cannot be turned into an unbounded
        // allocation by a crafted stream.
        let file = manifest
            .compression
            .decompress(&stream, manifest.original_size)?;
        manifest.verify(&file)?;
        Ok(file)
    }
}

impl Default for Reassembler {
    fn default() -> Reassembler {
        Reassembler::new(Limits::default())
    }
}
