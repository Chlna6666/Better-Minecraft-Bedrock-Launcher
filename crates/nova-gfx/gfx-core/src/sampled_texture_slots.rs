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
    current_snapshot: Option<BindGroupId>,
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
            current_snapshot: None,
        })
    }

    /// Index zero is always available for materials lacking a texture.
    #[must_use]
    pub const fn fallback_index(&self) -> u32 { 0 }

    /// Current shader-visible descriptor capacity, including fallback index zero.
    #[must_use]
    pub fn capacity(&self) -> u32 { self.views.len() as u32 }

    /// A shrink must not truncate live or GPU-retired indices. Retain the
    /// generation tombstones above the new logical capacity so growing
    /// again cannot resurrect a stale CPU slot handle.
    #[must_use]
    pub fn can_resize(&self, capacity: u32) -> bool {
        capacity != 0 && (capacity >= self.capacity() ||
            self.slots[capacity as usize..self.views.len()].iter().all(|slot|
                matches!(slot.state, State::Free | State::Exhausted)))
    }

    /// Atomically migrate to a compatible new shader/table capacity.
    ///
    /// Native publication happens before any allocator changes. Existing
    /// live indices, pending GPU-reader proofs and dirty values survive.
    /// The caller owns the returned old snapshot and must retire it after
    /// every real reader, and switch to a matching pipeline layout.
    pub fn resize_snapshot_with(
        &mut self, capacity: u32,
        create: impl FnOnce(&[TextureViewId]) -> Result<BindGroupId>,
    ) -> Result<(Option<BindGroupId>, BindGroupId)> {
        if !self.can_resize(capacity) {
            return Err(Error::InvalidInput("sampled table resize would truncate a live or retired slot".into()));
        }
        let previous_capacity = self.capacity();
        let mut views = self.views.clone();
        views.resize(capacity as usize, self.fallback);
        let snapshot = create(&views)?;
        let previous = self.current_snapshot.replace(snapshot);
        if capacity > previous_capacity {
            if self.slots.len() < capacity as usize {
                self.slots.resize_with(capacity as usize, || Slot { generation: 1, state: State::Free });
            }
            // Prefer the already-free lower indices before growing upward.
            let mut added: Vec<_> = (previous_capacity..capacity).rev().filter(|&index|
                matches!(self.slots[index as usize].state, State::Free)).collect();
            added.append(&mut self.free);
            self.free = added;
        } else if capacity < previous_capacity {
            self.free.retain(|&index| index < capacity);
        }
        self.views = views;
        self.dirty.clear();
        Ok((previous, snapshot))
    }

    /// Number of immediately reusable indices. Retired slots whose GPU
    /// readers have not completed are deliberately excluded.
    #[must_use]
    pub fn available_slots(&self) -> usize { self.free.len() }

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
    pub fn create_snapshot<D: ResourceDevice + ?Sized>(&mut self, device: &mut D, layout: BindGroupLayoutId) -> Result<BindGroupId> {
        self.create_snapshot_with(|views| device.create_sampled_texture_table(layout, views))
    }

    /// The same transactional publication through a restricted resource
    /// facade (e.g. GPUI ExtensionDevice), without exposing native queues.
    pub fn create_snapshot_with(&mut self, create: impl FnOnce(&[TextureViewId]) -> Result<BindGroupId>) -> Result<BindGroupId> {
        if self.current_snapshot.is_some() {
            return Err(Error::InvalidInput("sampled slot table already has a published snapshot".into()));
        }
        let snapshot = create(&self.views)?;
        self.current_snapshot = Some(snapshot);
        self.dirty.clear();
        Ok(snapshot)
    }

    /// Latest snapshot identity; old snapshots remain independently owned
    /// by the caller until their GPU readers have retired.
    #[must_use]
    pub const fn current_snapshot(&self) -> Option<BindGroupId> {
        self.current_snapshot
    }

    /// Publish changes over the caller's current snapshot. None means no
    /// native allocation was needed. On failure the dirty journal is intact.
    /// The caller must retire the old snapshot separately after its last use.
    pub fn publish<D: ResourceDevice + ?Sized>(&mut self, device: &mut D, current: BindGroupId) -> Result<Option<BindGroupId>> {
        self.publish_with(current, |source, changes| device.fork_sampled_texture_table(source, changes))
    }

    /// Publish through a restricted resource facade. Failure preserves the
    /// current native snapshot and pending dirty journal for retry.
    pub fn publish_with(&mut self, current: BindGroupId, fork: impl FnOnce(BindGroupId, &[(u32, TextureViewId)]) -> Result<BindGroupId>) -> Result<Option<BindGroupId>> {
        // A different or stale base can silently resurrect unchanged slots.
        // Reject it even if there are no pending dirty writes.
        if self.current_snapshot != Some(current) {
            return Err(Error::InvalidInput("sampled texture snapshot base is not current".into()));
        }
        if self.dirty.is_empty() { return Ok(None); }
        let changes: Vec<_> = self.dirty.iter().map(|(&index, &view)| (index, view)).collect();
        let snapshot = fork(current, &changes)?;
        self.current_snapshot = Some(snapshot);
        self.dirty.clear();
        Ok(Some(snapshot))
    }

    /// Sorted pending changes, exposed for diagnostics and tests.
    pub fn dirty_changes(&self) -> impl Iterator<Item = (u32, TextureViewId)> + '_ {
        self.dirty.iter().map(|(&index, &view)| (index, view))
    }
}

