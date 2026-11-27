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
    let third = nonempty.next();
    let left = &log[first];
    let right = &log[second];
    if third.is_none()
        && left.windows(2).all(|pair| pair[0] <= pair[1])
        && right.windows(2).all(|pair| pair[0] <= pair[1])
    {
        // Two ordered runs are common across render phases. Merge the
        // borrowed slices directly into the existing scratch allocation.
        merge_ordered_runs(scratch, left, right);
    } else {
        scratch.clear();
        scratch.extend_from_slice(left);
        scratch.extend_from_slice(right);
        for range in third.into_iter().chain(nonempty) {
            scratch.extend_from_slice(&log[range]);
        }
        sort_unique(scratch);
    }
    interner.get(scratch)
}

fn merge_ordered_runs<T: Ord + Copy>(out: &mut Vec<T>, mut left: &[T], mut right: &[T]) {
    out.clear();
    while let (Some(&a), Some(&b)) = (left.first(), right.first()) {
        let next = if a <= b {
            left = &left[1..];
            a
        } else {
            right = &right[1..];
            b
        };
        if out.last() != Some(&next) {
            out.push(next);
        }
    }
    for &next in left.iter().chain(right) {
        if out.last() != Some(&next) {
            out.push(next);
        }
    }
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StateDomain {
    #[default]
    Content,
    Placement,
}

#[derive(Clone, Copy, Default)]
struct StateEpochs {
    content: u64,
    placement: u64,
}

impl StateEpochs {
    fn all(self) -> u64 {
        // Every mutation advances exactly one domain. Keep the existing
        // combined generation contract for window-wide invalidation.
        self.content.wrapping_add(self.placement)
    }

    fn advance(&mut self, domain: StateDomain) {
        let epoch = match domain {
            StateDomain::Content => &mut self.content,
            StateDomain::Placement => &mut self.placement,
        };
        *epoch = epoch.wrapping_add(1);
    }
}

// Bounded UI-thread journal of observed state identities. No owning
// references, per-state subscriber lists, or unbounded mutation history.
const STATE_MUTATION_CAPACITY: usize = 256;

struct StateMutations {
    sequence: u64,
    identities: [usize; STATE_MUTATION_CAPACITY],
}

#[derive(Clone, Copy)]
pub(crate) struct StateMutationCursor(u64);

impl StateMutationCursor {
    pub(crate) fn current() -> Self {
        STATE_MUTATIONS.with(|log| Self(log.borrow().sequence))
    }

    /// Visit only changes since a certified snapshot. A lagging consumer
    /// or a dense update must fall back rather than omit overwritten events.
    pub(crate) fn visit_since(
        self,
        budget: usize,
        mut visit: impl FnMut(usize),
    ) -> Option<Self> {
        STATE_MUTATIONS.with(|log| {
            let log = log.borrow();
            let count = log.sequence.wrapping_sub(self.0);
            if count > STATE_MUTATION_CAPACITY.min(budget) as u64 {
                return None;
            }
            for offset in 0..count {
                let sequence = self.0.wrapping_add(offset);
                visit(log.identities[sequence as usize % STATE_MUTATION_CAPACITY]);
            }
            Some(Self(log.sequence))
        })
    }
}

fn record_state_mutation(identity: usize) {
    let _ = STATE_MUTATIONS.try_with(|log| {
        let mut log = log.borrow_mut();
        let index = log.sequence as usize % STATE_MUTATION_CAPACITY;
        log.identities[index] = identity;
        log.sequence = log.sequence.wrapping_add(1);
    });
}

thread_local! {
    static STATE_MUTATIONS: RefCell<StateMutations> = const {
        RefCell::new(StateMutations {
            sequence: 0,
            identities: [0; STATE_MUTATION_CAPACITY],
        })
    };
    static STATE_READ_LOG: RefCell<Option<Rc<RefCell<Vec<StateRead>>>>> = const { RefCell::new(None) };
    static STATE_EPOCHS: Cell<StateEpochs> = const { Cell::new(StateEpochs { content: 0, placement: 0 }) };
}

pub(crate) fn state_generation() -> u64 {
    STATE_EPOCHS.with(|epochs| epochs.get().all())
}

fn advance_state_domain(domain: StateDomain) {
    // Destruction may run during thread-local teardown.
    let _ = STATE_EPOCHS.try_with(|epochs| {
        let mut next = epochs.get();
        next.advance(domain);
        epochs.set(next);
    });
}

#[derive(Default, Debug)]
struct StateVersionInner {
    value: Cell<u64>,
    domain: StateDomain,
}

#[derive(Clone, Default, Debug)]
pub(crate) struct StateVersion(Rc<StateVersionInner>);

impl StateVersion {
    /// A scroll-sensitive source. Explicit reads remain dependencies even
    /// when the caller uses the offset to generate content.
    pub(crate) fn placement() -> Self {
        Self(Rc::new(StateVersionInner {
            value: Cell::new(0),
            domain: StateDomain::Placement,
        }))
    }

    pub(crate) fn get(&self) -> u64 {
        self.0.value.get()
    }

    pub(crate) fn bump(&self) {
        self.0.value.set(self.get().wrapping_add(1));
        advance_state_domain(self.0.domain);
        if Rc::weak_count(&self.0) != 0 {
            record_state_mutation(Rc::as_ptr(&self.0) as usize);
        }
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

// A weak snapshot becomes invalid on the final owner's drop even without a
// version bump. Publish that transition to the same UI-thread generation.
impl Drop for StateVersion {
    fn drop(&mut self) {
        if Rc::strong_count(&self.0) == 1 && Rc::weak_count(&self.0) != 0 {
            advance_state_domain(self.0.domain);
            record_state_mutation(Rc::as_ptr(&self.0) as usize);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ValidatedEpoch {
    #[default]
    Unchecked,
    Empty,
    Content(u64),
    Placement(u64),
    Mixed(u64),
}

impl ValidatedEpoch {
    fn matches(self, epochs: StateEpochs) -> bool {
        match self {
            Self::Unchecked => false,
            Self::Empty => true,
            Self::Content(epoch) => epoch == epochs.content,
            Self::Placement(epoch) => epoch == epochs.placement,
            Self::Mixed(epoch) => epoch == epochs.all(),
        }
    }
}

/// A successful validation records the domains actually read. No additional
/// per-read field, dependency registry or second weak-reference traversal.
#[derive(Clone, Default)]
pub(crate) struct StateReadValidation(Cell<ValidatedEpoch>);

impl StateReadValidation {
    fn certify(&self, epochs: StateEpochs, content: bool, placement: bool) {
        self.0.set(match (content, placement) {
            (true, true) => ValidatedEpoch::Mixed(epochs.all()),
            (true, false) => ValidatedEpoch::Content(epochs.content),
            (false, true) => ValidatedEpoch::Placement(epochs.placement),
            (false, false) => ValidatedEpoch::Empty,
        });
    }

    /// Visit every changed/dead read exactly once on a cache miss. A clean
    /// immutable snapshot can skip the entire iterator on an epoch hit.
    /// The caller must reset this proof whenever the snapshot is replaced.
    pub(crate) fn poll<'a, T>(
        &self,
        reads: impl IntoIterator<Item = (T, &'a StateRead)>,
        mut invalid: impl FnMut(T, StateReadStatus),
    ) {
        let epochs = STATE_EPOCHS.with(Cell::get);
        if self.0.get().matches(epochs) {
            return;
        }
        let mut content = false;
        let mut placement = false;
        let mut clean = true;
        for (key, read) in reads {
            match read.checked_domain() {
                Ok(StateDomain::Content) => content = true,
                Ok(StateDomain::Placement) => placement = true,
                Err(status) => {
                    clean = false;
                    invalid(key, status);
                }
            }
        }
        if clean {
            self.certify(epochs, content, placement);
        } else {
            self.0.set(ValidatedEpoch::Unchecked);
        }
    }

    pub(crate) fn unchanged(&self, reads: &[StateRead]) -> bool {
        if reads.is_empty() {
            return true;
        }
        let epochs = STATE_EPOCHS.with(Cell::get);
        if self.0.get().matches(epochs) {
            return true;
        }
        let mut content = false;
        let mut placement = false;
        for read in reads {
            match read.checked_domain() {
                Ok(StateDomain::Content) => content = true,
                Ok(StateDomain::Placement) => placement = true,
                Err(_) => {
                    self.0.set(ValidatedEpoch::Unchecked);
                    return false;
                }
            }
        }
        self.certify(epochs, content, placement);
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StateReadStatus { Unchanged, Changed, Dead }

#[derive(Clone)]
pub(crate) struct StateRead {
    state: Weak<StateVersionInner>,
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

    pub(crate) fn id(&self) -> usize {
        self.state.as_ptr() as usize
    }

    pub(crate) fn belongs_to(&self, version: &StateVersion) -> bool {
        self.state.as_ptr() == Rc::as_ptr(&version.0)
    }

    fn checked_domain(&self) -> Result<StateDomain, StateReadStatus> {
        let state = self.state.upgrade().ok_or(StateReadStatus::Dead)?;
        if state.value.get() == self.at {
            Ok(state.domain)
        } else {
            Err(StateReadStatus::Changed)
        }
    }

    pub(crate) fn status(&self) -> StateReadStatus {
        match self.checked_domain() {
            Ok(_) => StateReadStatus::Unchanged,
            Err(status) => status,
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
    fn validation_epoch_rejects_changed_and_dropped_states() {
        let state = StateVersion::default();
        let reads = [StateRead::new(&state)];
        let validation = StateReadValidation::default();
        assert!(validation.unchanged(&reads));
        let generation = state_generation();
        assert!(validation.unchanged(&reads));
        assert_eq!(validation.0.get(), ValidatedEpoch::Content(STATE_EPOCHS.with(Cell::get).content));
        let clone = state.clone();
        drop(clone);
        assert_eq!(state_generation(), generation);
        drop(state);
        assert_ne!(state_generation(), generation);
        assert!(!validation.unchanged(&reads));
        assert_eq!(reads[0].status(), StateReadStatus::Dead);

        let state = StateVersion::default();
        let reads = [StateRead::new(&state)];
        let validation = StateReadValidation::default();
        assert!(validation.unchanged(&reads));
        state.bump();
        assert!(!validation.unchanged(&reads));
        assert_eq!(validation.0.get(), ValidatedEpoch::Unchecked);
    }

    #[test]
    fn bounded_mutation_cursor_reports_observed_changes_and_overflow() {
        let state = StateVersion::placement();
        let read = StateRead::new(&state);
        let cursor = StateMutationCursor::current();
        let unobserved = StateVersion::default();
        unobserved.bump();
        let mut changed = Vec::new();
        state.bump();
        let next = cursor.visit_since(1, |id| changed.push(id)).unwrap();
        assert_eq!(changed, [read.id()]);
        drop(state);
        changed.clear();
        let _ = next.visit_since(1, |id| changed.push(id)).unwrap();
        assert_eq!(changed, [read.id()]);
        let cursor = StateMutationCursor::current();
        let state = StateVersion::default();
        let _read = StateRead::new(&state);
        for _ in 0..=STATE_MUTATION_CAPACITY {
            state.bump();
        }
        assert!(cursor.visit_since(usize::MAX, |_| unreachable!()).is_none());
    }

    #[test]
    fn polling_skips_clean_domains_and_reports_all_invalid_reads() {
        let a = StateVersion::default();
        let b = StateVersion::placement();
        let c = StateVersion::default();
        let reads = [StateRead::new(&a), StateRead::new(&b), StateRead::new(&c)];
        let proof = StateReadValidation::default();
        let mut invalid = Vec::new();
        proof.poll(reads.iter().enumerate(), |i, status| invalid.push((i, status)));
        assert!(invalid.is_empty());
        // An epoch hit must not even consume the supplied iterator.
        proof.poll(reads.iter().enumerate().inspect(|_| panic!("clean proof must skip")), |_, _| unreachable!());
        a.bump();
        drop(c);
        proof.poll(reads.iter().enumerate(), |i, status| invalid.push((i, status)));
        assert_eq!(invalid, [(0, StateReadStatus::Changed), (2, StateReadStatus::Dead)]);

        let proof = StateReadValidation::default();
        let reads = [StateRead::new(&b)];
        proof.poll(reads.iter().enumerate(), |_, _| unreachable!());
        a.bump();
        proof.poll(reads.iter().enumerate().inspect(|_| panic!("unread domain changed")), |_, _| unreachable!());
        b.bump();
        invalid.clear();
        proof.poll(reads.iter().enumerate(), |i, status| invalid.push((i, status)));
        assert_eq!(invalid, [(0, StateReadStatus::Changed)]);
    }

    #[test]
    fn state_validation_ignores_only_unread_domains() {
        let content = StateVersion::default();
        let placement = StateVersion::placement();
        let content_reads = [StateRead::new(&content)];
        let placement_reads = [StateRead::new(&placement)];
        let mixed_reads = [content_reads[0].clone(), placement_reads[0].clone()];
        let content_proof = StateReadValidation::default();
        let placement_proof = StateReadValidation::default();
        let mixed_proof = StateReadValidation::default();
        assert!(content_proof.unchanged(&content_reads));
        assert!(placement_proof.unchanged(&placement_reads));
        assert!(mixed_proof.unchanged(&mixed_reads));
        let before = STATE_EPOCHS.with(Cell::get);
        placement.bump();
        let after = STATE_EPOCHS.with(Cell::get);
        assert_eq!(before.content, after.content);
        assert_ne!(before.placement, after.placement);
        assert!(content_proof.0.get().matches(after));
        assert!(content_proof.unchanged(&content_reads));
        assert!(!placement_proof.unchanged(&placement_reads));
        assert!(!mixed_proof.unchanged(&mixed_reads));

        let placement_reads = [StateRead::new(&placement)];
        assert!(placement_proof.unchanged(&placement_reads));
        content.bump();
        assert!(placement_proof.0.get().matches(STATE_EPOCHS.with(Cell::get)));
        assert!(placement_proof.unchanged(&placement_reads));
        assert!(!content_proof.unchanged(&content_reads));
        drop(placement);
        assert!(!placement_proof.unchanged(&placement_reads));
        assert_eq!(placement_reads[0].status(), StateReadStatus::Dead);
    }

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
    fn two_ordered_runs_merge_repeats_without_sorting() {
        let mut scratch = Vec::new();
        for a in 0..32_u32 {
            for b in 0..32_u32 {
                let left: Vec<u32> = (0..5).filter(|i| a & (1 << i) != 0)
                    .flat_map(|i| [i, i]).collect();
                let right: Vec<u32> = (0..5).filter(|i| b & (1 << i) != 0).collect();
                merge_ordered_runs(&mut scratch, &left, &right);
                let expected: Vec<u32> = (0..5).filter(|i| (a | b) & (1 << i) != 0).collect();
                assert_eq!(scratch, expected);
            }
        }
        let mut interner = Interned::default();
        let log = [1, 1, 3, 2, 3, 4];
        let result = intern_ranges(&mut interner, &mut scratch, &log, [0..3, 3..6].into_iter());
        assert_eq!(&*result, &[1, 2, 3, 4]);
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
