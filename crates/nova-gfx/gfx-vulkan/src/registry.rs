use gfx_core::{Error, ResourceId, Result};

struct ResourceSlot<T> {
    generation: u32,
    native_revision: u64,
    value: Option<T>,
}

pub(crate) struct ResourceRegistry<T> {
    kind: &'static str,
    slots: Vec<ResourceSlot<T>>,
    free_indices: Vec<u32>,
    identity_epoch: u64,
}

impl<T> Default for ResourceRegistry<T> {
    fn default() -> Self {
        Self::new("resource")
    }
}

impl<T> ResourceRegistry<T> {
    pub(crate) fn new(kind: &'static str) -> Self {
        Self {
            kind,
            slots: Vec::new(),
            free_indices: Vec::new(),
            identity_epoch: 0,
        }
    }

    pub(crate) fn insert<R>(&mut self, value: T) -> ResourceId<R> {
        if let Some(index) = self.free_indices.pop() {
            let slot = &mut self.slots[index as usize];
            slot.value = Some(value);
            return ResourceId::from_parts(index, slot.generation);
        }
        let index = u32::try_from(self.slots.len()).unwrap_or(u32::MAX);
        self.slots.push(ResourceSlot {
            generation: 1,
            native_revision: 0,
            value: Some(value),
        });
        ResourceId::from_parts(index, 1)
    }

    /// Changes only when an existing native identity can become invalid.
    /// Uploads and ordinary mutable resource state do not invalidate caches.
    pub(crate) fn identity_epoch(&self) -> u64 { self.identity_epoch }

    pub(crate) fn native_stamp<R>(&self, id: ResourceId<R>) -> Result<u64> {
        let slot = self.valid_slot(id)?;
        if slot.value.is_none() { return Err(stale_handle_error(self.kind, id)); }
        Ok(slot.native_revision)
    }

    pub(crate) fn get<R>(&self, id: ResourceId<R>) -> Result<&T> {
        let slot = self.valid_slot(id)?;
        slot.value
            .as_ref()
            .ok_or_else(|| stale_handle_error(self.kind, id))
    }

    pub(crate) fn get_mut<R>(&mut self, id: ResourceId<R>) -> Result<&mut T> {
        let kind = self.kind;
        let slot = self.valid_slot_mut(id)?;
        slot.value
            .as_mut()
            .ok_or_else(|| stale_handle_error(kind, id))
    }

    pub(crate) fn take<R>(&mut self, id: ResourceId<R>) -> Result<T> {
        let index = id.index();
        let kind = self.kind;
        let slot = self.valid_slot_mut(id)?;
        let value = slot
            .value
            .take()
            .ok_or_else(|| stale_handle_error(kind, id))?;
        slot.generation = slot.generation.wrapping_add(1).max(1);
        self.free_indices.push(index);
        self.identity_epoch = self.identity_epoch.checked_add(1).expect("resource identity epoch exhausted");
        Ok(value)
    }

    pub(crate) fn replace_live<R>(&mut self, id: ResourceId<R>, value: T) -> Result<T> {
        let kind = self.kind;
        let slot = self.valid_slot_mut(id)?;
        if slot.value.is_none() { return Err(stale_handle_error(kind, id)); }
        slot.native_revision = slot.native_revision.checked_add(1).expect("native resource revision exhausted");
        let previous = slot.value.replace(value).expect("validated live resource");
        self.identity_epoch = self.identity_epoch.checked_add(1).expect("resource identity epoch exhausted");
        Ok(previous)
    }

    pub(crate) fn live_len(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.value.is_some())
            .count()
    }

    pub(crate) fn live_ids<R>(&self) -> Vec<ResourceId<R>> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.value
                    .as_ref()
                    .map(|_| ResourceId::from_parts(index as u32, slot.generation))
            })
            .collect()
    }

    pub(crate) fn for_each_live(&self, mut visitor: impl FnMut(&T)) {
        for slot in &self.slots {
            if let Some(value) = slot.value.as_ref() {
                visitor(value);
            }
        }
    }

    pub(crate) fn for_each_live_mut(&mut self, mut visitor: impl FnMut(&mut T)) {
        for slot in &mut self.slots {
            if let Some(value) = slot.value.as_mut() {
                visitor(value);
            }
        }
    }

    pub(crate) fn drain_live(&mut self) -> Vec<(u32, T)> {
        let mut values = Vec::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if let Some(value) = slot.value.take() {
                values.push((u32::try_from(index).unwrap_or(u32::MAX), value));
                slot.generation = slot.generation.wrapping_add(1).max(1);
            }
        }
        self.free_indices.clear();
        if !values.is_empty() { self.identity_epoch = self.identity_epoch.checked_add(1).expect("resource identity epoch exhausted"); }
        values
    }

    pub(crate) fn remove_where(&mut self, mut predicate: impl FnMut(&T) -> bool) -> usize {
        let mut removed = 0;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let should_remove = slot.value.as_ref().is_some_and(&mut predicate);
            if should_remove {
                slot.value = None;
                slot.generation = slot.generation.wrapping_add(1).max(1);
                self.free_indices
                    .push(u32::try_from(index).unwrap_or(u32::MAX));
                removed += 1;
            }
        }
        if removed != 0 { self.identity_epoch = self.identity_epoch.checked_add(1).expect("resource identity epoch exhausted"); }
        removed
    }

    fn valid_slot<R>(&self, id: ResourceId<R>) -> Result<&ResourceSlot<T>> {
        let slot = self
            .slots
            .get(id.index() as usize)
            .ok_or_else(|| stale_handle_error(self.kind, id))?;
        if slot.generation != id.generation() {
            return Err(stale_handle_error(self.kind, id));
        }
        Ok(slot)
    }

    fn valid_slot_mut<R>(&mut self, id: ResourceId<R>) -> Result<&mut ResourceSlot<T>> {
        let kind = self.kind;
        let slot = self
            .slots
            .get_mut(id.index() as usize)
            .ok_or_else(|| stale_handle_error(kind, id))?;
        if slot.generation != id.generation() {
            return Err(stale_handle_error(kind, id));
        }
        Ok(slot)
    }
}

fn stale_handle_error<R>(kind: &'static str, id: ResourceId<R>) -> Error {
    Error::InvalidInput(format!(
        "stale or invalid {kind} handle: index={}, generation={}",
        id.index(),
        id.generation()
    ))
}

#[cfg(test)]
mod native_identity_tests {
    use super::*;
    #[test]
    fn native_stamp_changes_only_on_replacement_or_retirement() {
        let mut registry = ResourceRegistry::new("test");
        let id: ResourceId<()> = registry.insert(1u32);
        let stamp = registry.native_stamp(id).unwrap();
        let epoch = registry.identity_epoch();
        *registry.get_mut(id).unwrap() = 2;
        assert_eq!(registry.identity_epoch(), epoch);
        assert_eq!(registry.native_stamp(id).unwrap(), stamp);
        let other: ResourceId<()> = registry.insert(3);
        registry.take(other).unwrap();
        assert_ne!(registry.identity_epoch(), epoch);
        assert_eq!(registry.native_stamp(id).unwrap(), stamp);
        registry.replace_live(id, 4).unwrap();
        assert_ne!(registry.native_stamp(id).unwrap(), stamp);
        registry.take(id).unwrap();
        assert!(registry.native_stamp(id).is_err());
    }
}
