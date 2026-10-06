use crate::{
    acquire_bitmap_buffer_capacity, configure_global_bitmap_pool, global_bitmap_pool,
    release_bitmap_buffer, trim_global_bitmap_pool_to,
};

/// Capacity accounting for one bitmap-pool acquire/release workload.
///
/// These values describe Vec reservations and idle pool storage, not allocator allocations,
/// committed pages, process RSS, or heap fragmentation.
#[derive(Clone, Copy, Debug)]
pub struct BitmapPoolBenchmarkSample {
    /// Sum of requested spare capacities, without initializing pixel contents.
    pub requested_bytes: usize,
    /// Sum of capacities actually acquired, including bucket rounding and oversized reuse.
    pub acquired_capacity_bytes: usize,
    /// Sum of idle pixel-buffer capacities retained by the global pool after release.
    pub retained_bytes: usize,
    /// Number of idle pixel buffers retained after release.
    pub free_buffers: usize,
    /// Capacity bytes of the benchmark's reusable outer Vec, excluding pixel buffers.
    pub staging_capacity_bytes: usize,
}

/// Drives the production global bitmap pool in an isolated, serial benchmark process.
///
/// Construction changes the process-global pool limit and clears its idle buffers. Do not run
/// this harness concurrently with other users of the pool. Pixel contents are not initialized.
pub struct BitmapPoolBenchmark {
    buffers: Vec<Vec<u8>>,
}

impl BitmapPoolBenchmark {
    /// Configures the idle-buffer budget and clears retained global pool buffers.
    ///
    /// The budget does not cap live image allocations. The production pool may retain one
    /// moderately oversized hot buffer according to its existing policy.
    pub fn new(byte_limit: usize) -> Self {
        configure_global_bitmap_pool(byte_limit);
        trim_global_bitmap_pool_to(0);
        Self {
            buffers: Vec::new(),
        }
    }

    /// Acquires the entire batch before releasing it, retaining the outer Vec for reuse.
    ///
    /// Returns requested and acquired capacities plus the pool's post-release idle storage.
    /// The first batch may grow the staging Vec; equal-sized warm batches reuse that capacity.
    pub fn cycle(&mut self, capacities: &[usize]) -> BitmapPoolBenchmarkSample {
        self.buffers.reserve(capacities.len());
        let mut acquired_capacity_bytes = 0;
        for &capacity in capacities {
            let buffer = acquire_bitmap_buffer_capacity(capacity);
            acquired_capacity_bytes += buffer.capacity();
            self.buffers.push(buffer);
        }
        for buffer in self.buffers.drain(..) {
            release_bitmap_buffer(buffer);
        }
        let snapshot = global_bitmap_pool().snapshot();
        BitmapPoolBenchmarkSample {
            requested_bytes: capacities.iter().sum(),
            acquired_capacity_bytes,
            retained_bytes: snapshot.retained_bytes,
            free_buffers: snapshot.free_buffers,
            staging_capacity_bytes: self.buffers.capacity() * std::mem::size_of::<Vec<u8>>(),
        }
    }

    /// Releases idle global pool buffers above the requested capacity and the empty staging Vec.
    ///
    /// This changes idle ownership only. It does not promise that the allocator returns pages
    /// to the OS, and does not change the configured budget or any live image buffers.
    pub fn trim(&mut self, byte_limit: usize) -> BitmapPoolBenchmarkSample {
        trim_global_bitmap_pool_to(byte_limit);
        self.buffers.shrink_to_fit();
        let snapshot = global_bitmap_pool().snapshot();
        BitmapPoolBenchmarkSample {
            requested_bytes: 0,
            acquired_capacity_bytes: 0,
            retained_bytes: snapshot.retained_bytes,
            free_buffers: snapshot.free_buffers,
            staging_capacity_bytes: self.buffers.capacity() * std::mem::size_of::<Vec<u8>>(),
        }
    }
}
