use serde::{Deserialize, Serialize};

/// Per-memory-location allocator metrics for one backend bucket.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllocatorBucketMetricsSnapshot {
    /// Bytes currently allocated from this allocator bucket.
    pub allocated_bytes: usize,
    /// Bytes currently reserved by blocks in this allocator bucket.
    pub reserved_bytes: usize,
    /// Number of live blocks in this allocator bucket.
    pub block_count: usize,
    /// Bytes attributed to committed allocations in this allocator bucket.
    pub committed_allocated_bytes: usize,
    /// Number of committed allocations in this allocator bucket.
    pub committed_allocation_count: usize,
}
