use parking_lot::Mutex;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

const LARGE_BITMAP_BUCKET_GRANULARITY: usize = 64 * 1024;
const VERY_LARGE_BITMAP_THRESHOLD: usize = 4 * 1024 * 1024;
const VERY_LARGE_BITMAP_BUCKET_GRANULARITY: usize = 1024 * 1024;
const HUGE_BITMAP_THRESHOLD: usize = 32 * 1024 * 1024;
const HUGE_BITMAP_BUCKET_GRANULARITY: usize = 4 * 1024 * 1024;
const SMALL_REUSE_CLASS_LIMIT: usize = 1024 * 1024;
const MEDIUM_REUSE_CLASS_LIMIT: usize = 8 * 1024 * 1024;
const LARGE_REUSE_CLASS_LIMIT: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BitmapPoolSnapshot {
    pub retained_bytes: usize,
    pub free_buffers: usize,
    pub max_bytes: usize,
    pub max_buffer_bytes: usize,
}

struct BitmapPoolState {
    free: BTreeMap<usize, Vec<Vec<u8>>>,
    free_buffers: usize,
    retained_bytes: usize,
}

pub(crate) struct BitmapPool {
    state: Mutex<BitmapPoolState>,
    max_bytes: AtomicUsize,
    max_buffer_bytes: AtomicUsize,
}

impl BitmapPool {
    pub(crate) fn new(max_bytes: usize, max_buffer_bytes: usize) -> Self {
        Self {
            state: Mutex::new(BitmapPoolState {
                free: BTreeMap::new(),
                free_buffers: 0,
                retained_bytes: 0,
            }),
            max_bytes: AtomicUsize::new(max_bytes),
            max_buffer_bytes: AtomicUsize::new(max_buffer_bytes.max(1)),
        }
    }

    fn bucket_capacity(&self, requested: usize) -> usize {
        let requested = requested.max(1);
        let capacity = if requested <= LARGE_BITMAP_BUCKET_GRANULARITY {
            requested.next_power_of_two()
        } else if requested <= VERY_LARGE_BITMAP_THRESHOLD {
            requested
                .div_ceil(LARGE_BITMAP_BUCKET_GRANULARITY)
                .saturating_mul(LARGE_BITMAP_BUCKET_GRANULARITY)
        } else if requested <= HUGE_BITMAP_THRESHOLD {
            requested
                .div_ceil(VERY_LARGE_BITMAP_BUCKET_GRANULARITY)
                .saturating_mul(VERY_LARGE_BITMAP_BUCKET_GRANULARITY)
        } else {
            requested
                .div_ceil(HUGE_BITMAP_BUCKET_GRANULARITY)
                .saturating_mul(HUGE_BITMAP_BUCKET_GRANULARITY)
        };
        capacity.min(self.max_buffer_bytes.load(Ordering::Relaxed))
    }

    fn reuse_class(capacity: usize) -> u8 {
        if capacity <= SMALL_REUSE_CLASS_LIMIT {
            0
        } else if capacity <= MEDIUM_REUSE_CLASS_LIMIT {
            1
        } else if capacity <= LARGE_REUSE_CLASS_LIMIT {
            2
        } else {
            3
        }
    }

    /// Returns an empty buffer with at least `capacity` spare capacity, without zero-filling.
    ///
    /// Callers that need an initialized prefix should use [`Self::acquire`] or resize the
    /// returned buffer themselves; skipping the fill here avoids a redundant memset for
    /// callers that overwrite the whole buffer anyway.
    fn acquire_capacity(&self, capacity: usize) -> Vec<u8> {
        if capacity == 0 || capacity > self.max_buffer_bytes.load(Ordering::Relaxed) {
            return Vec::with_capacity(capacity);
        }

        let bucket = self.bucket_capacity(capacity);
        let requested_class = Self::reuse_class(bucket);
        let mut state = self.state.lock();
        let available_capacity = state
            .free
            .range(bucket..)
            .find(|(available_capacity, _)| {
                Self::reuse_class(**available_capacity) == requested_class
            })
            .map(|(&available_capacity, _)| available_capacity);
        let mut buffer = if let Some(available_capacity) = available_capacity {
            let buffers = state
                .free
                .get_mut(&available_capacity)
                .expect("the selected bitmap capacity exists");
            let buffer = buffers
                .pop()
                .expect("a retained bitmap capacity has at least one buffer");
            if buffers.is_empty() {
                state.free.remove(&available_capacity);
            }
            state.free_buffers = state.free_buffers.saturating_sub(1);
            state.retained_bytes = state.retained_bytes.saturating_sub(buffer.capacity());
            buffer
        } else {
            Vec::with_capacity(bucket)
        };
        buffer.clear();
        buffer
    }

