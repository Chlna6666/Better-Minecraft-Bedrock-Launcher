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
    /// The operation vector is compacted in place so large transactions retain and reuse their
    /// existing allocation instead of creating a second operation vector of equal capacity.
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

    /// Encodes this batch into a newly allocated LevelDB write-batch buffer.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.encoded_len_hint());
        self.encode_into(&mut out)?;
        Ok(out)
    }

    /// Encodes this batch into a caller-owned reusable buffer.
    ///
    /// Existing capacity is retained so high-frequency writers can keep one
    /// thread-local WAL scratch allocation instead of allocating for every
    /// batch.
    pub(crate) fn encode_into(&self, out: &mut Vec<u8>) -> Result<()> {
        let op_count = u32::try_from(self.ops.len())
            .map_err(|_| LevelDbError::invalid_argument("batch is too large".to_string()))?;
        out.clear();
        let required = self.encoded_len_hint();
        if out.capacity() < required {
            out.reserve(required.saturating_sub(out.len()));
        }
        out.extend_from_slice(&self.sequence.to_le_bytes());
        out.extend_from_slice(&op_count.to_le_bytes());
        for op in &self.ops {
            match op {
                WriteOp::Put { key, value } => {
                    out.push(VALUE_TYPE_VALUE);
                    put_length_prefixed_slice(key, out)?;
                    put_length_prefixed_slice(value, out)?;
                }
                WriteOp::Delete { key } => {
                    out.push(VALUE_TYPE_DELETION);
                    put_length_prefixed_slice(key, out)?;
                }
            }
        }
        Ok(())
    }

    /// Decodes one `LevelDB` write batch payload.
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
    fn encode_into_reuses_existing_capacity() {
        let mut batch = WriteBatch::new();
        batch.put(Bytes::from_static(b"a"), Bytes::from_static(b"value"));
        let mut scratch = Vec::with_capacity(4096);
        let ptr = scratch.as_ptr();
        batch.encode_into(&mut scratch).expect("encode into");
        assert_eq!(scratch.as_ptr(), ptr);
        assert_eq!(WriteBatch::decode(&scratch).expect("decode"), batch);
    }

    #[test]
    fn decode_rejects_impossible_count_before_allocation() {
        let mut bytes = vec![0_u8; 12];
        bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());

        let error = WriteBatch::decode(&bytes).expect_err("impossible count must fail");

        assert_eq!(error.kind(), crate::error::ErrorKind::Corruption);
    }
}
