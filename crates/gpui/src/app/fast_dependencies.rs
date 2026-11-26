//! Shared dependency lists from longbridge/gpui-fast, fast/dependencies.rs.
//! Upstream revision: 1100a214532bdeac0282acd1c5c6d2062d8ab1a7.
//! Copyright 2022-2025 Zed Industries, Inc. Copyright 2026 Longbridge.
//! SPDX-License-Identifier: Apache-2.0

use crate::EntityId;
use collections::FxHashSet;
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
    items.sort_unstable();
    items.dedup();
}

/// The union of two sorted lists without repeats, itself sorted and without
/// repeats. When one holds all of the other, which is the usual case — a
/// view's paint reads what its prepaint read — it is shared, not copied.
pub(crate) fn merge_sorted<T: Ord + Copy>(a: &Rc<[T]>, b: &Rc<[T]>) -> Rc<[T]> {
    fn contains_all<T: Ord>(all: &[T], some: &[T]) -> bool {
        let mut rest = all;
        some.iter().all(|item| match rest.binary_search(item) {
            Ok(index) => {
                rest = &rest[index + 1..];
                true
            }
            Err(_) => false,
        })
    }
    if b.is_empty() || Rc::ptr_eq(a, b) || contains_all(a, b) {
        return a.clone();
    }
    if a.is_empty() || contains_all(b, a) {
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
    scratch_states: Vec<StateRead>,
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
        if self.states.is_empty() {
            self.states = previous.states.clone();
        } else if !previous.states.is_empty() {
            let mut states = Vec::with_capacity(self.states.len() + previous.states.len());
            states.extend_from_slice(&self.states);
            states.extend_from_slice(&previous.states);
            self.states = unique_states(&states);
        }
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
            self.scratch_entities.clear();
            self.scratch_globals.clear();
            self.scratch_states.clear();
            for range in group {
                self.scratch_entities.extend_from_slice(&self.entity_log[range.entities.clone()]);
                self.scratch_globals.extend_from_slice(&self.global_log[range.globals.clone()]);
                self.scratch_states.extend_from_slice(&states_log[range.states.clone()]);
            }
            sort_unique(&mut self.scratch_entities);
            sort_unique(&mut self.scratch_globals);
            publish(group[0].owner, RenderDependencies {
                entities: self.interned_entities.get(&self.scratch_entities),
                globals: self.interned_globals.get(&self.scratch_globals),
                states: if self.scratch_states.is_empty() {
                    no_states.clone()
                } else {
                    unique_states(&self.scratch_states)
                },
            });
        }
        drop(states_log);
        self.scratch_states.clear();
        self.clear();
    }

    pub(crate) fn begin_state_reads(&self) -> StateReadRecording {
        StateReadRecording::enter(Some(self.state_read_log.clone()))
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

impl StateRead {
    pub(crate) fn new(version: &StateVersion) -> Self {
        Self { state: Rc::downgrade(&version.0), at: version.get() }
    }

    fn id(&self) -> usize {
        self.state.as_ptr() as usize
    }

    pub(crate) fn status(&self) -> StateReadStatus {
        match self.state.upgrade() {
            Some(state) if state.get() == self.at => StateReadStatus::Unchanged,
            Some(_) => StateReadStatus::Changed,
            None => StateReadStatus::Dead,
        }
    }
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
    let mut seen = FxHashSet::with_capacity_and_hasher(states.len(), Default::default());
    states.iter().filter(|state| seen.insert(state.id())).cloned().collect()
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