/// A complete material binding to one descriptor bank. All four indices
/// address that bank; shaders never have to index across descriptor sets.
/// The lease is intentionally not Clone. Release it through its allocator.
pub struct SampledMaterialLease {
    owner: std::sync::Arc<()>,
    bank: usize,
    views: [Option<TextureViewId>; 4],
    indices: [u32; 4],
}

impl SampledMaterialLease {
    /// Bank whose descriptor snapshot must be bound for this material.
    pub fn bank(&self) -> usize { self.bank }

    /// Albedo, normal, occlusion and metallic/roughness shader indices.
    pub fn indices(&self) -> [u32; 4] { self.indices }
}

struct BankImage {
    slot: SampledTextureSlot,
    references: usize,
}

struct TextureBank {
    slots: SampledTextureSlots,
    images: HashMap<TextureViewId, BankImage>,
}

/// Material-aware bank allocator. A texture can appear in multiple banks
/// when necessary, but its native image remains caller-owned. Each bank
/// uses the same shader capacity and retains independent snapshot/slot state.
/// Bank identities stay stable for the allocator's lifetime.
pub struct SampledTextureBanks {
    owner: std::sync::Arc<()>,
    capacity: u32,
    fallback: TextureViewId,
    max_banks: usize,
    banks: Vec<TextureBank>,
}

impl SampledTextureBanks {
    /// The per-bank capacity includes fallback slot zero. A finite bank
    /// budget bounds descriptor memory; callers can fall back on exhaustion.
    pub fn new(capacity: u32, fallback: TextureViewId, max_banks: usize) -> Result<Self> {
        if capacity == 0 || max_banks == 0 {
            return Err(Error::InvalidInput("sampled bank capacity and budget must be nonzero".into()));
        }
        Ok(Self { owner: std::sync::Arc::new(()), capacity, fallback, max_banks, banks: Vec::new() })
    }

    /// Number of allocated bank identities.
    pub fn bank_count(&self) -> usize { self.banks.len() }

    /// Read-only access to one bank's current immutable snapshot and journal.
    pub fn slots(&self, bank: usize) -> Option<&SampledTextureSlots> {
        self.banks.get(bank).map(|bank| &bank.slots)
    }

    /// Place all material images together, preferring the bank requiring
    /// the fewest new descriptors. Existing material bindings never move.
    /// Duplicate roles in one material count as a single image reference.
    pub fn acquire_material(&mut self, views: [Option<TextureViewId>; 4]) -> Result<SampledMaterialLease> {
        let views = views.map(|view| view.filter(|&view| view != self.fallback));
        let unique = unique_material_views(&views);
        if unique.len() > (self.capacity - 1) as usize {
            return Err(Error::Unavailable("material exceeds sampled bank capacity".into()));
        }
        let selected = self.banks.iter().enumerate().filter_map(|(index, bank)| {
            let missing = unique.iter().filter(|view| !bank.images.contains_key(*view)).count();
            (missing <= bank.slots.available_slots()).then_some((missing, index))
        }).min().map(|(_, index)| index);
        let index = if let Some(index) = selected {
            index
        } else {
            if self.banks.len() == self.max_banks {
                return Err(Error::Unavailable("sampled descriptor bank budget exhausted".into()));
            }
            let slots = SampledTextureSlots::new(self.capacity, self.fallback)?;
            let index = self.banks.len();
            self.banks.push(TextureBank { slots, images: HashMap::new() });
            index
        };
        let bank = &mut self.banks[index];
        if unique.iter().any(|view| bank.images.get(view).is_some_and(|entry| entry.references == usize::MAX)) {
            return Err(Error::Unavailable("sampled material reference count exhausted".into()));
        }
        for view in unique {
            if let Some(entry) = bank.images.get_mut(&view) {
                entry.references += 1;
            } else {
                // Placement checked the exact number of free slots first.
                let slot = bank.slots.allocate(view).expect("preflighted sampled bank capacity");
                bank.images.insert(view, BankImage { slot, references: 1 });
            }
        }
        let indices = views.map(|view| view.map_or(0, |view| bank.images[&view].slot.index()));
        Ok(SampledMaterialLease { owner: self.owner.clone(), bank: index, views, indices })
    }

