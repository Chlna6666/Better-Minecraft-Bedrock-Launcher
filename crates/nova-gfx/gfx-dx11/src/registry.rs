use gfx_core::{Error, ResourceId, Result};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

// IDs are process-wide and never reused: stale and foreign-device handles cannot alias.
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
}
fn invalid(id: u64) -> Error {
    Error::InvalidInput(format!("stale or foreign D3D11 resource {id}"))
}
