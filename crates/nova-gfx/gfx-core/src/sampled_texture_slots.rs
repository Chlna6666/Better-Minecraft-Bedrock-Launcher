//! Stable sampled-image indices for immutable native descriptor snapshots.
use std::collections::{BTreeMap, HashMap, HashSet};
use crate::{BindGroupId, BindGroupLayoutId, Error, ResourceDevice, Result, SubmissionId, TextureViewId};

/// CPU identity of a GPU texture index. The generation is checked on the
/// CPU before publishing a material; the GPU consumes only the index.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SampledTextureSlot {
    index: u32,
    generation: u32,
}
impl SampledTextureSlot {
    /// Stable shader-visible index.
    #[must_use]
    pub const fn index(self) -> u32 { self.index }
    /// CPU-side stale-handle discriminator.
    #[must_use]
    pub const fn generation(self) -> u32 { self.generation }
}

#[derive(Clone, Copy, Debug)]
enum State {
    Free,
    Live,
    Retired { remaining: usize },
    Exhausted,
}
struct Slot {
    generation: u32,
    state: State,
}

/// A bounded, device-owner-local descriptor slot allocator. Native snapshots
/// are immutable; retiring a slot clears the next snapshot but does not make
/// the index reusable until every supplied GPU completion has been observed.
///
/// Callers must include every actual GPU reader of the previous material/slot
/// across all queues, including pending old descriptor snapshots. A GPU-side
/// queue wait is not itself completion. Texture views and old BindGroupIds
/// remain caller-owned and must be retained separately through those readers.
pub struct SampledTextureSlots {
    fallback: TextureViewId,
    slots: Vec<Slot>,
    views: Vec<TextureViewId>,
    free: Vec<u32>,
    dirty: BTreeMap<u32, TextureViewId>,
    waiting: HashMap<SubmissionId, Vec<SampledTextureSlot>>,
}