    fn acquire(&self, length: usize) -> Vec<u8> {
        let mut buffer = self.acquire_capacity(length);
        buffer.resize(length, 0);
        buffer
    }

    fn evict_capacity(state: &mut BitmapPoolState, capacity: usize) -> bool {
        let Some(buffers) = state.free.get_mut(&capacity) else {
            return false;
        };
        let buffer = buffers
            .pop()
            .expect("a retained bitmap capacity has at least one buffer");
        if buffers.is_empty() {
            state.free.remove(&capacity);
        }
        state.free_buffers = state.free_buffers.saturating_sub(1);
        state.retained_bytes = state.retained_bytes.saturating_sub(buffer.capacity());
        true
    }

    fn evict_largest_free_buffer(state: &mut BitmapPoolState) -> bool {
        let Some(largest_capacity) = state.free.last_key_value().map(|(&capacity, _)| capacity)
        else {
            return false;
        };
        Self::evict_capacity(state, largest_capacity)
    }

    fn evict_largest_free_buffer_in_class(state: &mut BitmapPoolState, class: u8) -> bool {
        let capacity = state
            .free
            .keys()
            .rev()
            .copied()
            .find(|capacity| Self::reuse_class(*capacity) == class);
        capacity.is_some_and(|capacity| Self::evict_capacity(state, capacity))
    }

    fn release(&self, mut buffer: Vec<u8>) {
        let capacity = buffer.capacity();
        if capacity == 0 || capacity > self.max_buffer_bytes.load(Ordering::Relaxed) {
            return;
        }

        buffer.clear();
        let max_bytes = self.max_bytes.load(Ordering::Relaxed);
        let class = Self::reuse_class(capacity);
        let mut state = self.state.lock();

        // Keep resize/decode reuse local to a broad size class. In particular a returned 4K
        // buffer should replace stale 4K-class buffers before it evicts the small allocations
        // used by normal UI. This reduces allocator churn and cross-workload fragmentation.
        if capacity > max_bytes {
            while Self::evict_largest_free_buffer_in_class(&mut state, class) {}
            // One oversized hot buffer is still allowed, but preserve a small quarter-budget
            // working set for unrelated UI instead of flushing the entire pool.
            let protected_other_bytes = max_bytes / 4;
            while state.retained_bytes > protected_other_bytes {
                if !Self::evict_largest_free_buffer(&mut state) {
                    break;
                }
            }
        } else {
            while state.retained_bytes.saturating_add(capacity) > max_bytes {
                if Self::evict_largest_free_buffer_in_class(&mut state, class) {
                    continue;
                }
                if !Self::evict_largest_free_buffer(&mut state) {
                    break;
                }
            }
        }

        state.retained_bytes = state.retained_bytes.saturating_add(capacity);
        state.free_buffers = state.free_buffers.saturating_add(1);
        state.free.entry(capacity).or_default().push(buffer);
    }

    pub(crate) fn trim_to(&self, max_bytes: usize) {
        let mut state = self.state.lock();
        let max_buffer_bytes = self.max_buffer_bytes.load(Ordering::Relaxed);
        state
            .free
            .retain(|capacity, _| *capacity <= max_buffer_bytes);
        state.free_buffers = state.free.values().map(Vec::len).sum();
        state.retained_bytes = state
            .free
            .iter()
            .map(|(capacity, buffers)| capacity.saturating_mul(buffers.len()))
            .sum();
        while state.retained_bytes > max_bytes {
            if !Self::evict_largest_free_buffer(&mut state) {
                break;
            }
        }
    }

