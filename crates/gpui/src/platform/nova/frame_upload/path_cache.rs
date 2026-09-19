use super::*;

pub(in crate::platform::nova) const MAX_PATH_RASTERIZATION_CACHE_ENTRIES: usize = 4096;

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
/// entries arbitrarily. Accesses update a monotonic stamp and insertion evicts the coldest half only
/// when a new key would cross the retained-entry bound.
#[derive(Default)]
pub(in crate::platform::nova) struct PathRasterizationCache {
    entries: FxHashMap<PathRasterizationCacheKey, (PathRasterizationCacheEntry, u64)>,
    eviction_scratch: Vec<(u64, PathRasterizationCacheKey)>,
    next_stamp: u64,
}

impl PathRasterizationCache {
    #[inline]
    fn next_stamp(&mut self) -> u64 {
        self.next_stamp = self.next_stamp.wrapping_add(1);
        self.next_stamp
    }

    #[inline]
    pub(in crate::platform::nova) fn get(
        &mut self,
        key: &PathRasterizationCacheKey,
    ) -> Option<&PathRasterizationCacheEntry> {
        let stamp = self.next_stamp();
        let (entry, last_used) = self.entries.get_mut(key)?;
        *last_used = stamp;
        Some(entry)
    }

    #[inline]
    pub(in crate::platform::nova) fn insert(
        &mut self,
        key: PathRasterizationCacheKey,
        entry: PathRasterizationCacheEntry,
    ) {
        if self.entries.len() >= MAX_PATH_RASTERIZATION_CACHE_ENTRIES
            && !self.entries.contains_key(&key)
        {
            self.evict_cold_half();
        }
        let stamp = self.next_stamp();
        let _ = self.entries.insert(key, (entry, stamp));
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
        cache.evict_cold_half();

        assert_eq!(cache.entries.len(), 2);
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.get(&key(1)).is_some());
        assert!(cache.get(&key(2)).is_none());
        assert!(cache.get(&key(3)).is_none());
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