    /// Release one material. The supplied readers must include ALL still
    /// outstanding native readers of this bank's slot generations, including
    /// earlier released materials. Unaccepted/untracked readers must be
    /// sealed by the caller before release. This is not a GPU wait.
    pub fn release_material(&mut self, lease: SampledMaterialLease, readers: &[SubmissionId]) -> Result<()> {
        if !std::sync::Arc::ptr_eq(&self.owner, &lease.owner) {
            return Err(Error::InvalidInput("sampled material lease belongs to another bank allocator".into()));
        }
        let bank = &mut self.banks[lease.bank];
        let unique = unique_material_views(&lease.views);
        for view in &unique {
            let entry = bank.images.get(view).ok_or_else(|| Error::InvalidInput("missing sampled material image".into()))?;
            if entry.references == 0 || !bank.slots.contains(entry.slot) {
                return Err(Error::InvalidInput("stale sampled material image".into()));
            }
        }
        for view in unique {
            let entry = bank.images.get_mut(&view).expect("validated material image");
            entry.references -= 1;
            if entry.references == 0 {
                let slot = entry.slot;
                bank.slots.retire(slot, readers)?;
                bank.images.remove(&view);
            }
        }
        Ok(())
    }

    /// Create/fork one bank snapshot transactionally. Old native snapshots
    /// remain caller-owned until their actual GPU readers have completed.
    /// Returns the replaced snapshot (if any) and the current snapshot.
    pub fn publish_bank_with(
        &mut self, index: usize,
        create: impl FnOnce(&[TextureViewId]) -> Result<BindGroupId>,
        fork: impl FnOnce(BindGroupId, &[(u32, TextureViewId)]) -> Result<BindGroupId>,
    ) -> Result<(Option<BindGroupId>, BindGroupId)> {
        let bank = self.banks.get_mut(index).ok_or_else(|| Error::InvalidInput("unknown sampled bank".into()))?;
        if let Some(old) = bank.slots.current_snapshot() {
            match bank.slots.publish_with(old, fork)? {
                Some(next) => Ok((Some(old), next)),
                None => Ok((None, old)),
            }
        } else {
            bank.slots.create_snapshot_with(create).map(|next| (None, next))
        }
    }

    /// Recycle only slots protected by the completed native submission.
    pub fn complete(&mut self, submission: SubmissionId) {
        for bank in &mut self.banks { bank.slots.complete(submission); }
    }
}

