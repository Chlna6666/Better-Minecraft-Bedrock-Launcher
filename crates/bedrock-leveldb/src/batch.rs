use crate::coding::{
    VALUE_TYPE_DELETION, VALUE_TYPE_VALUE, get_length_prefixed_slice, put_length_prefixed_slice,
};
use crate::error::{LevelDbError, Result};
use bytes::Bytes;
use std::collections::HashSet;

/// One operation inside a [`WriteBatch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOp {
    /// Store `value` at `key`.
    Put {
        /// Raw key bytes to store.
        key: Bytes,
        /// Raw value bytes to store at `key`.
        value: Bytes,
    },
    /// Remove `key` from the visible view.
    Delete {
        /// Raw key bytes to delete.
        key: Bytes,
    },
}

impl WriteOp {
    #[must_use]
    fn key(&self) -> &Bytes {
        match self {
            Self::Put { key, .. } | Self::Delete { key } => key,
        }
    }
}

/// LevelDB-compatible write batch payload used by the WAL overlay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteBatch {
    sequence: u64,
    ops: Vec<WriteOp>,
}

impl WriteBatch {
    /// Creates an empty batch.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sequence: 0,
            ops: Vec::new(),
        }
    }

    /// Returns the sequence number encoded in this batch.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Sets the sequence number encoded in this batch.
    pub const fn set_sequence(&mut self, sequence: u64) {
        self.sequence = sequence;
    }

    /// Returns the operations in insertion order.
    #[must_use]
    pub fn ops(&self) -> &[WriteOp] {
        &self.ops
    }

    /// Returns the number of operations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Returns true when the batch has no operations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Returns an upper-bound-oriented encoded payload size estimate.
    ///
    /// The result includes the 12-byte batch header, operation tags, key/value bytes and a
    /// conservative five-byte varint allowance for each length-prefixed slice. It is intended for
    /// queue/backpressure decisions and capacity reservation, not as a replacement for [`Self::encode`].
    #[must_use]
    pub fn encoded_len_hint(&self) -> usize {
        self.ops.iter().fold(12usize, |total, op| match op {
            WriteOp::Put { key, value } => total
                .saturating_add(1 + 5)
                .saturating_add(key.len())
                .saturating_add(5)
                .saturating_add(value.len()),
            WriteOp::Delete { key } => total.saturating_add(1 + 5).saturating_add(key.len()),
        })
    }

    /// Adds a put operation.
    pub fn put(&mut self, key: impl Into<Bytes>, value: impl Into<Bytes>) {
        self.ops.push(WriteOp::Put {
            key: key.into(),
            value: value.into(),
        });
    }

    /// Adds a delete operation.
    pub fn delete(&mut self, key: impl Into<Bytes>) {
        self.ops.push(WriteOp::Delete { key: key.into() });
    }

    /// Removes superseded writes to the same key using LevelDB's last-write-wins semantics.
    ///
    /// The final operation for every key is retained, and retained operations keep their relative
    /// order from the original batch. This is useful for map-editor transactions where the same
    /// chunk/subchunk record may be updated several times before commit: compacting avoids redundant
    /// WAL and memtable traffic without changing the visible result of the batch.
    ///
    /// The operation vector is compacted in place so large transactions retain and reuse their
    /// existing allocation instead of creating a second operation vector of equal capacity.
    ///
    /// Returns the number of removed operations.
    pub fn compact_last_write_wins(&mut self) -> usize {
        if self.ops.len() < 2 {
            return 0;
        }
        let original_len = self.ops.len();
        let mut seen = HashSet::<Bytes>::with_capacity(self.ops.len());
        self.ops.reverse();
        self.ops.retain(|op| seen.insert(op.key().clone()));
        self.ops.reverse();
        original_len.saturating_sub(self.ops.len())
    }

    /// Encodes this batch into the `LevelDB` write batch wire format.
    ///
    /// # Errors
    ///
    /// Returns [`LevelDbError::InvalidArgument`] when the batch contains more
    /// than `u32::MAX` operations or when a key/value slice is too large to
    /// encode as a `LevelDB` length-prefixed slice.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let op_count = u32::try_from(self.ops.len())
            .map_err(|_| LevelDbError::invalid_argument("batch is too large".to_string()))?;
        let mut out = Vec::with_capacity(self.encoded_len_hint());
        out.extend_from_slice(&self.sequence.to_le_bytes());
        out.extend_from_slice(&op_count.to_le_bytes());
        for op in &self.ops {
            match op {
                WriteOp::Put { key, value } => {
                    out.push(VALUE_TYPE_VALUE);
                    put_length_prefixed_slice(key, &mut out)?;
                    put_length_prefixed_slice(value, &mut out)?;
                }
                WriteOp::Delete { key } => {
                    out.push(VALUE_TYPE_DELETION);
                    put_length_prefixed_slice(key, &mut out)?;
                }
            }
        }
        Ok(out)
    }

    /// Decodes one `LevelDB` write batch payload.
    ///
    /// # Errors
    ///
    /// Returns [`LevelDbError::Corruption`] when the header, record count, tag,
    /// or length-prefixed payloads are malformed.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 12 {
            return Err(LevelDbError::corruption(
                "write batch header is truncated".to_string(),
            ));
        }
        let mut sequence_bytes = [0_u8; 8];
        sequence_bytes.copy_from_slice(&bytes[..8]);
        let sequence = u64::from_le_bytes(sequence_bytes);

        let mut count_bytes = [0_u8; 4];
        count_bytes.copy_from_slice(&bytes[8..12]);
        let expected_count = usize::try_from(u32::from_le_bytes(count_bytes))
            .map_err(|_| LevelDbError::corruption("batch count overflow".to_string()))?;

        let mut input = &bytes[12..];
        // Every operation requires at least a one-byte tag and a one-byte
        // length prefix, even when its key is empty. Reject impossible counts
        // before reserving attacker-controlled capacity.
        if expected_count > input.len() / 2 {
            return Err(LevelDbError::corruption(format!(
                "batch record count {expected_count} exceeds payload capacity"
            )));
        }
        let mut ops = Vec::with_capacity(expected_count);
        while !input.is_empty() {
            let Some((&tag, rest)) = input.split_first() else {
                break;
            };
            input = rest;
            match tag {
                VALUE_TYPE_VALUE => {
                    let key = Bytes::copy_from_slice(get_length_prefixed_slice(&mut input)?);
                    let value = Bytes::copy_from_slice(get_length_prefixed_slice(&mut input)?);
                    ops.push(WriteOp::Put { key, value });
                }
                VALUE_TYPE_DELETION => {
                    let key = Bytes::copy_from_slice(get_length_prefixed_slice(&mut input)?);
                    ops.push(WriteOp::Delete { key });
                }
                other => {
                    return Err(LevelDbError::corruption(format!(
                        "unknown batch record tag {other}"
                    )));
                }
            }
        }
        if ops.len() != expected_count {
            return Err(LevelDbError::corruption(format!(
                "batch record count mismatch: expected {expected_count}, got {}",
                ops.len()
            )));
        }
        Ok(Self { sequence, ops })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_batch_roundtrips() {
        let mut batch = WriteBatch::new();
        batch.set_sequence(42);
        batch.put(Bytes::from_static(b"a"), Bytes::from_static(b"one"));
        batch.delete(Bytes::from_static(b"b"));

        let encoded = batch.encode().expect("encode");
        let decoded = WriteBatch::decode(&encoded).expect("decode");
        assert_eq!(decoded, batch);
    }

    #[test]
    fn compact_last_write_wins_removes_superseded_operations() {
        let mut batch = WriteBatch::new();
        batch.put(Bytes::from_static(b"chunk"), Bytes::from_static(b"old"));
        batch.put(Bytes::from_static(b"other"), Bytes::from_static(b"keep"));
        batch.delete(Bytes::from_static(b"chunk"));
        batch.put(Bytes::from_static(b"chunk"), Bytes::from_static(b"new"));

        let capacity = batch.ops.capacity();
        assert_eq!(batch.compact_last_write_wins(), 2);
        assert!(batch.ops.capacity() >= capacity);
        assert_eq!(
            batch.ops(),
            &[
                WriteOp::Put {
                    key: Bytes::from_static(b"other"),
                    value: Bytes::from_static(b"keep"),
                },
                WriteOp::Put {
                    key: Bytes::from_static(b"chunk"),
                    value: Bytes::from_static(b"new"),
                },
            ]
        );
    }

    #[test]
    fn encoded_len_hint_is_never_smaller_than_actual_encoding() {
        let mut batch = WriteBatch::new();
        batch.put(Bytes::from_static(b"a"), Bytes::from_static(b"value"));
        batch.delete(Bytes::from_static(b"b"));
        let encoded = batch.encode().expect("encode");
        assert!(batch.encoded_len_hint() >= encoded.len());
    }

    #[test]
    fn decode_rejects_impossible_count_before_allocation() {
        let mut bytes = vec![0_u8; 12];
        bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());

        let error = WriteBatch::decode(&bytes).expect_err("impossible count must fail");

        assert_eq!(error.kind(), crate::error::ErrorKind::Corruption);
    }
}
