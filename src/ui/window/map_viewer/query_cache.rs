use futures::channel::oneshot;
use rustc_hash::{FxHashMap, FxHasher};
use std::any::Any;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub(super) const MAP_QUERY_CONCURRENCY: usize = 2;
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

#[derive(Default)]
struct MapQueryGate {
    active: usize,
    waiters: VecDeque<oneshot::Sender<()>>,
}

#[derive(Clone, Default)]
pub(super) struct MapQueryCoordinator {
    gate: Arc<Mutex<MapQueryGate>>,
    generations: Arc<Mutex<FxHashMap<MapQueryKind, u64>>>,
    cache: Arc<Mutex<MemoryCache>>,
}

impl MapQueryCoordinator {
    pub(super) async fn acquire(&self) -> MapQueryPermit {
        loop {
            let receiver = {
                let mut gate = self
                    .gate
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if gate.active < MAP_QUERY_CONCURRENCY.max(1) {
                    gate.active = gate.active.saturating_add(1);
                    return MapQueryPermit {
                        gate: Arc::clone(&self.gate),
                    };
                }

                let (sender, receiver) = oneshot::channel();
                gate.waiters.push_back(sender);
                receiver
            };

            // Releases wake the current waiter set. A cancelled waiter simply drops its receiver;
            // the remaining waiters are woken by the same release and re-contend for the slot.
            let _ = receiver.await;
        }
    }

    pub(super) fn try_acquire(&self) -> Option<MapQueryPermit> {
        let mut gate = self
            .gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if gate.active >= MAP_QUERY_CONCURRENCY.max(1) {
            return None;
        }
        gate.active = gate.active.saturating_add(1);
        Some(MapQueryPermit {
            gate: Arc::clone(&self.gate),
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
        self.gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .active
    }
}

pub(super) type MapQueryBudget = MapQueryCoordinator;

pub(super) struct MapQueryPermit {
    gate: Arc<Mutex<MapQueryGate>>,
}

impl Drop for MapQueryPermit {
    fn drop(&mut self) {
        let waiters = {
            let mut gate = self
                .gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            gate.active = gate.active.saturating_sub(1);
            std::mem::take(&mut gate.waiters)
        };

        // Wake the complete current waiter set so cancellation cannot consume the only wakeup and
        // strand a free permit. Only the first contenders up to the concurrency limit can acquire;
        // the rest atomically re-register themselves without polling.
        for waiter in waiters {
            let _ = waiter.send(());
        }
    }
}

fn world_identity(path: &Path) -> u64 {
    let mut hasher = FxHasher::default();
    path.to_string_lossy().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_budget_enforces_concurrency_and_releases_slots() {
        let budget = MapQueryCoordinator::default();
        let first = budget.try_acquire().expect("first permit");
        let second = budget.try_acquire().expect("second permit");

        assert_eq!(budget.active(), MAP_QUERY_CONCURRENCY);
        assert!(budget.try_acquire().is_none());

        drop(first);
        assert_eq!(budget.active(), MAP_QUERY_CONCURRENCY - 1);
        let replacement = budget.try_acquire().expect("replacement permit");
        assert_eq!(budget.active(), MAP_QUERY_CONCURRENCY);

        drop(replacement);
        drop(second);
        assert_eq!(budget.active(), 0);
    }
}
