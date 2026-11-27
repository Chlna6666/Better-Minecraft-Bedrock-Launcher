//! Shared dependency lists from longbridge/gpui-fast, fast/dependencies.rs.
//! Upstream revision: 1100a214532bdeac0282acd1c5c6d2062d8ab1a7.
//! Copyright 2022-2025 Zed Industries, Inc. Copyright 2026 Longbridge.
//! SPDX-License-Identifier: Apache-2.0

use crate::EntityId;
use collections::FxHashSet;
use smallvec::SmallVec;
#[cfg(test)]
use collections::FxHashMap;
use std::{any::TypeId, cell::{Cell, RefCell}, ops::Range, rc::{Rc, Weak}};

/// Lists of what retained subtrees read, each kept once and shared by every
/// subtree that read the same: a view built again reads what it read last
/// frame, a recording's own reads are often all of its reads, and many rows
/// read the same few entities. A recording finished with a list already held
/// takes it without allocating.
///
/// Lists nothing else holds any more are dropped once the set has doubled
/// since it was last swept, so it stays about as large as what is in use.
pub(crate) struct Interned<T> {
    lists: FxHashSet<Rc<[T]>>,
    sweep_at: usize,
    empty: Rc<[T]>,
}

impl<T> Default for Interned<T> {
    fn default() -> Self {
        Self {
            lists: FxHashSet::default(),
            sweep_at: 1024,
            empty: Rc::from(Vec::new()),
        }
    }
}

impl<T: Eq + std::hash::Hash + Copy> Interned<T> {
    /// The shared list holding `items`, which are sorted without repeats.
    pub(crate) fn get(&mut self, items: &[T]) -> Rc<[T]> {
        if items.is_empty() {
            return self.empty.clone();
        }
        if let Some(list) = self.lists.get(items) {
            return list.clone();
        }
        if self.lists.len() >= self.sweep_at {
            self.lists.retain(|list| Rc::strong_count(list) > 1);
            self.sweep_at = (self.lists.len() * 2).max(1024);
        }
        let list: Rc<[T]> = Rc::from(items);
        self.lists.insert(list.clone());
        list
    }
}

/// Sorts `items` and removes repeats.
pub(crate) fn sort_unique<T: Ord>(items: &mut Vec<T>) {
    // Sorted runs (including equal adjacent values) need no sort.
    if !items.windows(2).all(|pair| pair[0] <= pair[1]) {
        items.sort_unstable();
    }
    items.dedup();
}

// A single contiguous range frequently contains zero or one read, or is
// already ordered. Intern it directly rather than copying into scratch
// and sorting an unchanged sequence.
fn intern_range<T: Ord + Copy + std::hash::Hash>(
    interner: &mut Interned<T>,
    scratch: &mut Vec<T>,
    items: &[T],
) -> Rc<[T]> {
    if items.windows(2).all(|pair| pair[0] < pair[1]) {
        return interner.get(items);
    }
    scratch.clear();
    scratch.extend_from_slice(items);
    sort_unique(scratch);
    interner.get(scratch)
}

// Ignore empty ranges: an owner may have many recording boundaries but
// only one nonempty range for a particular dependency kind.
fn intern_ranges<T: Ord + Copy + std::hash::Hash>(
    interner: &mut Interned<T>,
    scratch: &mut Vec<T>,
    log: &[T],
    ranges: impl Iterator<Item = Range<usize>>,
) -> Rc<[T]> {
    let mut nonempty = ranges.filter(|range| !range.is_empty());
    let Some(first) = nonempty.next() else {
        return interner.get(&[]);
    };
    let Some(second) = nonempty.next() else {
        return intern_range(interner, scratch, &log[first]);
    };
    scratch.clear();
    scratch.extend_from_slice(&log[first]);
    scratch.extend_from_slice(&log[second]);
    for range in nonempty {
        scratch.extend_from_slice(&log[range]);
    }
    sort_unique(scratch);
    interner.get(scratch)
}

