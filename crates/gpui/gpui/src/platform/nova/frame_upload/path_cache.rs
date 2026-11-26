use super::*;
use std::collections::VecDeque;

pub(in crate::platform::nova) const MAX_PATH_RASTERIZATION_CACHE_ENTRIES: usize = 4096;
const MAX_PATH_RASTERIZATION_CACHE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::platform::nova) struct PathRasterizationCacheKey {
    pub(in crate::platform::nova) path_id: crate::PathCacheId,
    pub(in crate::platform::nova) generation: crate::PathGeometryGeneration,
    pub(in crate::platform::nova) vertex_count: usize,
    pub(in crate::platform::nova) geometry_hash: u64,
    pub(in crate::platform::nova) paint_hash: u64,
}

#[derive(Clone, Debug)]
pub(in crate::platform::nova) struct PathRasterizationCacheEntry {
    pub(in crate::platform::nova) bytes: Arc<[u8]>,
    pub(in crate::platform::nova) vertex_count: u32,
}

/// Bounded retained path cache with recency-aware eviction.
///
/// Hash-map bucket order is unrelated to usage, so capacity pressure must not discard hot SVG/path
/// entries arbitrarily. Accesses update a monotonic stamp and insertion evicts least-recently-used
/// entries only until the entry and 32 MiB payload budgets fit. Oversized paths still render,
/// but are not retained. The budget conservatively charges shared payloads once per entry.
#[derive(Default)]
pub(in crate::platform::nova) struct PathRasterizationCache {
    entries: FxHashMap<PathRasterizationCacheKey, (PathRasterizationCacheEntry, u64)>,
    access_order: VecDeque<(u64, PathRasterizationCacheKey)>,
    next_stamp: u64,
    retained_bytes: usize,
}

impl PathRasterizationCache {
    pub(in crate::platform::nova) fn payload_bytes(&self) -> u64 {
        let mut seen = FxHashSet::default();
        self.entries
            .values()
            .filter_map(|(entry, _)| {
                seen.insert(Arc::as_ptr(&entry.bytes) as *const u8 as usize)
                    .then_some(entry.bytes.len() as u64)
            })
            .sum()
    }
    #[inline]
    fn next_stamp(&mut self) -> u64 {
        self.next_stamp = self.next_stamp.wrapping_add(1);
        self.next_stamp
    }

    // Hits append stamps rather than unlinking heap-allocated LRU nodes. Periodically remove
    // superseded stamps so a hot path cannot grow the queue indefinitely between evictions.
    fn compact_access_order(&mut self) {
        if self.access_order.len() > self.entries.len().max(64).saturating_mul(2) {
            self.access_order.retain(|(stamp, key)| {
                self.entries
                    .get(key)
                    .is_some_and(|(_, current)| current == stamp)
            });
        }
    }

    #[inline]
    pub(in crate::platform::nova) fn get(
        &mut self,
        key: &PathRasterizationCacheKey,
    ) -> Option<&PathRasterizationCacheEntry> {
        self.compact_access_order();
        let stamp = self.next_stamp();
        let (entry, last_used) = self.entries.get_mut(key)?;
        *last_used = stamp;
        self.access_order.push_back((stamp, *key));
        Some(entry)
    }

    #[inline]
    pub(in crate::platform::nova) fn insert(
        &mut self,
        key: PathRasterizationCacheKey,
        entry: PathRasterizationCacheEntry,
    ) {
        self.compact_access_order();
        if let Some((previous, _)) = self.entries.remove(&key) {
            self.retained_bytes -= previous.bytes.len();
        }
        let bytes = entry.bytes.len();
        if bytes > MAX_PATH_RASTERIZATION_CACHE_BYTES {
            return;
        }
        if self.entries.len() >= MAX_PATH_RASTERIZATION_CACHE_ENTRIES
            || self.retained_bytes > MAX_PATH_RASTERIZATION_CACHE_BYTES - bytes
        {
            self.evict_to_fit(
                MAX_PATH_RASTERIZATION_CACHE_ENTRIES - 1,
                MAX_PATH_RASTERIZATION_CACHE_BYTES - bytes,
            );
        }
        let stamp = self.next_stamp();
        self.entries.insert(key, (entry, stamp));
        self.access_order.push_back((stamp, key));
        self.retained_bytes += bytes;
    }

