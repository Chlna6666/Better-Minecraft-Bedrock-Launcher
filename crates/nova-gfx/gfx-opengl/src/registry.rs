use gfx_core::{Error, ResourceId, Result};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
pub(crate) struct Registry<T>(HashMap<u64, T>);
impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self(HashMap::new())
    }
}
impl<T> Registry<T> {
    pub(crate) fn insert<R>(&mut self, value: T) -> ResourceId<R> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        self.0.insert(id, value);
        ResourceId::new(id)
    }
    pub(crate) fn get<R>(&self, id: ResourceId<R>) -> Result<&T> {
        self.0.get(&id.raw()).ok_or_else(|| invalid(id.raw()))
    }
    pub(crate) fn get_mut<R>(&mut self, id: ResourceId<R>) -> Result<&mut T> {
        self.0.get_mut(&id.raw()).ok_or_else(|| invalid(id.raw()))
    }
    pub(crate) fn take<R>(&mut self, id: ResourceId<R>) -> Result<T> {
        self.0.remove(&id.raw()).ok_or_else(|| invalid(id.raw()))
    }
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
    pub(crate) fn values(&self) -> impl Iterator<Item = &T> {
        self.0.values()
    }

    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.0.values_mut()
    }

    pub(crate) fn trim(&mut self) {
        self.0.shrink_to_fit();
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_reclaims_idle_capacity_without_invalidating_live_ids() {
        let mut registry = Registry::default();
        let ids = (0..1024)
            .map(|value| registry.insert::<()>(value))
            .collect::<Vec<_>>();
        for id in &ids[1..] {
            registry.take(*id).expect("live entry");
        }
        let before = registry.0.capacity();
        registry.trim();
        assert!(registry.0.capacity() < before);
        assert_eq!(*registry.get(ids[0]).expect("retained entry"), 0);
        assert!(registry.get(ids[1]).is_err());
        registry.take(ids[0]).expect("last entry");
        registry.trim();
        assert_eq!(registry.0.capacity(), 0);
    }
}
fn invalid(id: u64) -> Error {
    Error::InvalidInput(format!("stale or foreign OpenGL resource {id}"))
}
