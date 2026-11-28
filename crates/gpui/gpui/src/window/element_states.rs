use crate::{GlobalElementId, ImageElementState};
use collections::{FxHashMap, FxHashSet};
use std::any::{Any, TypeId};

pub(super) type ElementStateKey = (GlobalElementId, TypeId);

#[derive(Default)]
pub(super) struct ElementStates {
    slots: FxHashMap<ElementStateKey, Slot>,
    generation: u64,
    accessed: usize,
    boundary: Option<BoundaryStates>,
    // Built lazily only for a window that actually commits stateful
    // variable boundaries; discarded on ordinary traversal.
    boundary_access_counts: Option<FxHashMap<ElementStateKey, usize>>,
    boundary_slots_validated: bool,
}

/// Only newly materialized states are held here. Never move a committed
/// non-Clone value into a speculative boundary callback.
#[derive(Default)]
struct BoundaryStates {
    fresh: FxHashMap<ElementStateKey, Box<dyn Any>>,
    conflict: bool,
}

/// Prepared under exclusive Window ownership. No callbacks or fallible
/// admission may run between preparation and final transfer.
pub(super) struct PreparedBoundaryStates {
    retired_keys: Vec<ElementStateKey>,
    retired: Vec<Box<dyn Any>>,
    counts: Option<Vec<(ElementStateKey, usize)>>,
}

struct Slot {
    value: Option<Box<dyn Any>>,
    generation: u64,
}

impl ElementStates {
    pub(super) fn begin_frame(&mut self) {
        debug_assert!(self.boundary.is_none());
        self.boundary_access_counts = None;
        self.boundary_slots_validated = false;
        self.generation += 1;
        self.accessed = 0;
    }

    pub(super) fn take(&mut self, key: &ElementStateKey) -> Option<Box<dyn Any>> {
        if let Some(boundary) = self.boundary.as_mut() {
            if self.slots.contains_key(key) {
                boundary.conflict = true;
                return None;
            }
            return boundary.fresh.remove(key);
        }
        self.boundary_slots_validated = false;
        self.boundary_access_counts = None;
        self.slots.get_mut(key).and_then(|slot| slot.value.take())
    }

    pub(super) fn store(&mut self, key: ElementStateKey, value: Box<dyn Any>) {
        if let Some(boundary) = self.boundary.as_mut() {
            if self.slots.contains_key(&key) {
                boundary.conflict = true;
            }
            boundary.fresh.insert(key, value);
            return;
        }
        self.boundary_slots_validated = false;
        self.boundary_access_counts = None;
        let slot = self.slots.entry(key).or_insert(Slot {
            value: None,
            generation: 0,
        });
        slot.value = Some(value);
        if slot.generation != self.generation {
            slot.generation = self.generation;
            self.accessed += 1;
        }
    }

    /// Begin an isolated fresh-state transaction. Existing states must be
    /// retained by their access records, not lent to an entering row.
    #[allow(dead_code)]
    pub(super) fn begin_boundary(&mut self) -> Option<()> {
        if self.boundary.is_some() { return None; }
        if !self.boundary_slots_validated {
            if self.slots.values().any(|slot| slot.value.is_none()) { return None; }
            self.boundary_slots_validated = true;
        }
        self.boundary = Some(BoundaryStates::default());
        Some(())
    }

    pub(super) fn boundary_is_untouched(&self) -> bool {
        self.boundary.as_ref().is_some_and(|boundary| !boundary.conflict && boundary.fresh.is_empty())
    }

    #[allow(dead_code)]
    pub(super) fn cancel_boundary(&mut self) {
        self.boundary = None;
    }