    pub(crate) fn snapshot(&self) -> BitmapPoolSnapshot {
        let state = self.state.lock();
        BitmapPoolSnapshot {
            retained_bytes: state.retained_bytes,
            free_buffers: state.free_buffers,
            max_bytes: self.max_bytes.load(Ordering::Relaxed),
            max_buffer_bytes: self.max_buffer_bytes.load(Ordering::Relaxed),
        }
    }
}

static GLOBAL_BITMAP_POOL: OnceLock<Arc<BitmapPool>> = OnceLock::new();

pub(crate) fn global_bitmap_pool() -> &'static Arc<BitmapPool> {
    GLOBAL_BITMAP_POOL.get_or_init(|| Arc::new(BitmapPool::new(64 * 1024 * 1024, usize::MAX)))
}

pub(crate) fn configure_global_bitmap_pool(max_bytes: usize, max_buffer_bytes: usize) {
    let pool = global_bitmap_pool();
    pool.max_bytes.store(max_bytes, Ordering::Relaxed);
    pool.max_buffer_bytes
        .store(max_buffer_bytes.max(1), Ordering::Relaxed);
    pool.trim_to(max_bytes);
}

pub(crate) fn trim_global_bitmap_pool(level: crate::GpuiMemoryTrimLevel) {
    let pool = global_bitmap_pool();
    let max_bytes = match level {
        crate::GpuiMemoryTrimLevel::Light => pool.snapshot().max_bytes.saturating_mul(3) / 4,
        crate::GpuiMemoryTrimLevel::Moderate => 0,
        crate::GpuiMemoryTrimLevel::Aggressive => 0,
    };
    pool.trim_to(max_bytes);
}

pub(crate) fn trim_global_bitmap_pool_to(max_bytes: usize) {
    global_bitmap_pool().trim_to(max_bytes);
}

pub(crate) fn acquire_bitmap_buffer(length: usize) -> Vec<u8> {
    global_bitmap_pool().acquire(length)
}

pub(crate) fn acquire_bitmap_buffer_capacity(capacity: usize) -> Vec<u8> {
    global_bitmap_pool().acquire_capacity(capacity)
}

pub(crate) fn release_bitmap_buffer(buffer: Vec<u8>) {
    global_bitmap_pool().release(buffer);
}

pub(crate) struct BitmapBytes {
    storage: BitmapStorage,
}

enum BitmapStorage {
    Pooled(Option<Vec<u8>>),
    Shared(Arc<[u8]>),
}

