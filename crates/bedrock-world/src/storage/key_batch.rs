//! Shared-backing storage-key batches for exact multi-get operations.
//!
//! Bedrock keys are usually only 9-14 bytes long. Allocating one heap object per key creates more
//! allocator traffic than useful data for large chunk batches. This builder copies all raw keys into
//! one contiguous `Bytes` backing allocation and exposes cheap sliced `Bytes` descriptors compatible
//! with [`super::WorldStorage::get_many`].

use bytes::{Bytes, BytesMut};
use std::ops::Range;

/// Immutable exact-key batch whose entries share one contiguous backing allocation.
#[derive(Debug, Clone, Default)]
pub struct StorageKeyBatch {
    keys: Vec<Bytes>,
    total_key_bytes: usize,
}

impl StorageKeyBatch {
    /// Returns the exact keys in insertion order.
    #[must_use]
    pub fn keys(&self) -> &[Bytes] {
        &self.keys
    }

    /// Returns the number of exact keys in this batch.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Returns whether this batch contains no keys.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Returns the total number of raw key bytes stored in the shared backing buffer.
    #[must_use]
    pub const fn total_key_bytes(&self) -> usize {
        self.total_key_bytes
    }
}

/// Builder for [`StorageKeyBatch`].
#[derive(Debug, Default)]
pub struct StorageKeyBatchBuilder {
    bytes: BytesMut,
    ranges: Vec<Range<usize>>,
}

impl StorageKeyBatchBuilder {
    /// Creates an empty key batch builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a builder with expected raw-byte and key-count capacities.
    #[must_use]
    pub fn with_capacity(byte_capacity: usize, key_capacity: usize) -> Self {
        Self {
            bytes: BytesMut::with_capacity(byte_capacity),
            ranges: Vec::with_capacity(key_capacity),
        }
    }

    /// Copies one raw key into the contiguous backing buffer.
    pub fn push(&mut self, key: &[u8]) {
        let start = self.bytes.len();
        self.bytes.extend_from_slice(key);
        let end = self.bytes.len();
        self.ranges.push(start..end);
    }

    /// Returns the number of keys staged so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    /// Returns whether no keys have been staged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Freezes the shared backing allocation and creates one lightweight `Bytes` slice per key.
    #[must_use]
    pub fn finish(self) -> StorageKeyBatch {
        let total_key_bytes = self.bytes.len();
        if self.ranges.is_empty() {
            return StorageKeyBatch::default();
        }
        let backing = self.bytes.freeze();
        let keys = self
            .ranges
            .into_iter()
            .map(|range| backing.slice(range))
            .collect();
        StorageKeyBatch {
            keys,
            total_key_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_preserves_key_order_and_contents() {
        let mut builder = StorageKeyBatchBuilder::with_capacity(32, 3);
        builder.push(b"alpha");
        builder.push(b"b");
        builder.push(b"chunk-key");
        let batch = builder.finish();
        assert_eq!(batch.len(), 3);
        assert_eq!(batch.total_key_bytes(), 5 + 1 + 9);
        assert_eq!(batch.keys()[0].as_ref(), b"alpha");
        assert_eq!(batch.keys()[1].as_ref(), b"b");
        assert_eq!(batch.keys()[2].as_ref(), b"chunk-key");
    }

    #[test]
    fn slices_share_the_same_contiguous_backing_region() {
        let mut builder = StorageKeyBatchBuilder::new();
        builder.push(b"abcd");
        builder.push(b"efgh");
        let batch = builder.finish();
        let first_end = batch.keys()[0].as_ptr() as usize + batch.keys()[0].len();
        assert_eq!(first_end, batch.keys()[1].as_ptr() as usize);
    }
}
