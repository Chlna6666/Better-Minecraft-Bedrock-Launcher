use crate::coding::{
    VALUE_TYPE_DELETION, VALUE_TYPE_VALUE, get_length_prefixed_slice, put_length_prefixed_slice,
};
use crate::error::{LevelDbError, Result};
use bytes::Bytes;

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
        let mut out = Vec::new();
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
}