    fn evict_to_fit(&mut self, max_entries: usize, max_bytes: usize) {
        while self.entries.len() > max_entries || self.retained_bytes > max_bytes {
            let Some((stamp, key)) = self.access_order.pop_front() else {
                break;
            };
            if self
                .entries
                .get(&key)
                .is_some_and(|(_, current)| *current == stamp)
                && let Some((entry, _)) = self.entries.remove(&key)
            {
                self.retained_bytes -= entry.bytes.len();
            }
        }
    }

    pub(in crate::platform::nova) fn clear(&mut self) {
        self.entries.clear();
        self.access_order.clear();
        self.next_stamp = 0;
        self.retained_bytes = 0;
    }

    pub(in crate::platform::nova) fn shrink_to(&mut self, min_capacity: usize) {
        self.entries.shrink_to(min_capacity);
        self.access_order.shrink_to(min_capacity);
    }
}

#[derive(Clone, Copy, Debug)]
pub(in crate::platform::nova) struct PathGeometryHashMemo {
    pub(in crate::platform::nova) generation: crate::PathGeometryGeneration,
    pub(in crate::platform::nova) vertex_count: usize,
    pub(in crate::platform::nova) first_xy_bits: (u32, u32),
    pub(in crate::platform::nova) last_xy_bits: (u32, u32),
    pub(in crate::platform::nova) geometry_hash: u64,
}

/// Bounded memo for path geometry hashes. Path IDs are stable across paint-only changes, so
/// retaining the most recently used geometry fingerprints avoids rescanning large SVG/path vertex
/// arrays while still letting cold high-water marks age out.
#[derive(Default)]
pub(in crate::platform::nova) struct PathGeometryHashCache {
    entries: FxHashMap<crate::PathCacheId, (PathGeometryHashMemo, u64)>,
    eviction_scratch: Vec<(u64, crate::PathCacheId)>,
    next_stamp: u64,
}

impl PathGeometryHashCache {
    #[inline]
    fn next_stamp(&mut self) -> u64 {
        self.next_stamp = self.next_stamp.wrapping_add(1);
        self.next_stamp
    }

    #[inline]
    pub(in crate::platform::nova) fn get(
        &mut self,
        key: &crate::PathCacheId,
    ) -> Option<PathGeometryHashMemo> {
        let stamp = self.next_stamp();
        let (memo, last_used) = self.entries.get_mut(key)?;
        *last_used = stamp;
        Some(*memo)
    }

    #[inline]
    pub(in crate::platform::nova) fn insert(
        &mut self,
        key: crate::PathCacheId,
        memo: PathGeometryHashMemo,
    ) {
        if self.entries.len() >= MAX_PATH_RASTERIZATION_CACHE_ENTRIES
            && !self.entries.contains_key(&key)
        {
            self.evict_cold_half();
        }
        let stamp = self.next_stamp();
        let _ = self.entries.insert(key, (memo, stamp));
    }

    fn evict_cold_half(&mut self) {
        let remove_count = self.entries.len() / 2;
        if remove_count == 0 {
            return;
        }

        self.eviction_scratch.clear();
        self.eviction_scratch
            .extend(self.entries.iter().map(|(key, (_, stamp))| (*stamp, *key)));
        self.eviction_scratch
            .select_nth_unstable_by_key(remove_count - 1, |(stamp, _)| *stamp);
        for (_, key) in self.eviction_scratch.iter().take(remove_count) {
            self.entries.remove(key);
        }
    }

    pub(in crate::platform::nova) fn clear(&mut self) {
        self.entries.clear();
        self.eviction_scratch.clear();
        self.next_stamp = 0;
    }

