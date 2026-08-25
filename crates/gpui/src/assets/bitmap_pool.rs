use parking_lot::Mutex;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

const LARGE_BITMAP_BUCKET_GRANULARITY: usize = 64 * 1024;

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
        } else {
            requested
                .div_ceil(LARGE_BITMAP_BUCKET_GRANULARITY)
                .saturating_mul(LARGE_BITMAP_BUCKET_GRANULARITY)
        };
        capacity.min(self.max_buffer_bytes.load(Ordering::Relaxed))
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
        let mut state = self.state.lock();
        let available_capacity = state
            .free
            .range(bucket..)
            .next()
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

    fn release(&self, mut buffer: Vec<u8>) {
        let capacity = buffer.capacity();
        if capacity == 0
            || capacity > self.max_buffer_bytes.load(Ordering::Relaxed)
            || capacity > self.max_bytes.load(Ordering::Relaxed)
        {
            return;
        }

        buffer.clear();
        let mut state = self.state.lock();
        if state.retained_bytes.saturating_add(capacity) > self.max_bytes.load(Ordering::Relaxed) {
            return;
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
            let Some(largest_capacity) = state.free.last_key_value().map(|(&capacity, _)| capacity)
            else {
                break;
            };
            let buffers = state
                .free
                .get_mut(&largest_capacity)
                .expect("the largest bitmap capacity exists");
            let buffer = buffers
                .pop()
                .expect("a retained bitmap capacity has at least one buffer");
            if buffers.is_empty() {
                state.free.remove(&largest_capacity);
            }
            state.free_buffers = state.free_buffers.saturating_sub(1);
            state.retained_bytes = state.retained_bytes.saturating_sub(buffer.capacity());
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
    GLOBAL_BITMAP_POOL.get_or_init(|| Arc::new(BitmapPool::new(64 * 1024 * 1024, 16 * 1024 * 1024)))
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
    use super::{BitmapPool, LARGE_BITMAP_BUCKET_GRANULARITY};
    use std::sync::Arc;

    #[test]
    fn reuses_capacity_buckets_and_respects_limits() {
        let pool = BitmapPool::new(1024, 512);
        let buffer = pool.acquire(200);
        assert!(buffer.capacity() >= 200);
        pool.release(buffer);
        assert_eq!(pool.snapshot().free_buffers, 1);

        let reused = pool.acquire(128);
        assert!(reused.capacity() >= 128);
        assert_eq!(pool.snapshot().free_buffers, 0);
        pool.release(reused);

        pool.release(Vec::with_capacity(2048));
        assert_eq!(pool.snapshot().free_buffers, 1);
        pool.trim_to(0);
        assert_eq!(pool.snapshot().retained_bytes, 0);
    }

    #[test]
    fn large_buffers_use_dense_buckets_to_limit_internal_fragmentation() {
        let pool = BitmapPool::new(8 * 1024 * 1024, 8 * 1024 * 1024);
        let requested = 1024 * 1024 + 1;
        let buffer = pool.acquire_capacity(requested);

        assert!(buffer.capacity() >= requested);
        assert!(buffer.capacity() <= requested + LARGE_BITMAP_BUCKET_GRANULARITY);
        assert!(buffer.capacity() < requested.next_power_of_two());
    }

    #[test]
    fn supports_concurrent_acquire_and_release() {
        let pool = Arc::new(BitmapPool::new(64 * 1024, 4096));
        let workers = (0..8)
            .map(|_| {
                let pool = pool.clone();
                std::thread::spawn(move || {
                    for _ in 0..64 {
                        let buffer = pool.acquire(1024);
                        assert_eq!(buffer.len(), 1024);
                        pool.release(buffer);
                    }
                })
            })
            .collect::<Vec<_>>();

        for worker in workers {
            worker.join().expect("bitmap pool worker should complete");
        }
        assert!(pool.snapshot().retained_bytes <= 64 * 1024);
    }
}