    /// Reserve every destination and validate the candidate access log
    /// before transferring ownership. Retire values into caller-owned
    /// storage so destructors cannot reenter before Frame publication.
    #[allow(dead_code)]
    pub(super) fn prepare_boundary<'a>(
        &mut self, accesses: impl Iterator<Item = &'a ElementStateKey>,
    ) -> Option<PreparedBoundaryStates> {
        let boundary = self.boundary.as_ref()?;
        if boundary.conflict { return None; }
        let mut live = FxHashSet::default();
        for key in accesses {
            if !self.slots.contains_key(key) && !boundary.fresh.contains_key(key) {
                return None;
            }
            live.insert(key);
        }
        let mut retired_keys = Vec::new();
        for key in self.slots.keys() {
            if !live.contains(key) { retired_keys.push(key.clone()); }
        }
        // All fresh values must have a native access record.
        if !boundary.fresh.keys().all(|key| live.contains(key)) { return None; }
        let fresh_count = boundary.fresh.len();
        self.slots.try_reserve(fresh_count).ok()?;
        let mut retired = Vec::new();
        retired.try_reserve_exact(retired_keys.len()).ok()?;
        Some(PreparedBoundaryStates { retired_keys, retired, counts: None })
    }

    /// After the initial exact count construction, admission visits only
    /// edited access ranges and the fresh values. Shared keys survive until
    /// their final reference retires. No second state ownership table.
    pub(super) fn prepare_boundary_delta<'a>(
        &mut self,
        committed: impl Iterator<Item = &'a ElementStateKey>,
        removed: impl Iterator<Item = &'a ElementStateKey>,
        inserted: impl Iterator<Item = &'a ElementStateKey>,
    ) -> Option<PreparedBoundaryStates> {
        let boundary = self.boundary.as_ref()?;
        if boundary.conflict { return None; }
        if self.boundary_access_counts.is_none() {
            let mut counts = FxHashMap::<ElementStateKey, usize>::default();
            for key in committed {
                if !self.slots.contains_key(key) { return None; }
                let count = counts.entry(key.clone()).or_default();
                *count = count.checked_add(1)?;
            }
            // If a custom element owns an unlogged slot, do not infer
            // its lifetime from a partial access stream.
            if counts.len() != self.slots.len() { return None; }
            self.boundary_access_counts = Some(counts);
        }
        let counts = self.boundary_access_counts.as_mut()?;
        let mut changes = FxHashMap::<ElementStateKey, usize>::default();
        for key in removed {
            let current = changes.entry(key.clone()).or_insert_with(|| counts.get(key).copied().unwrap_or(0));
            *current = current.checked_sub(1)?;
        }
        for key in inserted {
            if !self.slots.contains_key(key) && !boundary.fresh.contains_key(key) { return None; }
            let current = changes.entry(key.clone()).or_insert_with(|| counts.get(key).copied().unwrap_or(0));
            *current = current.checked_add(1)?;
        }
        if !boundary.fresh.keys().all(|key| changes.get(key).is_some_and(|count| *count != 0)) { return None; }
        let retired_keys: Vec<_> = changes.iter().filter_map(|(key, &count)|
            (count == 0 && self.slots.contains_key(key)).then(|| key.clone())).collect();
        let fresh_count = boundary.fresh.len();
        self.slots.try_reserve(fresh_count).ok()?;
        counts.try_reserve(fresh_count).ok()?;
        let mut retired = Vec::new();
        retired.try_reserve_exact(retired_keys.len()).ok()?;
        Some(PreparedBoundaryStates { retired_keys, retired, counts: Some(changes.into_iter().collect()) })
    }

    #[allow(dead_code)]
    pub(super) fn commit_boundary(&mut self, mut prepared: PreparedBoundaryStates) -> Vec<Box<dyn Any>> {
        let boundary = self.boundary.take().expect("validated boundary");
        debug_assert!(!boundary.conflict);
        for key in prepared.retired_keys {
            if let Some(slot) = self.slots.remove(&key) {
                prepared.retired.extend(slot.value);
            }
        }
        for (key, value) in boundary.fresh {
            self.slots.insert(key, Slot { value: Some(value), generation: self.generation });
        }
        // A boundary does not advance the ordinary state generation.
        // All surviving slots are owned by the validated access log.
        self.accessed = self.slots.len();
        if let Some(updates) = prepared.counts {
            let counts = self.boundary_access_counts.as_mut().expect("validated access counts");
            for (key, count) in updates {
                if count == 0 { counts.remove(&key); }
                else { counts.insert(key, count); }
            }
        } else {
            self.boundary_access_counts = None;
        }
        self.boundary_slots_validated = true;
        prepared.retired
    }

    #[cfg(test)]
    fn adopt_boundary<'a>(&mut self, accesses: impl Iterator<Item = &'a ElementStateKey>) -> Option<Vec<Box<dyn Any>>> {
        let prepared = self.prepare_boundary(accesses)?;
        Some(self.commit_boundary(prepared))
    }

    /// Retained views and scroll rows keep their states live by replaying access ranges.
    /// A rejected frame skips this sweep, preserving both unvisited states and values
    /// returned by element callbacks before the abort.
    pub(super) fn finish_frame<'a>(
        &mut self,
        retained_accesses: impl Iterator<Item = &'a ElementStateKey>,
    ) {
        debug_assert!(self.boundary.is_none());
        if self.accessed == self.slots.len() {
            return;
        }
        for key in retained_accesses {
            if let Some(slot) = self.slots.get_mut(key)
                && slot.value.is_some()
                && slot.generation != self.generation
            {
                slot.generation = self.generation;
                self.accessed += 1;
                if self.accessed == self.slots.len() {
                    return;
                }
            }
        }
        self.slots
            .retain(|_, slot| slot.generation == self.generation && slot.value.is_some());
        self.trim(16, false);
    }

    pub(super) fn capacity(&self) -> usize {
        self.slots.capacity()
    }

    pub(super) fn trim(&mut self, floor: usize, eager: bool) {
        let target = floor.max(self.slots.len());
        if eager || self.slots.capacity() > target.saturating_mul(4) {
            self.slots.shrink_to(target);
        }
    }

    pub(super) fn release_image_bitmaps(&mut self) {
        if self.boundary.is_some() { return; }
        for ((_, type_id), slot) in &mut self.slots {
            if *type_id == TypeId::of::<ImageElementState>()
                && let Some(state) = slot
                    .value
                    .as_deref_mut()
                    .and_then(|state| state.downcast_mut::<Option<ImageElementState>>())
                    .and_then(Option::as_mut)
            {
                state.current_image = None;
                state.current_frame = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_state_collision_does_not_move_committed_value() {
        let key = (GlobalElementId::default(), TypeId::of::<u32>());
        let mut states = ElementStates::default();
        states.begin_frame();
        states.store(key.clone(), Box::new(7_u32));
        states.finish_frame(std::iter::empty());
        states.begin_boundary().unwrap();
        assert!(states.take(&key).is_none());
        states.store(key.clone(), Box::new(99_u32));
        assert!(states.adopt_boundary([&key].into_iter()).is_none());
        states.cancel_boundary();
        assert_eq!(*states.take(&key).unwrap().downcast::<u32>().unwrap(), 7);
    }

    #[test]
    fn boundary_delta_counts_shared_owners_and_reuses_index() {
        let shared = (GlobalElementId::default(), TypeId::of::<u64>());
        let fresh = (GlobalElementId::default(), TypeId::of::<u32>());
        let mut states = ElementStates::default();
        states.begin_frame();
        states.store(shared.clone(), Box::new(8_u64));
        states.finish_frame(std::iter::empty());
        states.begin_boundary().unwrap();
        states.store(fresh.clone(), Box::new(9_u32));
        let prepared = states.prepare_boundary_delta(
            [&shared, &shared].into_iter(), [&shared].into_iter(), [&fresh].into_iter(),
        ).unwrap();
        assert!(states.commit_boundary(prepared).is_empty());
        assert_eq!(states.boundary_access_counts.as_ref().unwrap()[&shared], 1);
        states.begin_boundary().unwrap();
        // A second transaction must not traverse the full committed log.
        let prepared = states.prepare_boundary_delta(
            std::iter::from_fn(|| -> Option<&ElementStateKey> { panic!("full log scanned") }),
            [&shared].into_iter(), std::iter::empty(),
        ).unwrap();
        let retired = states.commit_boundary(prepared);
        assert_eq!(retired.len(), 1);
        assert!(!states.slots.contains_key(&shared));
        assert!(states.slots.contains_key(&fresh));
        assert_eq!(states.boundary_access_counts.as_ref().unwrap()[&fresh], 1);
    }

    #[test]
    fn boundary_adopts_new_states_and_defers_retirement() {
        use std::{cell::Cell, rc::Rc};
        struct Owner(Rc<Cell<usize>>);
        impl Drop for Owner {
            fn drop(&mut self) { self.0.set(self.0.get() + 1); }
        }
        let kept = (GlobalElementId::default(), TypeId::of::<u64>());
        let gone = (GlobalElementId::default(), TypeId::of::<Owner>());
        let fresh = (GlobalElementId::default(), TypeId::of::<u32>());
        let drops = Rc::new(Cell::new(0));
        let mut states = ElementStates::default();
        states.begin_frame();
        states.store(kept.clone(), Box::new(8_u64));
        states.store(gone.clone(), Box::new(Owner(drops.clone())));
        states.finish_frame(std::iter::empty());
        let before = states.slots[&kept].value.as_ref().unwrap().as_ref() as *const dyn Any;
        states.begin_boundary().unwrap();
        states.store(fresh.clone(), Box::new(9_u32));
        assert!(states.adopt_boundary([&kept].into_iter()).is_none());
        let retired = states.adopt_boundary([&kept, &fresh, &kept].into_iter()).unwrap();
        assert_eq!(drops.get(), 0);
        assert!(!states.slots.contains_key(&gone));
        let after = states.slots[&kept].value.as_ref().unwrap().as_ref() as *const dyn Any;
        assert!(std::ptr::addr_eq(before, after));
        drop(retired);
        assert_eq!(drops.get(), 1);
        assert_eq!(*states.take(&fresh).unwrap().downcast::<u32>().unwrap(), 9);
    }

    #[test]
    fn retains_shared_state_and_releases_unreferenced_state_on_commit() {
        let direct = (GlobalElementId::default(), TypeId::of::<u32>());
        let replayed = (GlobalElementId::default(), TypeId::of::<u64>());
        let gone = (GlobalElementId::default(), TypeId::of::<String>());
        let mut states = ElementStates::default();
        states.begin_frame();
        states.store(direct.clone(), Box::new(1_u32));
        states.store(replayed.clone(), Box::new(2_u64));
        states.store(gone.clone(), Box::new(String::from("old page")));
        let before = states.slots[&replayed].value.as_ref().unwrap().as_ref() as *const dyn Any;
        states.finish_frame(std::iter::empty());

        states.begin_frame();
        let direct_value = states.take(&direct).unwrap();
        states.store(direct.clone(), direct_value);
        states.finish_frame([&direct, &replayed, &replayed].into_iter());

        let after = states.slots[&replayed].value.as_ref().unwrap().as_ref() as *const dyn Any;
        assert!(std::ptr::addr_eq(before, after));
        assert!(!states.slots.contains_key(&gone));
    }

    #[test]
    fn abandoned_frame_preserves_unvisited_and_moved_values() {
        let visited = (GlobalElementId::default(), TypeId::of::<u32>());
        let unvisited = (GlobalElementId::default(), TypeId::of::<u64>());
        let mut states = ElementStates::default();
        states.begin_frame();
        states.store(visited.clone(), Box::new(1_u32));
        states.store(unvisited.clone(), Box::new(2_u64));
        states.finish_frame(std::iter::empty());

        states.begin_frame();
        let mut value = states.take(&visited).unwrap();
        *value.downcast_mut::<u32>().unwrap() = 3;
        states.store(visited.clone(), value);

        states.begin_frame();
        assert_eq!(
            *states.take(&visited).unwrap().downcast::<u32>().unwrap(),
            3
        );
        assert_eq!(
            *states.take(&unvisited).unwrap().downcast::<u64>().unwrap(),
            2
        );
    }
}
