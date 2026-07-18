use gpui::Timer;
use rustc_hash::{FxHashMap, FxHasher};
use std::any::Any;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(super) const MAP_QUERY_CONCURRENCY: usize = 2;
pub(super) const MAP_QUERY_RETRY_INTERVAL: Duration = Duration::from_millis(8);
const MAP_QUERY_MEMORY_CACHE_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum MapQueryKind {
    Overlay,
    VillageIndex,
    SlimeRuns,
    SlimeCandidates,
    Selection,
    Detail,
    History,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct MapQueryCacheKey {
    pub(super) kind: MapQueryKind,
    pub(super) world_id: u64,
    pub(super) dimension_id: i32,
    pub(super) min_x: i32,
    pub(super) max_x: i32,
    pub(super) min_z: i32,
    pub(super) max_z: i32,
    pub(super) variant: u64,
}

impl MapQueryCacheKey {
    pub(super) fn new(
        kind: MapQueryKind,
        world_path: &Path,
        dimension_id: i32,
        bounds: (i32, i32, i32, i32),
        variant: u64,
    ) -> Self {
        Self {
            kind,
            world_id: world_identity(world_path),
            dimension_id,
            min_x: bounds.0,
            max_x: bounds.1,
            min_z: bounds.2,
            max_z: bounds.3,
            variant,
        }
    }
}

#[derive(Default)]
struct MemoryCache {
    values: FxHashMap<MapQueryCacheKey, Arc<dyn Any + Send + Sync>>,
    lru: VecDeque<MapQueryCacheKey>,
}

impl MemoryCache {
    fn get<T>(&mut self, key: MapQueryCacheKey) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        let value = self.values.get(&key)?.clone().downcast::<T>().ok()?;
        self.touch(key);
        Some(value)
    }

    fn insert<T>(&mut self, key: MapQueryCacheKey, value: Arc<T>)
    where
        T: Any + Send + Sync,
    {
        self.values.insert(key, value);
        self.touch(key);
        while self.values.len() > MAP_QUERY_MEMORY_CACHE_CAPACITY {
            let Some(oldest) = self.lru.pop_front() else {
                break;
            };
            self.values.remove(&oldest);
        }
    }

    fn touch(&mut self, key: MapQueryCacheKey) {
        self.lru.retain(|current| *current != key);
        self.lru.push_back(key);
    }
}

#[derive(Clone, Default)]
pub(super) struct MapQueryCoordinator {
    active: Arc<AtomicUsize>,
    generations: Arc<Mutex<FxHashMap<MapQueryKind, u64>>>,
    cache: Arc<Mutex<MemoryCache>>,
}

impl MapQueryCoordinator {
    pub(super) async fn acquire(&self) -> MapQueryPermit {
        loop {
            if let Some(permit) = self.try_acquire() {
                return permit;
            }
            Timer::after(MAP_QUERY_RETRY_INTERVAL).await;
        }
    }

    pub(super) fn try_acquire(&self) -> Option<MapQueryPermit> {
        self.active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAP_QUERY_CONCURRENCY.max(1)).then_some(active.saturating_add(1))
            })
            .ok()?;
        Some(MapQueryPermit {
            active: Arc::clone(&self.active),
        })
    }

    pub(super) fn next_generation(&self, kind: MapQueryKind) -> u64 {
        let Ok(mut generations) = self.generations.lock() else {
            return 1;
        };
        let generation = generations.entry(kind).or_default();
        *generation = generation.saturating_add(1);
        *generation
    }

    pub(super) fn is_current(&self, kind: MapQueryKind, generation: u64) -> bool {
        self.generations
            .lock()
            .ok()
            .and_then(|generations| generations.get(&kind).copied())
            == Some(generation)
    }

    pub(super) fn cached<T>(&self, key: MapQueryCacheKey) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        self.cache.lock().ok()?.get(key)
    }

    pub(super) fn cache<T>(&self, key: MapQueryCacheKey, value: Arc<T>)
    where
        T: Any + Send + Sync,
    {
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(key, value);
        }
    }

    pub(super) fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

pub(super) type MapQueryBudget = MapQueryCoordinator;

pub(super) struct MapQueryPermit {
    active: Arc<AtomicUsize>,
}

impl Drop for MapQueryPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

fn world_identity(path: &Path) -> u64 {
    let mut hasher = FxHasher::default();
    path.to_string_lossy().hash(&mut hasher);
    hasher.finish()
}
