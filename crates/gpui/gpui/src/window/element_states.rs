use crate::{GlobalElementId, ImageElementState};
use collections::FxHashMap;
use std::any::{Any, TypeId};

pub(super) type ElementStateKey = (GlobalElementId, TypeId);

#[derive(Default)]
pub(super) struct ElementStates {
    slots: FxHashMap<ElementStateKey, Slot>,
    generation: u64,
    accessed: usize,
}

struct Slot {
    value: Option<Box<dyn Any>>,
    generation: u64,
}

impl ElementStates {
    pub(super) fn begin_frame(&mut self) {
        self.generation += 1;
        self.accessed = 0;
    }

    pub(super) fn take(&mut self, key: &ElementStateKey) -> Option<Box<dyn Any>> {
        self.slots.get_mut(key).and_then(|slot| slot.value.take())
    }

    pub(super) fn store(&mut self, key: ElementStateKey, value: Box<dyn Any>) {
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

    /// Retained views and scroll rows keep their states live by replaying access ranges.
    /// A rejected frame skips this sweep, preserving both unvisited states and values
    /// returned by element callbacks before the abort.
    pub(super) fn finish_frame<'a>(
        &mut self,
        retained_accesses: impl Iterator<Item = &'a ElementStateKey>,
    ) {
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