impl BitmapBytes {
    pub(crate) fn from_vec(bytes: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            storage: BitmapStorage::Pooled(Some(bytes)),
        })
    }

    pub(crate) fn from_shared(bytes: Arc<[u8]>) -> Arc<Self> {
        Arc::new(Self {
            storage: BitmapStorage::Shared(bytes),
        })
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        match &self.storage {
            BitmapStorage::Pooled(Some(bytes)) => bytes.as_slice(),
            BitmapStorage::Pooled(None) => &[],
            BitmapStorage::Shared(bytes) => bytes.as_ref(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.as_slice().len()
    }
}

impl Drop for BitmapBytes {
    fn drop(&mut self) {
        if let BitmapStorage::Pooled(bytes) = &mut self.storage
            && let Some(bytes) = bytes.take()
        {
            global_bitmap_pool().release(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BitmapPool, HUGE_BITMAP_BUCKET_GRANULARITY, LARGE_BITMAP_BUCKET_GRANULARITY,
        VERY_LARGE_BITMAP_BUCKET_GRANULARITY,
    };
    use std::sync::Arc;

    #[test]
    fn reuses_capacity_buckets_and_respects_pool_eligibility() {
        let pool = BitmapPool::new(1024, 512);
        let buffer = pool.acquire(200);
        assert!(buffer.capacity() >= 200);
        pool.release(buffer);
        assert_eq!(pool.snapshot().free_buffers, 1);

        let reused = pool.acquire(128);
        assert!(reused.capacity() >= 128);
        assert_eq!(pool.snapshot().free_buffers, 0);
        pool.release(reused);

        // Buffers above the explicit reuse eligibility threshold are still valid allocations,
        // they just bypass this particular pool.
        pool.release(Vec::with_capacity(2048));
        assert_eq!(pool.snapshot().free_buffers, 1);
        pool.trim_to(0);
        assert_eq!(pool.snapshot().retained_bytes, 0);
    }

    #[test]
    fn oversized_image_buffer_is_kept_as_single_hot_reuse_slot() {
        let pool = BitmapPool::new(1024, usize::MAX);
        pool.release(Vec::with_capacity(4096));

        let snapshot = pool.snapshot();
        assert_eq!(snapshot.free_buffers, 1);
        assert!(snapshot.retained_bytes >= 4096);

        let reused = pool.acquire_capacity(3000);
        assert!(reused.capacity() >= 4096);
        assert_eq!(pool.snapshot().free_buffers, 0);
    }

    #[test]
    fn oversized_buffer_preserves_small_ui_reuse_reserve() {
        let pool = BitmapPool::new(1024 * 1024, usize::MAX);
        pool.release(Vec::with_capacity(128 * 1024));
        pool.release(Vec::with_capacity(128 * 1024));
        pool.release(Vec::with_capacity(2 * 1024 * 1024));

        let snapshot = pool.snapshot();
        assert_eq!(snapshot.free_buffers, 3);
        assert!(snapshot.retained_bytes >= 2 * 1024 * 1024 + 256 * 1024);
    }

    #[test]
    fn newest_buffer_replaces_stale_buffers_when_free_list_budget_is_full() {
        let pool = BitmapPool::new(1024, usize::MAX);
        pool.release(Vec::with_capacity(512));
        pool.release(Vec::with_capacity(512));
        assert_eq!(pool.snapshot().retained_bytes, 1024);

        pool.release(Vec::with_capacity(768));
        let snapshot = pool.snapshot();
        assert_eq!(snapshot.free_buffers, 1);
        assert!(snapshot.retained_bytes >= 768);
    }

    #[test]
    fn large_bucket_granularity_coarsens_as_buffers_grow() {
        let pool = BitmapPool::new(usize::MAX, usize::MAX);
        assert_eq!(
            pool.bucket_capacity(2 * 1024 * 1024 + 1) % LARGE_BITMAP_BUCKET_GRANULARITY,
            0
        );
        assert_eq!(
            pool.bucket_capacity(8 * 1024 * 1024 + 1) % VERY_LARGE_BITMAP_BUCKET_GRANULARITY,
            0
        );
        assert_eq!(
            pool.bucket_capacity(40 * 1024 * 1024 + 1) % HUGE_BITMAP_BUCKET_GRANULARITY,
            0
        );
    }

    #[test]
    fn trim_releases_retained_capacity() {
        let pool = BitmapPool::new(4096, usize::MAX);
        pool.release(Vec::with_capacity(1024));
        pool.release(Vec::with_capacity(2048));
        assert!(pool.snapshot().retained_bytes >= 3072);
        pool.trim_to(1024);
        assert!(pool.snapshot().retained_bytes <= 1024);
    }

    #[test]
    fn bitmap_bytes_returns_owned_vec_to_pool_on_last_arc_drop() {
        let pool = super::global_bitmap_pool();
        pool.trim_to(0);
        let bytes = super::BitmapBytes::from_vec(Vec::with_capacity(256));
        let cloned = Arc::clone(&bytes);
        drop(bytes);
        assert_eq!(pool.snapshot().free_buffers, 0);
        drop(cloned);
        assert_eq!(pool.snapshot().free_buffers, 1);
        pool.trim_to(0);
    }
}
