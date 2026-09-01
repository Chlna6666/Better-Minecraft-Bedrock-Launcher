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
/// The encode path historically used `FxHashMap::retain` with a toggling predicate once the hard
/// limit was reached. Hash-map bucket order is unrelated to usage, so that discarded hot SVG/path
/// entries just as readily as cold ones. Keep the existing call surface while recording a monotonic
/// access stamp on get/insert and use that old retain hook to evict the coldest half instead.
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
    pub(in crate::platform::nova) fn len(&self) -> usize {
        self.entries.len()
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
    ) -> Option<PathRasterizationCacheEntry> {
        let stamp = self.next_stamp();
        self.entries
            .insert(key, (entry, stamp))
            .map(|(entry, _)| entry)
    }

    /// Compatibility hook for the existing encode-time half-eviction call.
    ///
    /// The predicate is intentionally not evaluated: its previous implementation only alternated
    /// true/false to remove roughly half of arbitrary hash buckets. Recency is the actual cache
    /// policy now, so remove the oldest half deterministically by access stamp instead.
    pub(in crate::platform::nova) fn retain<F>(&mut self, _predicate: F)
    where
        F: FnMut(&PathRasterizationCacheKey, &mut PathRasterizationCacheEntry) -> bool,
    {
        self.evict_cold_half();
    }

    fn evict_cold_half(&mut self) {
        let remove_count = self.entries.len() / 2;
        if remove_count == 0 {
            return;
        }

        self.eviction_scratch.clear();
        self.eviction_scratch.extend(
            self.entries
                .iter()
                .map(|(key, (_, stamp))| (*stamp, *key)),
        );
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
        cache.retain(|_, _| true);

        assert_eq!(cache.len(), 2);
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.get(&key(1)).is_some());
        assert!(cache.get(&key(2)).is_none());
        assert!(cache.get(&key(3)).is_none());
    }
}