/// The union of two sorted lists without repeats, itself sorted and without
/// repeats. When one holds all of the other, which is the usual case — a
/// view's paint reads what its prepaint read — it is shared, not copied.
pub(crate) fn merge_sorted<T: Ord + Copy>(a: &Rc<[T]>, b: &Rc<[T]>) -> Rc<[T]> {
    fn contains_all<T: Ord>(all: &[T], some: &[T]) -> bool {
        if some.is_empty() {
            return true;
        }
        if all.len() < some.len()
            || all.first() > some.first()
            || all.last() < some.last()
        {
            return false;
        }
        if all.len() == some.len() {
            return all == some;
        }
        let mut rest = all;
        if all.len() / some.len() >= 8 {
            // Sparse probes should not walk the intervening large gaps.
            return some.iter().all(|item| match rest.binary_search(item) {
                Ok(index) => {
                    rest = &rest[index + 1..];
                    true
                }
                Err(_) => false,
            });
        }
        // Similarly sized sets use one sequential pass instead of a binary
        // search for every item. Each element is visited at most once.
        for item in some {
            while let Some((head, tail)) = rest.split_first() {
                match head.cmp(item) {
                    std::cmp::Ordering::Less => rest = tail,
                    std::cmp::Ordering::Equal => {
                        rest = tail;
                        break;
                    }
                    std::cmp::Ordering::Greater => return false,
                }
            }
            // A match at the last element is valid only for the last probe.
            if rest.is_empty() {
                return some.last() == Some(item);
            }
        }
        true
    }
    if b.is_empty() || Rc::ptr_eq(a, b) {
        return a.clone();
    }
    if a.is_empty() {
        return b.clone();
    }
    // Only the larger set can contain the smaller one. Avoid two subset
    // probes and retain the left allocation for equal sets.
    if a.len() >= b.len() {
        if contains_all(a, b) {
            return a.clone();
        }
    } else if contains_all(b, a) {
        return b.clone();
    }
    let mut merged = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => {
                merged.push(a[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                merged.push(b[j]);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                merged.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    merged.extend_from_slice(&a[i..]);
    merged.extend_from_slice(&b[j..]);
    merged.into()
}


// The upstream recording model uses contiguous logs with range boundaries.
// Until RetainedSubtree owns dependencies, publish the own-read ranges through
// the existing window notification routes. No owner is stored per read.
#[derive(Default)]
pub(crate) struct AppDependencies {
    entity_log: Vec<EntityId>,
    global_log: Vec<(TypeId, Option<bool>)>,
    state_read_log: Rc<RefCell<Vec<StateRead>>>,
    recording: DependencyRecording,
    ranges: Vec<LogRanges>,
    scratch_entities: Vec<EntityId>,
    scratch_globals: Vec<(TypeId, Option<bool>)>,
    interned_entities: Interned<EntityId>,
    interned_globals: Interned<(TypeId, Option<bool>)>,
    no_states: Option<Rc<[StateRead]>>,
}

#[derive(Default)]
struct DependencyRecording {
    owner: Option<EntityId>,
    entities: usize,
    globals: usize,
    states: usize,
}

struct LogRanges {
    owner: Option<EntityId>,
    entities: Range<usize>,
    globals: Range<usize>,
    states: Range<usize>,
}

/// The entity and global portion of Fast's RenderDependencies. Global presence
/// is still represented by the existing notification API's observed boolean.
#[derive(Clone, Default)]
pub(crate) struct RenderDependencies {
    pub(crate) entities: Rc<[EntityId]>,
    pub(crate) globals: Rc<[(TypeId, Option<bool>)]>,
    pub(crate) states: Rc<[StateRead]>,
}

impl RenderDependencies {
    /// Union the reads of a rebuilt fragment with a replayed sibling of the
    /// same owner. Keep the original first-read ordering of the new log.
    pub(crate) fn merge_replayed(&mut self, previous: &Self) {
        self.entities = merge_sorted(&self.entities, &previous.entities);
        self.globals = merge_sorted(&self.globals, &previous.globals);
        self.states = merge_state_reads(&self.states, &previous.states);
    }

    pub(crate) fn same_routes(&self, other: &Self) -> bool {
        (Rc::ptr_eq(&self.entities, &other.entities) || self.entities == other.entities)
            && (Rc::ptr_eq(&self.globals, &other.globals) || self.globals == other.globals)
    }
}

impl AppDependencies {
    #[inline]
    pub(crate) fn note_access(&mut self, entity_id: EntityId) {
        // From Fast note_access: an equal adjacent read may be elided only
        // within the same recording boundary.
        if self.entity_log.len() > self.recording.entities
            && self.entity_log.last() == Some(&entity_id)
        {
            return;
        }
        self.entity_log.push(entity_id);
    }

    #[inline]
    pub(crate) fn note_global_read(&mut self, global: (TypeId, Option<bool>)) {
        if self.global_log.len() > self.recording.globals
            && self.global_log.last() == Some(&global)
        {
            return;
        }
        self.global_log.push(global);
    }

    fn close_recording(&mut self) {
        let entities = self.recording.entities..self.entity_log.len();
        let globals = self.recording.globals..self.global_log.len();
        let states = self.recording.states..self.state_read_log.borrow().len();
        if !entities.is_empty() || !globals.is_empty() || !states.is_empty() {
            self.recording.entities = entities.end;
            self.recording.globals = globals.end;
            self.recording.states = states.end;
            self.ranges.push(LogRanges {
                owner: self.recording.owner,
                entities,
                globals,
                states,
            });
        }
    }

    pub(crate) fn set_owner(&mut self, owner: Option<EntityId>) -> Option<EntityId> {
        let previous = self.recording.owner;
        if previous != owner {
            self.close_recording();
            self.recording.owner = owner;
        }
        previous
    }

    pub(crate) fn clear(&mut self) {
        self.entity_log.clear();
        self.global_log.clear();
        self.state_read_log.borrow_mut().clear();
        self.ranges.clear();
        self.recording = DependencyRecording::default();
    }

    /// Finish each owner's disjoint read ranges once. Publish the interned
    /// lists directly, without constructing temporary empty owner sets.
    pub(crate) fn finish(&mut self, mut publish: impl FnMut(Option<EntityId>, RenderDependencies)) {
        self.close_recording();
        self.ranges.sort_unstable_by_key(|range| (range.owner, range.states.start));
        let states_log = self.state_read_log.borrow();
        let no_states = self.no_states.get_or_insert_with(|| Rc::from([])).clone();
        for group in self.ranges.chunk_by(|a, b| a.owner == b.owner) {
            if let [range] = group {
                let states = &states_log[range.states.clone()];
                publish(range.owner, RenderDependencies {
                    entities: intern_range(
                        &mut self.interned_entities,
                        &mut self.scratch_entities,
                        &self.entity_log[range.entities.clone()],
                    ),
                    globals: intern_range(
                        &mut self.interned_globals,
                        &mut self.scratch_globals,
                        &self.global_log[range.globals.clone()],
                    ),
                    states: if states.is_empty() {
                        no_states.clone()
                    } else {
                        unique_states(states)
                    },
                });
                continue;
            }
            let mut state_ranges = group.iter().map(|range| &states_log[range.states.clone()])
                .filter(|reads| !reads.is_empty());
            let states = match (state_ranges.next(), state_ranges.next()) {
                (None, _) => no_states.clone(),
                (Some(first), None) => unique_states(first),
                (Some(first), Some(second)) => unique_state_iter(
                    first.iter().chain(second).chain(state_ranges.flatten()),
                ),
            };
            publish(group[0].owner, RenderDependencies {
                entities: intern_ranges(
                    &mut self.interned_entities,
                    &mut self.scratch_entities,
                    &self.entity_log,
                    group.iter().map(|range| range.entities.clone()),
                ),
                globals: intern_ranges(
                    &mut self.interned_globals,
                    &mut self.scratch_globals,
                    &self.global_log,
                    group.iter().map(|range| range.globals.clone()),
                ),
                states,
            });
        }
        drop(states_log);
        self.clear();
    }

    pub(crate) fn begin_state_reads(&self) -> StateReadRecording {
        StateReadRecording::enter(Some(self.state_read_log.clone()))
    }

    pub(crate) fn state_read_index(&self) -> usize {
        self.state_read_log.borrow().len()
    }

    pub(crate) fn state_reads_since(&mut self, start: usize) -> Rc<[StateRead]> {
        let reads = self.state_read_log.borrow();
        if start == reads.len() {
            self.no_states.get_or_insert_with(|| Rc::from([])).clone()
        } else {
            unique_states(&reads[start..])
        }
    }

    #[cfg(test)]
    pub(crate) fn entity(&mut self, owner: Option<EntityId>, entity: EntityId) {
        self.set_owner(owner);
        self.note_access(entity);
    }

    #[cfg(test)]
    pub(crate) fn global(&mut self, owner: Option<EntityId>, key: (TypeId, Option<bool>)) {
        self.set_owner(owner);
        self.note_global_read(key);
    }

    #[cfg(test)]
    pub(crate) fn into_sets(&mut self) -> FxHashMap<Option<EntityId>, RenderDependencies> {
        let mut sets = FxHashMap::default();
        self.finish(|owner, reads| { sets.insert(owner, reads); });
        sets
    }
}

// StateVersion and first-read deduplication from fast/dependencies.rs,
// adapted to keep weak state snapshots and window notification semantics.
thread_local! {
    static STATE_READ_LOG: RefCell<Option<Rc<RefCell<Vec<StateRead>>>>> = const { RefCell::new(None) };
    static STATE_GENERATION: Cell<u64> = const { Cell::new(0) };
}

pub(crate) fn state_generation() -> u64 {
    STATE_GENERATION.with(Cell::get)
}

#[derive(Clone, Default, Debug)]
pub(crate) struct StateVersion(Rc<Cell<u64>>);

impl StateVersion {
    pub(crate) fn get(&self) -> u64 {
        self.0.get()
    }

    pub(crate) fn bump(&self) {
        self.0.set(self.0.get().wrapping_add(1));
        STATE_GENERATION.with(|generation| generation.set(generation.get().wrapping_add(1)));
    }

    #[inline]
    pub(crate) fn bump_if(&self, changed: bool) {
        if changed {
            self.bump();
        }
    }

    pub(crate) fn read(&self) {
        STATE_READ_LOG.with_borrow(|log| {
            if let Some(log) = log {
                log.borrow_mut().push(StateRead::new(self));
            }
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StateReadStatus { Unchanged, Changed, Dead }

#[derive(Clone)]
pub(crate) struct StateRead {
    state: Weak<Cell<u64>>,
    at: u64,
}

impl PartialEq for StateRead {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at && self.state.ptr_eq(&other.state)
    }
}

impl StateRead {
    pub(crate) fn new(version: &StateVersion) -> Self {
        Self { state: Rc::downgrade(&version.0), at: version.get() }
    }

    fn id(&self) -> usize {
        self.state.as_ptr() as usize
    }

    pub(crate) fn belongs_to(&self, version: &StateVersion) -> bool {
        self.state.as_ptr() == Rc::as_ptr(&version.0)
    }

    pub(crate) fn status(&self) -> StateReadStatus {
        match self.state.upgrade() {
            Some(state) if state.get() == self.at => StateReadStatus::Unchanged,
            Some(_) => StateReadStatus::Changed,
            None => StateReadStatus::Dead,
        }
    }
}

/// Merge two first-read-unique snapshots in observation order. Keep the
/// earlier version even if the state changed between recording phases.
/// Common duplicate-only merges retain the original immutable allocation.
pub(crate) fn merge_state_reads(
    earlier: &Rc<[StateRead]>,
    later: &Rc<[StateRead]>,
) -> Rc<[StateRead]> {
    if later.is_empty() || Rc::ptr_eq(earlier, later) {
        return earlier.clone();
    }
    if earlier.is_empty() {
        return later.clone();
    }

    if earlier.len().saturating_add(later.len()) <= 16 {
        let mut new_reads = later.iter().filter(|read| {
            !earlier.iter().any(|old| old.id() == read.id())
        }).peekable();
        if new_reads.peek().is_none() {
            return earlier.clone();
        }
        return earlier.iter().chain(new_reads).cloned().collect();
    }

    let mut seen = FxHashSet::with_capacity_and_hasher(
        earlier.len().saturating_add(later.len()),
        Default::default(),
    );
    seen.extend(earlier.iter().map(StateRead::id));
    let mut new_reads = later.iter().filter(|read| seen.insert(read.id())).peekable();
    if new_reads.peek().is_none() {
        return earlier.clone();
    }
    earlier.iter().chain(new_reads).cloned().collect()
}

// Deduplicate borrowed reads across disjoint ranges before cloning any
// Weak pointer. The hash table grows with unique states, not raw log size.
fn unique_state_iter<'a>(states: impl IntoIterator<Item = &'a StateRead>) -> Rc<[StateRead]> {
    let mut unique: SmallVec<[&StateRead; 8]> = SmallVec::new();
    let mut seen: Option<FxHashSet<usize>> = None;
    for state in states {
        let id = state.id();
        if let Some(seen) = &mut seen {
            if !seen.insert(id) {
                continue;
            }
        } else {
            if unique.iter().any(|old| old.id() == id) {
                continue;
            }
            if unique.len() == 8 {
                let mut identities: FxHashSet<usize> =
                    unique.iter().map(|old| old.id()).collect();
                identities.insert(id);
                seen = Some(identities);
            }
        }
        unique.push(state);
    }
    unique.into_iter().cloned().collect()
}

/// The first read of each state, in order, as in Fast's unique_states.
fn unique_states(states: &[StateRead]) -> Rc<[StateRead]> {
    // A single scroll handle is common; avoid allocating a hash table for it.
    if states.len() <= 1 {
        return Rc::from(states);
    }
    // Repeated getters commonly record the same handle dozens of times.
    // Keep its earliest snapshot without allocating a hash table.
    if states.iter().skip(1).all(|state| state.id() == states[0].id()) {
        return Rc::from(&states[..1]);
    }
    if states.len() <= 8 {
        return states.iter().enumerate()
            .filter(|(index, state)| !states[..*index].iter().any(|old| old.id() == state.id()))
            .map(|(_, state)| state.clone())
            .collect();
    }
    unique_state_iter(states)
}

/// Shares the existing frame log with code that reads state without an App.
/// Restore the nested caller's log, including on unwind.
pub(crate) struct StateReadRecording(Option<Rc<RefCell<Vec<StateRead>>>>);

impl StateReadRecording {
    pub(crate) fn enter(log: Option<Rc<RefCell<Vec<StateRead>>>>) -> Self {
        Self(STATE_READ_LOG.with(|current| current.replace(log)))
    }
}

impl Drop for StateReadRecording {
    fn drop(&mut self) {
        STATE_READ_LOG.with(|current| { current.replace(self.0.take()); });
    }
}

#[cfg(test)]
mod state_dedup_tests {
    use super::*;

    #[test]
    fn sorted_union_matches_reference_and_shares_subsets() {
        for mask_a in 0..64_u32 {
            for mask_b in 0..64_u32 {
                let a: Rc<[u32]> = (0..6).filter(|i| mask_a & (1 << i) != 0).collect();
                let b: Rc<[u32]> = (0..6).filter(|i| mask_b & (1 << i) != 0).collect();
                let expected: Vec<_> = (0..6).filter(|i| (mask_a | mask_b) & (1 << i) != 0).collect();
                let union = merge_sorted(&a, &b);
                assert_eq!(&*union, expected.as_slice());
                if mask_b & !mask_a == 0 {
                    assert!(Rc::ptr_eq(&union, &a));
                } else if mask_a & !mask_b == 0 {
                    assert!(Rc::ptr_eq(&union, &b));
                }
            }
        }
        let dense: Rc<[u32]> = (0..1024).collect();
        let sparse: Rc<[u32]> = Rc::from([3, 511, 1023]);
        assert!(Rc::ptr_eq(&merge_sorted(&dense, &sparse), &dense));
        let missing: Rc<[u32]> = Rc::from([511, 1024]);
        assert_eq!(merge_sorted(&dense, &missing).len(), 1025);
    }

    #[test]
    fn disjoint_dependency_ranges_skip_empty_and_share_existing_lists() {
        let mut interner = Interned::default();
        let mut scratch = vec![99_u32];
        let log = [1, 3, 7, 2, 3];
        let one = intern_ranges(&mut interner, &mut scratch, &log, [0..0, 0..2, 2..2].into_iter());
        assert_eq!(&*one, &[1, 3]);
        assert_eq!(scratch, [99]);
        let union = intern_ranges(&mut interner, &mut scratch, &log, [0..2, 3..5].into_iter());
        assert_eq!(&*union, &[1, 2, 3]);
        let ordered = interner.get(&[1, 2, 3]);
        assert!(Rc::ptr_eq(&union, &ordered));
    }

    #[test]
    fn disjoint_state_reads_keep_earliest_versions_and_promote_by_unique_count() {
        let states: Vec<_> = (0..12).map(|_| StateVersion::default()).collect();
        let first: Vec<_> = states[..9].iter().map(StateRead::new).collect();
        states[0].bump();
        let second: Vec<_> = states.iter().map(StateRead::new).collect();
        let merged = unique_state_iter(first.iter().chain(second.iter()).chain(second.iter()));
        assert_eq!(merged.len(), states.len());
        assert_eq!(merged[0].status(), StateReadStatus::Changed);
        assert!(merged.iter().zip(&states).all(|(read, state)| read.belongs_to(state)));
    }

    #[test]
    fn contiguous_dependency_ranges_intern_without_scratch_copy() {
        let mut interner = Interned::default();
        let mut scratch = vec![99_u32];
        let ordered = intern_range(&mut interner, &mut scratch, &[1, 2, 3]);
        assert_eq!(scratch, [99]);
        let repeated = intern_range(&mut interner, &mut scratch, &[3, 1, 3, 2]);
        assert_eq!(&*repeated, &[1, 2, 3]);
        assert!(Rc::ptr_eq(&ordered, &repeated));
        let empty = intern_range(&mut interner, &mut scratch, &[]);
        assert!(empty.is_empty());
    }

    #[test]
    fn single_owner_range_publishes_first_state_version() {
        let mut dependencies = AppDependencies::default();
        let state = StateVersion::default();
        let scope = dependencies.begin_state_reads();
        let owner = EntityId::from(1_u64);
        dependencies.entity(Some(owner), EntityId::from(3_u64));
        dependencies.entity(Some(owner), EntityId::from(2_u64));
        dependencies.entity(Some(owner), EntityId::from(3_u64));
        state.read();
        state.bump();
        state.read();
        drop(scope);
        let sets = dependencies.into_sets();
        let result = &sets[&Some(owner)];
        assert_eq!(&*result.entities, &[EntityId::from(2_u64), EntityId::from(3_u64)]);
        assert_eq!(result.states.len(), 1);
        assert_eq!(result.states[0].status(), StateReadStatus::Changed);
    }

    #[test]
    fn merge_keeps_first_snapshot_and_shares_duplicate_only_reads() {
        let first = StateVersion::default();
        let second = StateVersion::default();
        let earlier = unique_states(&[StateRead::new(&first)]);
        first.bump();
        let duplicate = unique_states(&[StateRead::new(&first)]);
        let merged = merge_state_reads(&earlier, &duplicate);
        assert!(Rc::ptr_eq(&merged, &earlier));
        assert_eq!(merged[0].status(), StateReadStatus::Changed);

        let later = unique_states(&[StateRead::new(&second), StateRead::new(&first)]);
        let merged = merge_state_reads(&earlier, &later);
        assert_eq!(merged.len(), 2);
        assert!(merged[0].belongs_to(&first));
        assert!(merged[1].belongs_to(&second));
        assert_eq!(merged[0].status(), StateReadStatus::Changed);
    }

    #[test]
    fn merge_large_unique_snapshots_preserves_order_and_identity() {
        let states: Vec<_> = (0..32).map(|_| StateVersion::default()).collect();
        let earlier = unique_states(&states[..24].iter().map(StateRead::new).collect::<Vec<_>>());
        let duplicate = unique_states(&states[8..24].iter().map(StateRead::new).collect::<Vec<_>>());
        assert!(Rc::ptr_eq(&merge_state_reads(&earlier, &duplicate), &earlier));
        let later = unique_states(&states[16..].iter().map(StateRead::new).collect::<Vec<_>>());
        let merged = merge_state_reads(&earlier, &later);
        assert_eq!(merged.len(), states.len());
        assert!(merged.iter().zip(&states).all(|(read, state)| read.belongs_to(state)));
    }

    #[test]
    fn dedup_preserves_earliest_snapshot_in_small_and_repeated_logs() {
        let first = StateVersion::default();
        let second = StateVersion::default();
        let a = StateRead::new(&first);
        first.bump();
        let b = StateRead::new(&first);
        let c = StateRead::new(&second);
        let repeated = std::iter::once(a.clone()).chain(std::iter::repeat_n(b.clone(), 64)).collect::<Vec<_>>();
        let reads = unique_states(&repeated);
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].status(), StateReadStatus::Changed);
        let reads = unique_states(&[a, c.clone(), b, c]);
        assert_eq!(reads.len(), 2);
        assert_eq!(reads[0].status(), StateReadStatus::Changed);
        assert_eq!(reads[1].status(), StateReadStatus::Unchanged);
    }
}
