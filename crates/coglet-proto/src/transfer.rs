//! Bounded raw-byte credit accounting for git transfer. The sink owns quarantine
//! durability; call `consumed` only after accepted chunks have been durably stored.
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
#[error("invalid git transfer")]
pub struct InvalidTransfer;

pub struct Receiver {
    id: String,
    size: u64,
    hash: Vec<u8>,
    digest: Sha256,
    offset: u64,
    consumed: u64,
    credit: u64,
}

impl Receiver {
    pub fn new(id: &str, size: u64, hash: &str) -> Result<Self, InvalidTransfer> {
        if !(1..=67108864).contains(&size) || !lower_hex(id, 32) || !lower_hex(hash, 64) {
            return Err(InvalidTransfer);
        }
        Ok(Self {
            id: id.into(),
            size,
            hash: hex::decode(hash).map_err(|_| InvalidTransfer)?,
            digest: Sha256::new(),
            offset: 0,
            consumed: 0,
            credit: 0,
        })
    }

    /// Cumulative credit, never more than 64KiB ahead of durable consumption.
    pub fn grant(&mut self, bytes: u64) -> Result<(), InvalidTransfer> {
        if bytes < self.credit || bytes > self.size || bytes.saturating_sub(self.consumed) > 65536 {
            return Err(InvalidTransfer);
        }
        self.credit = bytes;
        Ok(())
    }

    pub fn chunk(&mut self, frame: &Value) -> Result<Vec<u8>, InvalidTransfer> {
        self.frame("hub-chunk", frame)?;
        let offset = frame["offset"].as_u64().ok_or(InvalidTransfer)?;
        let bytes = hex::decode(frame["hex"].as_str().ok_or(InvalidTransfer)?)
            .map_err(|_| InvalidTransfer)?;
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or(InvalidTransfer)?;
        if offset != self.offset || end > self.credit || end > self.size {
            return Err(InvalidTransfer);
        }
        self.digest.update(&bytes);
        self.offset = end;
        Ok(bytes)
    }

    pub fn consumed(&mut self) {
        self.consumed = self.offset;
    }

    pub fn finish(&self, frame: &Value) -> Result<(), InvalidTransfer> {
        self.frame("hub-end", frame)?;
        if self.offset != self.size
            || self.consumed != self.size
            || self.digest.clone().finalize().as_slice() != self.hash
        {
            return Err(InvalidTransfer);
        }
        Ok(())
    }

    fn frame(&self, rule: &str, frame: &Value) -> Result<(), InvalidTransfer> {
        let value = ciborium::Value::serialized(frame).map_err(|_| InvalidTransfer)?;
        coglet_types::validate(rule, &value).map_err(|_| InvalidTransfer)?;
        if frame["correlation_id"] != self.id {
            return Err(InvalidTransfer);
        }
        Ok(())
    }
}

fn lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
