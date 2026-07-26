use parking_lot::Mutex;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BitmapPoolSnapshot {
    pub retained_bytes: usize,
    pub free_buffers: usize,
    pub max_bytes: usize,
    pub max_buffer_bytes: usize,
}

struct BitmapPoolState {
    free: Vec<Vec<u8>>,
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
                free: Vec::new(),
                retained_bytes: 0,
            }),
            max_bytes: AtomicUsize::new(max_bytes),
            max_buffer_bytes: AtomicUsize::new(max_buffer_bytes.max(1)),
        }
    }

    fn bucket_capacity(&self, requested: usize) -> usize {
        requested
            .max(1)
            .next_power_of_two()
            .min(self.max_buffer_bytes.load(Ordering::Relaxed))
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
        let index = state
            .free
            .iter()
            .position(|buffer| buffer.capacity() >= bucket);
        let mut buffer = index
            .map(|index| {
                let buffer = state.free.swap_remove(index);
                state.retained_bytes = state.retained_bytes.saturating_sub(buffer.capacity());
                buffer
            })
            .unwrap_or_else(|| Vec::with_capacity(bucket));
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
        state.free.push(buffer);
    }

    pub(crate) fn trim_to(&self, max_bytes: usize) {
        let mut state = self.state.lock();
        let max_buffer_bytes = self.max_buffer_bytes.load(Ordering::Relaxed);
        state
            .free
            .retain(|buffer| buffer.capacity() <= max_buffer_bytes);
        state.retained_bytes = state.free.iter().map(Vec::capacity).sum();
        while state.retained_bytes > max_bytes {
            let Some(buffer) = state.free.pop() else {
                break;
            };
            state.retained_bytes = state.retained_bytes.saturating_sub(buffer.capacity());
        }
    }

    pub(crate) fn snapshot(&self) -> BitmapPoolSnapshot {
        let state = self.state.lock();
        BitmapPoolSnapshot {
            retained_bytes: state.retained_bytes,
            free_buffers: state.free.len(),
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
    use super::BitmapPool;
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