impl SampledTextureSlots {
    /// Reserve index zero for the permanent fallback. All unused indices
    /// initially reference the fallback, including on non-sparse hardware.
    pub fn new(capacity: u32, fallback: TextureViewId) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::InvalidInput("sampled table capacity must be nonzero".into()));
        }
        let mut slots = Vec::with_capacity(capacity as usize);
        slots.push(Slot { generation: 1, state: State::Live });
        slots.resize_with(capacity as usize, || Slot { generation: 1, state: State::Free });
        Ok(Self {
            fallback, slots, views: vec![fallback; capacity as usize],
            free: (1..capacity).rev().collect(), dirty: BTreeMap::new(), waiting: HashMap::new(),
        })
    }

    /// Index zero is always available for materials lacking a texture.
    #[must_use]
    pub const fn fallback_index(&self) -> u32 { 0 }

    /// Allocate an index without scanning the entire table.
    pub fn allocate(&mut self, view: TextureViewId) -> Result<SampledTextureSlot> {
        let index = self.free.pop().ok_or_else(|| Error::Unavailable("sampled texture slots exhausted".into()))?;
        let slot = &mut self.slots[index as usize];
        debug_assert!(matches!(slot.state, State::Free));
        slot.state = State::Live;
        let handle = SampledTextureSlot { index, generation: slot.generation };
        self.views[index as usize] = view;
        self.dirty.insert(index, view);
        Ok(handle)
    }

    /// Validate a CPU identity before writing its raw index into GPU metadata.
    #[must_use]
    pub fn contains(&self, handle: SampledTextureSlot) -> bool {
        self.slots.get(handle.index as usize).is_some_and(|slot|
            slot.generation == handle.generation && matches!(slot.state, State::Live))
    }

    /// Invalidate a live index and clear it in the next native snapshot.
    /// The input must enumerate the actual accepted, still-uncompleted
    /// native submissions that could read this slot's previous generation.
    pub fn retire(&mut self, handle: SampledTextureSlot, readers: &[SubmissionId]) -> Result<()> {
        if handle.index == 0 || !self.contains(handle) {
            return Err(Error::InvalidInput("stale or reserved sampled texture slot".into()));
        }
        let readers: HashSet<_> = readers.iter().copied().collect();
        self.views[handle.index as usize] = self.fallback;
        self.dirty.insert(handle.index, self.fallback);
        if readers.is_empty() {
            self.recycle(handle);
        } else {
            self.slots[handle.index as usize].state = State::Retired { remaining: readers.len() };
            for reader in readers {
                self.waiting.entry(reader).or_default().push(handle);
            }
        }
        Ok(())
    }

    /// Report real successful GPU completion, not CPU submit or a queued wait.
    /// Only slots dependent on this submission are visited.
    pub fn complete(&mut self, submission: SubmissionId) {
        let Some(handles) = self.waiting.remove(&submission) else { return; };
        for handle in handles {
            let slot = &mut self.slots[handle.index as usize];
            debug_assert_eq!(slot.generation, handle.generation);
            let State::Retired { remaining } = &mut slot.state else { continue; };
            *remaining -= 1;
            if *remaining == 0 { self.recycle(handle); }
        }
    }

    fn recycle(&mut self, handle: SampledTextureSlot) {
        let slot = &mut self.slots[handle.index as usize];
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            slot.state = State::Free;
            self.free.push(handle.index);
        } else {
            // Never wrap a generation and accidentally validate a stale handle.
            slot.state = State::Exhausted;
        }
    }

    /// Publish the first native immutable snapshot; only clear the dirty
    /// journal after the backend successfully creates the table.
    pub fn create_snapshot(&mut self, device: &mut impl ResourceDevice, layout: BindGroupLayoutId) -> Result<BindGroupId> {
        let snapshot = device.create_sampled_texture_table(layout, &self.views)?;
        self.dirty.clear();
        Ok(snapshot)
    }

    /// Publish changes over the caller's current snapshot. None means no
    /// native allocation was needed. On failure the dirty journal is intact.
    /// The caller must retire the old snapshot separately after its last use.
    pub fn publish(&mut self, device: &mut impl ResourceDevice, current: BindGroupId) -> Result<Option<BindGroupId>> {
        if self.dirty.is_empty() { return Ok(None); }
        let changes: Vec<_> = self.dirty.iter().map(|(&index, &view)| (index, view)).collect();
        let snapshot = device.fork_sampled_texture_table(current, &changes)?;
        self.dirty.clear();
        Ok(Some(snapshot))
    }

    /// Sorted pending changes, exposed for diagnostics and tests.
    pub fn dirty_changes(&self) -> impl Iterator<Item = (u32, TextureViewId)> + '_ {
        self.dirty.iter().map(|(&index, &view)| (index, view))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_reuse_requires_every_distinct_native_reader() {
        let fallback = TextureViewId::from_parts(1, 1);
        let texture = TextureViewId::from_parts(2, 1);
        let a = SubmissionId::from_parts(10, 1);
        let b = SubmissionId::from_parts(11, 1);
        let mut slots = SampledTextureSlots::new(2, fallback).unwrap();
        let first = slots.allocate(texture).unwrap();
        assert_eq!(first.index(), 1);
        assert!(slots.contains(first));
        slots.retire(first, &[a, b, a]).unwrap();
        assert!(!slots.contains(first));
        assert_eq!(slots.dirty_changes().collect::<Vec<_>>(), [(1, fallback)]);
        assert!(slots.allocate(texture).is_err());
        slots.complete(b);
        slots.complete(b);
        assert!(slots.allocate(texture).is_err());
        slots.complete(a);
        let second = slots.allocate(texture).unwrap();
        assert_eq!(second.index(), first.index());
        assert_ne!(second.generation(), first.generation());
        assert!(!slots.contains(first));
        assert!(slots.retire(first, &[]).is_err());
        assert_eq!(slots.dirty_changes().collect::<Vec<_>>(), [(1, texture)]);
    }

    #[test]
    fn no_reader_retirement_is_immediate_and_dirty_edits_coalesce() {
        let fallback = TextureViewId::from_parts(1, 1);
        let texture = TextureViewId::from_parts(2, 1);
        let mut slots = SampledTextureSlots::new(3, fallback).unwrap();
        let a = slots.allocate(texture).unwrap();
        let b = slots.allocate(texture).unwrap();
        slots.retire(a, &[]).unwrap();
        let c = slots.allocate(fallback).unwrap();
        assert_eq!(a.index(), c.index());
        assert_ne!(a.generation(), c.generation());
        assert_eq!(slots.dirty_changes().collect::<Vec<_>>(), [(1, fallback), (2, texture)]);
        assert!(slots.contains(b));
        assert!(slots.allocate(texture).is_err());
    }
}