fn unique_material_views(views: &[Option<TextureViewId>; 4]) -> Vec<TextureViewId> {
    let mut unique = Vec::with_capacity(4);
    for view in views.iter().flatten() {
        if !unique.contains(view) { unique.push(*view); }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_banks_keep_roles_together_and_reuse_only_completed_slots() {
        let fallback = TextureViewId::from_parts(1, 1);
        let a = TextureViewId::from_parts(2, 1);
        let b = TextureViewId::from_parts(3, 1);
        let c = TextureViewId::from_parts(4, 1);
        let d = TextureViewId::from_parts(5, 1);
        let reader = SubmissionId::from_parts(9, 1);
        let mut banks = SampledTextureBanks::new(3, fallback, 2).unwrap();
        let first = banks.acquire_material([Some(a), Some(a), Some(b), None]).unwrap();
        assert_eq!(first.bank(), 0);
        assert_eq!(first.indices(), [1, 1, 2, 0]);
        let shared = banks.acquire_material([Some(a), Some(fallback), None, None]).unwrap();
        assert_eq!(shared.bank(), 0);
        let overflow = banks.acquire_material([Some(a), Some(c), None, None]).unwrap();
        assert_eq!(overflow.bank(), 1);
        assert_eq!(banks.bank_count(), 2);
        assert!(banks.acquire_material([Some(d), None, None, None]).is_err());
        banks.release_material(first, &[reader]).unwrap();
        assert_eq!(banks.slots(0).unwrap().available_slots(), 0);
        banks.complete(reader);
        let next = banks.acquire_material([Some(a), Some(d), None, None]).unwrap();
        assert_eq!(next.bank(), 0);
        assert_eq!(next.indices(), [1, 2, 0, 0]);
        banks.release_material(shared, &[]).unwrap();
        assert_eq!(banks.slots(0).unwrap().available_slots(), 0);
        banks.release_material(next, &[]).unwrap();
        assert_eq!(banks.slots(0).unwrap().available_slots(), 2);
        assert_eq!(banks.slots(1).unwrap().available_slots(), 0);
        banks.release_material(overflow, &[]).unwrap();
    }

    #[test]
    fn bank_snapshot_failure_preserves_journal_and_other_bank_identity() {
        let fallback = TextureViewId::from_parts(1, 1);
        let a = TextureViewId::from_parts(2, 1);
        let b = TextureViewId::from_parts(3, 1);
        let first = BindGroupId::from_parts(1, 1);
        let second = BindGroupId::from_parts(2, 1);
        let mut banks = SampledTextureBanks::new(2, fallback, 2).unwrap();
        let _a = banks.acquire_material([Some(a), None, None, None]).unwrap();
        let _b = banks.acquire_material([Some(b), None, None, None]).unwrap();
        assert!(banks.publish_bank_with(0, |_| Err(Error::Unavailable("test".into())), |_, _| unreachable!()).is_err());
        assert_eq!(banks.slots(0).unwrap().dirty_changes().collect::<Vec<_>>(), [(1, a)]);
        assert_eq!(banks.publish_bank_with(0, |_| Ok(first), |_, _| unreachable!()).unwrap(), (None, first));
        assert_eq!(banks.publish_bank_with(1, |_| Ok(second), |_, _| unreachable!()).unwrap(), (None, second));
        assert_eq!(banks.slots(0).unwrap().current_snapshot(), Some(first));
        assert_eq!(banks.slots(1).unwrap().current_snapshot(), Some(second));
    }

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
    fn failed_snapshot_publication_keeps_pending_edits_and_current_identity() {
        let fallback = TextureViewId::from_parts(1, 1);
        let texture = TextureViewId::from_parts(2, 1);
        let old = BindGroupId::from_parts(1, 1);
        let next = BindGroupId::from_parts(2, 1);
        let mut slots = SampledTextureSlots::new(2, fallback).unwrap();
        slots.allocate(texture).unwrap();
        assert!(slots.create_snapshot_with(|_| Err(Error::Unavailable("test allocation failure".into()))).is_err());
        assert_eq!(slots.current_snapshot(), None);
        assert_eq!(slots.dirty_changes().collect::<Vec<_>>(), [(1, texture)]);
        assert_eq!(slots.create_snapshot_with(|views| {
            assert_eq!(views, [fallback, texture]);
            Ok(old)
        }).unwrap(), old);
        let handle = SampledTextureSlot { index: 1, generation: 1 };
        slots.retire(handle, &[]).unwrap();
        assert!(slots.publish_with(old, |_, _| Err(Error::Unavailable("test fork failure".into()))).is_err());
        assert_eq!(slots.current_snapshot(), Some(old));
        assert_eq!(slots.dirty_changes().collect::<Vec<_>>(), [(1, fallback)]);
        assert_eq!(slots.publish_with(old, |source, changes| {
            assert_eq!(source, old);
            assert_eq!(changes, [(1, fallback)]);
            Ok(next)
        }).unwrap(), Some(next));
        assert_eq!(slots.current_snapshot(), Some(next));
        assert!(slots.publish_with(old, |_, _| panic!("stale base must not reach backend")).is_err());
        assert!(slots.publish_with(next, |_, _| panic!("clean table must not allocate")).unwrap().is_none());
    }

    #[test]
    fn capacity_migration_preserves_live_slots_readers_and_failed_publication() {
        let fallback = TextureViewId::from_parts(1, 1);
        let image = TextureViewId::from_parts(2, 1);
        let first = BindGroupId::from_parts(1, 1);
        let second = BindGroupId::from_parts(2, 1);
        let third = BindGroupId::from_parts(3, 1);
        let reader = SubmissionId::from_parts(4, 1);
        let mut slots = SampledTextureSlots::new(2, fallback).unwrap();
        let low = slots.allocate(image).unwrap();
        slots.create_snapshot_with(|_| Ok(first)).unwrap();
        assert!(slots.resize_snapshot_with(4, |_| Err(Error::Unavailable("test failure".into()))).is_err());
        assert_eq!(slots.capacity(), 2);
        assert_eq!(slots.current_snapshot(), Some(first));
        assert!(slots.contains(low));
        assert_eq!(slots.resize_snapshot_with(4, |views| {
            assert_eq!(views, [fallback, image, fallback, fallback]);
            Ok(second)
        }).unwrap(), (Some(first), second));
        let high = slots.allocate(image).unwrap();
        assert_eq!(high.index(), 2);
        assert!(!slots.can_resize(2));
        slots.retire(high, &[reader]).unwrap();
        assert!(!slots.can_resize(2));
        slots.complete(reader);
        assert!(slots.can_resize(2));
        assert_eq!(slots.resize_snapshot_with(2, |views| {
            assert_eq!(views, [fallback, image]);
            Ok(third)
        }).unwrap(), (Some(second), third));
        assert!(slots.contains(low));
        slots.resize_snapshot_with(4, |_| Ok(first)).unwrap();
        let reused = slots.allocate(image).unwrap();
        assert_eq!(reused.index(), high.index());
        assert_ne!(reused.generation(), high.generation());
        assert!(!slots.contains(high));
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
