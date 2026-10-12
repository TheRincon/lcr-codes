//! Data chunks, CFT.md §6.

use crate::manifest::has_duplicate_keys;
use crate::{CftError, Limits};
use alloc::vec::Vec;
use lcr_core::cbor::Value;

mod key {
    /// Transfer ID — and the manifest/chunk discriminator.
    pub const TRANSFER_ID: i64 = 0;
    pub const INDEX: i64 = 1;
    pub const TOTAL: i64 = 2;
    pub const PAYLOAD: i64 = 3;
}

/// One piece of the compressed stream.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Chunk {
    /// Which transfer this belongs to (CFT.md §8).
    pub transfer_id: Vec<u8>,
    /// Zero-based index (CFT.md §9).
    pub index: u32,
    /// Total chunks. Restated per chunk as the document specifies, but the manifest is
    /// authoritative — [`crate::Reassembler`] rejects a chunk that disagrees.
    pub total: u32,
    /// Raw bytes of the compressed stream.
    ///
    /// CFT.md §6: a CBOR byte string, never base64. §9.6 of the LCR Code spec makes the
    /// same point about the symbol body, for the same reason.
    pub payload: Vec<u8>,
}

impl Chunk {
    /// Serialise to CBOR.
    pub fn to_vec(&self) -> Vec<u8> {
        Value::Map(alloc::vec![
            (
                Value::from_i64(key::TRANSFER_ID),
                Value::Bytes(self.transfer_id.clone()),
            ),
            (Value::from_i64(key::INDEX), Value::Uint(self.index as u64)),
            (Value::from_i64(key::TOTAL), Value::Uint(self.total as u64)),
            (
                Value::from_i64(key::PAYLOAD),
                Value::Bytes(self.payload.clone()),
            ),
        ])
        .to_vec()
    }

    /// Parse a chunk.
    pub fn from_slice(bytes: &[u8], limits: &Limits) -> Result<Chunk, CftError> {
        let value = Value::from_slice(bytes).map_err(|_| CftError::Malformed)?;
        let map = value.as_map().ok_or(CftError::Malformed)?;
        if has_duplicate_keys(map) {
            return Err(CftError::Malformed);
        }

        let transfer_id = match value.get_int_key(key::TRANSFER_ID) {
            Some(Value::Bytes(b)) if !b.is_empty() => b.clone(),
            // A text key 0 means this is a manifest, not a chunk.
            Some(Value::Text(_)) => return Err(CftError::Indeterminate),
            Some(_) => return Err(CftError::BadField(key::TRANSFER_ID as u8)),
            None => return Err(CftError::MissingField(key::TRANSFER_ID as u8)),
        };

        let index = u32::try_from(uint(&value, key::INDEX)?)
            .map_err(|_| CftError::BadField(key::INDEX as u8))?;
        let total = u32::try_from(uint(&value, key::TOTAL)?)
            .map_err(|_| CftError::BadField(key::TOTAL as u8))?;
        if total == 0 || total > limits.max_chunks || index >= total {
            return Err(CftError::ChunkOutOfRange { index, total });
        }

        let payload = match value.get_int_key(key::PAYLOAD) {
            Some(Value::Bytes(b)) if b.len() <= limits.max_chunk => b.clone(),
            Some(Value::Bytes(_)) => return Err(CftError::ChunkLength(index)),
            Some(_) => return Err(CftError::BadField(key::PAYLOAD as u8)),
            None => return Err(CftError::MissingField(key::PAYLOAD as u8)),
        };

        Ok(Chunk {
            transfer_id,
            index,
            total,
            payload,
        })
    }
}

/// Either kind of CFT message.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Message {
    /// A manifest.
    Manifest(crate::Manifest),
    /// A data chunk.
    Chunk(Chunk),
}

/// Decide what a CFT message is and parse it.
///
/// **[CFT.md §3/§6]** The document gives no way to tell a manifest from a chunk: both are
/// CBOR maps keyed from zero. The rule: **key 0
/// is a text string in a manifest and a byte string in a chunk**.
pub fn parse(bytes: &[u8], limits: &Limits) -> Result<Message, CftError> {
    let value = Value::from_slice(bytes).map_err(|_| CftError::Malformed)?;
    match value.get_int_key(0) {
        Some(Value::Text(_)) => crate::Manifest::from_slice(bytes, limits).map(Message::Manifest),
        Some(Value::Bytes(_)) => Chunk::from_slice(bytes, limits).map(Message::Chunk),
        _ => Err(CftError::Indeterminate),
    }
}

fn uint(value: &Value, k: i64) -> Result<u64, CftError> {
    match value.get_int_key(k) {
        Some(Value::Uint(u)) => Ok(*u),
        Some(_) => Err(CftError::BadField(k as u8)),
        None => Err(CftError::MissingField(k as u8)),
    }
}
