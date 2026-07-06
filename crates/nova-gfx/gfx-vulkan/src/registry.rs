use gfx_core::{GfxError, ResourceId, Result};

struct ResourceSlot<T> {
    generation: u32,
    value: Option<T>,
}

pub(crate) struct ResourceRegistry<T> {
    kind: &'static str,
    slots: Vec<ResourceSlot<T>>,
    free_indices: Vec<u32>,
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
            value: Some(value),
        });
        ResourceId::from_parts(index, 1)
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
        Ok(value)
    }

    pub(crate) fn replace_live<R>(&mut self, id: ResourceId<R>, value: T) -> Result<T> {
        let kind = self.kind;
        let slot = self.valid_slot_mut(id)?;
        slot.value
            .replace(value)
            .ok_or_else(|| stale_handle_error(kind, id))
    }

    pub(crate) fn live_len(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.value.is_some())
            .count()
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

fn stale_handle_error<R>(kind: &'static str, id: ResourceId<R>) -> GfxError {
    GfxError::InvalidInput(format!(
        "stale or invalid {kind} handle: index={}, generation={}",
        id.index(),
        id.generation()
    ))
}