    pub(in crate::platform::nova) fn shrink_to(&mut self, min_capacity: usize) {
        self.entries.shrink_to(min_capacity);
        self.eviction_scratch.shrink_to(min_capacity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: usize) -> PathRasterizationCacheKey {
        PathRasterizationCacheKey {
            path_id: crate::PathCacheId(id),
            generation: crate::PathGeometryGeneration(1),
            vertex_count: 3,
            geometry_hash: id as u64,
            paint_hash: 0,
        }
    }

    fn entry(value: u8) -> PathRasterizationCacheEntry {
        PathRasterizationCacheEntry {
            bytes: Arc::<[u8]>::from([value]),
            vertex_count: 3,
        }
    }

    #[test]
    fn eviction_preserves_recent_path_entries() {
        let mut cache = PathRasterizationCache::default();
        for id in 0..4 {
            cache.insert(key(id), entry(id as u8));
        }

        // Promote the two oldest inserts so the untouched newer entries become the cold half.
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.get(&key(1)).is_some());
        cache.evict_to_fit(2, usize::MAX);

        assert_eq!(cache.entries.len(), 2);
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.get(&key(1)).is_some());
        assert!(cache.get(&key(2)).is_none());
        assert!(cache.get(&key(3)).is_none());
    }

    #[test]
    fn byte_pressure_evicts_only_the_least_recent_payload() {
        let mut cache = PathRasterizationCache::default();
        let payload_len = MAX_PATH_RASTERIZATION_CACHE_BYTES / 3;
        for id in 0..3 {
            cache.insert(
                key(id),
                PathRasterizationCacheEntry {
                    bytes: vec![id as u8; payload_len].into(),
                    vertex_count: 3,
                },
            );
        }
        assert!(cache.get(&key(0)).is_some());
        let retired = Arc::downgrade(&cache.get(&key(1)).expect("cached path").bytes);
        // Refresh 0 and 2 after observing 1 so 1 remains the least recently used.
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.get(&key(2)).is_some());
        cache.insert(
            key(3),
            PathRasterizationCacheEntry {
                bytes: vec![3; payload_len].into(),
                vertex_count: 3,
            },
        );
        assert!(cache.retained_bytes <= MAX_PATH_RASTERIZATION_CACHE_BYTES);
        assert_eq!(cache.entries.len(), 3);
        assert!(cache.get(&key(1)).is_none());
        assert!(retired.upgrade().is_none());
        for id in [0, 2, 3] {
            assert!(cache.get(&key(id)).is_some());
        }
    }

    #[test]
    fn replacement_clear_and_single_entry_eviction_release_byte_charges() {
        let mut cache = PathRasterizationCache::default();
        cache.insert(key(0), entry(1));
        cache.insert(
            key(0),
            PathRasterizationCacheEntry {
                bytes: Arc::from([2; 8]),
                vertex_count: 3,
            },
        );
        assert_eq!(cache.retained_bytes, 8);
        cache.evict_to_fit(usize::MAX, 7);
        assert_eq!(cache.retained_bytes, 0);
        assert!(cache.entries.is_empty());
        cache.insert(key(1), entry(1));
        cache.clear();
        assert_eq!(cache.retained_bytes, 0);
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn oversized_path_does_not_evict_the_hot_working_set() {
        let mut cache = PathRasterizationCache::default();
        cache.insert(key(0), entry(1));
        let oversized = PathRasterizationCacheEntry {
            bytes: vec![0; MAX_PATH_RASTERIZATION_CACHE_BYTES + 1].into(),
            vertex_count: 3,
        };
        let released = Arc::downgrade(&oversized.bytes);
        cache.insert(key(1), oversized);
        assert!(released.upgrade().is_none());
        assert!(cache.get(&key(1)).is_none());
        assert!(cache.get(&key(0)).is_some());
        assert_eq!(cache.retained_bytes, 1);
    }

    #[test]
    fn repeated_hits_bound_recency_storage_and_preserve_eviction_order() {
        let mut cache = PathRasterizationCache::default();
        cache.insert(key(0), entry(0));
        cache.insert(key(1), entry(1));
        for _ in 0..10_000 {
            assert!(cache.get(&key(0)).is_some());
        }
        assert!(cache.access_order.len() <= 129);
        cache.evict_to_fit(1, usize::MAX);
        assert!(cache.get(&key(1)).is_none());
        assert!(cache.get(&key(0)).is_some());
    }

    fn geometry_memo(id: usize) -> PathGeometryHashMemo {
        PathGeometryHashMemo {
            generation: crate::PathGeometryGeneration(1),
            vertex_count: 3,
            first_xy_bits: (id as u32, 0),
            last_xy_bits: (id as u32, 1),
            geometry_hash: id as u64,
        }
    }

    #[test]
    fn geometry_hash_eviction_preserves_recent_paths() {
        let mut cache = PathGeometryHashCache::default();
        for id in 0..4 {
            cache.insert(crate::PathCacheId(id), geometry_memo(id));
        }

        assert!(cache.get(&crate::PathCacheId(0)).is_some());
        assert!(cache.get(&crate::PathCacheId(1)).is_some());
        cache.evict_cold_half();

        assert_eq!(cache.entries.len(), 2);
        assert!(cache.get(&crate::PathCacheId(0)).is_some());
        assert!(cache.get(&crate::PathCacheId(1)).is_some());
        assert!(cache.get(&crate::PathCacheId(2)).is_none());
        assert!(cache.get(&crate::PathCacheId(3)).is_none());
    }
}
