//! Uniform-list row reuse, from GPUI Fast's lists/paint phases.
//! Capture paint past the viewport so a translated row can reveal pixels
//! that the prior frame never submitted.

use super::{PaintIndex, PrepaintStateIndex, RetainedPaintContext, scroll_paint::{ScrollPaint, ScrollPaintTarget}};
use crate::{
    AnyElement, App, AvailableSpace, Bounds, GlobalElementId, Hitbox, Pixels, Point,
    ScrollCapture, Size, StateRead, StateReadValidation, StateMutationCursor, RenderDependencies, Window,
};
use collections::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use std::{any::TypeId, cell::{Cell, RefCell}, collections::VecDeque, ops::Range, rc::Rc, sync::Arc};
use super::lifecycle::{RowNotification, WindowInvalidator};

type RowKey = (u64, usize);

/// Exact captured row membership, compacted by logical list and contiguous
/// row index. Unlike a viewport range this excludes uncaptured rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RowRun {
    list: u64,
    start: usize,
    end: usize,
}

#[derive(Default)]
struct RowRuns {
    runs: Vec<RowRun>,
}

impl RowRuns {
    fn record(&mut self, (list, row): RowKey) {
        if let Some(last) = self.runs.last_mut()
            && last.list == list
            && last.end == row
        {
            last.end += 1;
            return;
        }
        self.runs.push(RowRun { list, start: row, end: row + 1 });
    }

    fn normalize(&mut self) {
        if !self.runs.windows(2).all(|pair| {
            pair[0].list < pair[1].list
                || (pair[0].list == pair[1].list && pair[0].end < pair[1].start)
        }) {
            // Paint order need not be the order of list identities. Sort
            // runs, not individual rows, and coalesce any repeated ranges.
            self.runs.sort_unstable_by_key(|run| (run.list, run.start));
            let mut written = 0;
            for index in 0..self.runs.len() {
                let run = self.runs[index];
                if written != 0 {
                    let last = &mut self.runs[written - 1];
                    if last.list == run.list && run.start <= last.end {
                        last.end = last.end.max(run.end);
                        continue;
                    }
                }
                self.runs[written] = run;
                written += 1;
            }
            self.runs.truncate(written);
        }
    }

    /// Old minus new, expressed as ranges. Never enumerate kept row keys.
    fn visit_retired(&self, next: &Self, mut visit: impl FnMut(RowRun)) {
        let mut j = 0;
        for old in &self.runs {
            let mut start = old.start;
            while let Some(kept) = next.runs.get(j) {
                if kept.list < old.list || (kept.list == old.list && kept.end <= start) {
                    j += 1;
                    continue;
                }
                if kept.list != old.list || kept.start >= old.end {
                    break;
                }
                if start < kept.start {
                    visit(RowRun { list: old.list, start, end: kept.start });
                }
                start = start.max(kept.end);
                if start >= old.end {
                    // This kept interval may also overlap the next old run.
                    break;
                }
                j += 1;
            }
            if start < old.end {
                visit(RowRun { list: old.list, start, end: old.end });
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum RowDependencyKey {
    Entity(crate::EntityId),
    Global(TypeId, Option<bool>),
    State(usize),
}

// A typical dependency is read by only one or two rows. High fan-out
// dependencies (notably UniformList batches) need constant expected-time
// retirement rather than repeated linear scans of a large reader list.
enum RowReaders {
    Small(SmallVec<[RowKey; 2]>),
    Many(FxHashSet<RowKey>),
}

impl Default for RowReaders {
    fn default() -> Self { Self::Small(SmallVec::new()) }
}

impl RowReaders {
    const PROMOTE_AT: usize = 8;
    const DEMOTE_AT: usize = 2;

    fn insert(&mut self, row: RowKey) {
        match self {
            Self::Small(rows) => {
                if rows.contains(&row) { return; }
                if rows.len() == Self::PROMOTE_AT {
                    let mut many = FxHashSet::default();
                    many.extend(rows.iter().copied());
                    many.insert(row);
                    *self = Self::Many(many);
                } else {
                    rows.push(row);
                }
            }
            Self::Many(rows) => { rows.insert(row); }
        }
    }

    fn remove(&mut self, row: RowKey) {
        match self {
            Self::Small(rows) => {
                if let Some(index) = rows.iter().position(|&reader| reader == row) {
                    rows.swap_remove(index);
                }
            }
            Self::Many(rows) => {
                rows.remove(&row);
                if rows.len() <= Self::DEMOTE_AT {
                    *self = Self::Small(rows.iter().copied().collect());
                } else if rows.capacity() > rows.len().saturating_mul(4) {
                    rows.shrink_to(rows.len().saturating_mul(2));
                }
            }
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Small(rows) => rows.is_empty(),
            Self::Many(rows) => rows.is_empty(),
        }
    }

    fn visit(&self, mut visit: impl FnMut(RowKey)) {
        match self {
            Self::Small(rows) => rows.iter().copied().for_each(&mut visit),
            Self::Many(rows) => rows.iter().copied().for_each(&mut visit),
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        match self {
            Self::Small(rows) => rows.len(),
            Self::Many(rows) => rows.len(),
        }
    }
}

/// Index only the immutable, committed row snapshots. The original
/// dependency slices remain authoritative and shared with owner recording.
#[derive(Default)]
struct RowNotificationRouting {
    edges: FxHashMap<RowDependencyKey, RowReaders>,
    dirty: FxHashSet<RowKey>,
    cursor: Option<u64>,
    overflow: bool,
    state_cursor: Option<StateMutationCursor>,
    state_generation: Option<u64>,
    state_dirty: FxHashSet<RowKey>,
}

impl RowNotificationRouting {
    const MIN_ROWS: usize = 16;

    fn has_reads(reads: &RenderDependencies) -> bool {
        !reads.entities.is_empty() || !reads.globals.is_empty()
    }

    fn insert(&mut self, key: RowDependencyKey, row: RowKey) {
        self.edges.entry(key).or_default().insert(row);
    }

    fn remove(&mut self, key: RowDependencyKey, row: RowKey) {
        if let Some(rows) = self.edges.get_mut(&key) {
            rows.remove(row);
            if rows.is_empty() {
                self.edges.remove(&key);
            }
        }
    }

    // Both entity and global snapshots are sorted and unique. Do not
    // touch the shared reverse edges when only a subset of reads changes.
    fn update_keys<T: Copy + Ord>(
        &mut self, row: RowKey, old: &[T], new: &[T],
        key: impl Fn(T) -> RowDependencyKey,
    ) {
        if std::ptr::eq(old, new) {
            return;
        }
        let (mut i, mut j) = (0, 0);
        while i < old.len() && j < new.len() {
            match old[i].cmp(&new[j]) {
                std::cmp::Ordering::Less => { self.remove(key(old[i]), row); i += 1; }
                std::cmp::Ordering::Greater => { self.insert(key(new[j]), row); j += 1; }
                std::cmp::Ordering::Equal => { i += 1; j += 1; }
            }
        }
        for &value in &old[i..] { self.remove(key(value), row); }
        for &value in &new[j..] { self.insert(key(value), row); }
    }

    fn update(&mut self, row: RowKey, old: Option<&RenderDependencies>, new: Option<&RenderDependencies>) {
        if !matches!((old, new), (Some(a), Some(b)) if a.same_routes(b)) {
            self.update_keys(row,
            old.map_or(&[][..], |reads| reads.entities.as_ref()),
            new.map_or(&[][..], |reads| reads.entities.as_ref()),
            RowDependencyKey::Entity);
        self.update_keys(row,
            old.map_or(&[][..], |reads| reads.globals.as_ref()),
            new.map_or(&[][..], |reads| reads.globals.as_ref()),
            |(global, exists)| RowDependencyKey::Global(global, exists));
        }
        let old_states = old.map_or(&[][..], |reads| reads.states.as_ref());
        let new_states = new.map_or(&[][..], |reads| reads.states.as_ref());
        // State reads preserve first-observation order, not identity order.
        // Equal identity sequences require no edge changes, even if versions
        // changed. For reordered or changed sets, sort only identity scratch.
        if !std::ptr::eq(old_states, new_states)
            && !(old_states.len() == new_states.len()
                && old_states.iter().zip(new_states).all(|(a, b)| a.id() == b.id()))
        {
            let mut before: SmallVec<[usize; 8]> = old_states.iter().map(StateRead::id).collect();
            let mut after: SmallVec<[usize; 8]> = new_states.iter().map(StateRead::id).collect();
            before.sort_unstable();
            after.sort_unstable();
            before.dedup();
            after.dedup();
            self.update_keys(row, &before, &after, RowDependencyKey::State);
        }
    }

    fn mark_state(&mut self, identity: usize) {
        if let Some(readers) = self.edges.get(&RowDependencyKey::State(identity)) {
            let dirty = &mut self.state_dirty;
            readers.visit(|row| { dirty.insert(row); });
        }
    }

    fn refresh_state_proof(&mut self, rows: &FxHashMap<RowKey, RowRecord>, generation: u64) {
        self.state_dirty.clear();
        for (&key, row) in rows {
            if !row.prepaint.validation.unchanged(&row.prepaint.reads.states) {
                self.state_dirty.insert(key);
            }
        }
        self.state_cursor = Some(StateMutationCursor::current());
        self.state_generation = Some(generation);
    }

    fn state_unchanged(
        &mut self, rows: &FxHashMap<RowKey, RowRecord>, key: RowKey,
        row: &RowRecord, invalidator: &WindowInvalidator,
    ) -> bool {
        if row.prepaint.reads.states.is_empty() {
            return true;
        }
        if rows.len() < Self::MIN_ROWS {
            return row.prepaint.validation.unchanged(&row.prepaint.reads.states);
        }
        if self.cursor.is_none() {
            self.build(rows, invalidator.row_notification_epoch());
        }
        let generation = crate::state_generation();
        if self.state_generation != Some(generation) {
            let next = self.state_cursor.and_then(|cursor| {
                cursor.visit_since((rows.len() / 2).max(16), |identity| self.mark_state(identity))
            });
            if let Some(next) = next {
                self.state_cursor = Some(next);
                self.state_generation = Some(generation);
            } else {
                // Initial observation, overflow or dense mutation: certify
                // every row before advancing the shared state cursor.
                self.refresh_state_proof(rows, generation);
            }
        }
        if !self.state_dirty.contains(&key) {
            return true;
        }
        // A mutation may predate a newly captured snapshot, or match an
        // identity whose version was already observed. Verify only hits.
        if row.prepaint.validation.unchanged(&row.prepaint.reads.states) {
            self.state_dirty.remove(&key);
            true
        } else {
            false
        }
    }

    fn route(
        &mut self, event: RowNotification, sequence: u64, now: u64,
        epoch_of: impl Fn(RowKey) -> Option<u64>,
    ) {
        let edges = &self.edges;
        let dirty = &mut self.dirty;
        let mut mark = |key| {
            if let Some(rows) = edges.get(&key) {
                rows.visit(|row| {
                    // A row captured after this notification already
                    // observed the new value. Compare relative ages to
                    // preserve ordering across sequence wraparound.
                    if epoch_of(row).is_some_and(|epoch|
                        now.wrapping_sub(epoch) > now.wrapping_sub(sequence))
                    {
                        dirty.insert(row);
                    }
                });
            }
        };
        match event {
            RowNotification::Entity(entity) => mark(RowDependencyKey::Entity(entity)),
            RowNotification::Global(global, exists) => {
                mark(RowDependencyKey::Global(global, None));
                mark(RowDependencyKey::Global(global, Some(!exists)));
            }
        }
    }

    fn reset_cursor(&mut self, oldest: u64) {
        self.cursor = Some(oldest);
        self.dirty.clear();
        self.overflow = false;
    }

    fn oldest_epoch(rows: &FxHashMap<RowKey, RowRecord>, now: u64) -> u64 {
        rows.values().filter(|row| Self::has_reads(&row.prepaint.reads))
            .map(|row| row.notification_epoch)
            .max_by_key(|epoch| now.wrapping_sub(*epoch))
            .unwrap_or(now)
    }

    fn build(&mut self, rows: &FxHashMap<RowKey, RowRecord>, now: u64) {
        self.edges.clear();
        self.state_cursor = None;
        self.state_generation = None;
        self.state_dirty.clear();
        for (&key, row) in rows {
            self.update(key, None, Some(&row.prepaint.reads));
        }
        self.reset_cursor(Self::oldest_epoch(rows, now));
    }

    fn unchanged(
        &mut self, rows: &FxHashMap<RowKey, RowRecord>, key: RowKey,
        row: &RowRecord, invalidator: &WindowInvalidator,
    ) -> bool {
        let now = invalidator.row_notification_epoch();
        if row.notification_epoch == now || !Self::has_reads(&row.prepaint.reads) {
            return true;
        }
        if rows.len() < Self::MIN_ROWS {
            return invalidator.row_dependencies_unchanged(row.notification_epoch, &row.prepaint.reads);
        }
        if self.cursor.is_none() {
            self.build(rows, now);
        }
        if !self.overflow {
            let since = self.cursor.expect("committed row notification cursor");
            if since != now {
                let next = invalidator.visit_row_notifications(since, |sequence, event| {
                    self.route(event, sequence, now, |key| rows.get(&key).map(|row| row.notification_epoch));
                });
                if let Some(next) = next {
                    self.cursor = Some(next);
                } else {
                    self.overflow = true;
                }
            }
        }
        if self.overflow {
            // A lagging frame cannot prove coverage from the shared cursor.
            // Rows with newer snapshots may still prove their own history.
            return invalidator.row_dependencies_unchanged(row.notification_epoch, &row.prepaint.reads);
        }
        !self.dirty.contains(&key)
    }

    fn reconcile(
        &mut self, old: &FxHashMap<RowKey, RowRecord>,
        new: &FxHashMap<RowKey, RowRecord>, old_runs: &RowRuns, new_runs: &RowRuns,
        changed: &[RowKey], oldest: u64,
    ) {
        old_runs.visit_retired(new_runs, |run| {
            for index in run.start..run.end {
                let key = (run.list, index);
                let row = &old[&key];
                self.update(key, Some(&row.prepaint.reads), None);
                self.state_dirty.remove(&key);
            }
        });
        // The candidate records actual dependency changes as rows are
        // published. Kept runs never need per-row retirement lookups.
        for &key in changed {
            let Some(row) = new.get(&key) else { continue; };
            let previous = old.get(&key);
            self.update(key, previous.map(|row| &row.prepaint.reads), Some(&row.prepaint.reads));
            if self.state_cursor.is_some()
                && !previous.is_some_and(|old| Rc::ptr_eq(&old.prepaint.reads.states, &row.prepaint.reads.states))
            {
                self.state_dirty.remove(&key);
                if !row.prepaint.validation.unchanged(&row.prepaint.reads.states) {
                    self.state_dirty.insert(key);
                }
            }
        }
        self.reset_cursor(oldest);
        let target = self.edges.len().max(Self::MIN_ROWS);
        if self.edges.capacity() > target.saturating_mul(4) {
            self.edges.shrink_to(target);
        }
        let row_target = new.len().max(Self::MIN_ROWS);
        if self.dirty.capacity() > row_target.saturating_mul(4) {
            self.dirty.shrink_to(row_target);
        }
        if self.state_dirty.capacity() > row_target.saturating_mul(4) {
            self.state_dirty.shrink_to(row_target);
        }
    }
}

#[derive(Default)]
pub(super) struct ScrollRows {
    rows: FxHashMap<RowKey, RowRecord>,
    notification_routing: RefCell<RowNotificationRouting>,
    changed_rows: Vec<RowKey>,
    membership: RowRuns,
    oldest_notification_epoch: Option<u64>,
    measurements: FxHashMap<GlobalElementId, (u64, Rc<RowMeasurement>)>,
    pub(super) enabled: bool,
    blocked: bool,
}

impl ScrollRows {
    pub(super) fn collect_atlas_tile_ids_into(&self, live: &mut collections::FxHashSet<(crate::AtlasTextureId, u32)>) {
        for row in self.rows.values() {
            row.content.collect_atlas_tile_ids_into(live);
        }
    }

    pub(super) fn clear(&mut self) {
        self.rows.clear();
        *self.notification_routing.get_mut() = Default::default();
        self.changed_rows.clear();
        self.membership.runs.clear();
        self.oldest_notification_epoch = None;
        self.measurements.clear();
        self.enabled = false;
        self.blocked = false;
    }

    /// Transfer an already-built index only after successful publication.
    /// Candidate frames cannot mutate the previous frame's reverse edges.
    pub(super) fn inherit_notification_routing(&mut self, previous: &mut Self, now: u64) {
        // Normalize even when the index is not yet allocated: the committed
        // frame may lazily build it at the next notification.
        self.membership.normalize();
        if self.rows.len() < RowNotificationRouting::MIN_ROWS {
            return;
        }
        let mut routing = std::mem::take(previous.notification_routing.get_mut());
        if routing.cursor.is_some() {
            routing.reconcile(&previous.rows, &self.rows, &previous.membership,
                &self.membership, &self.changed_rows, self.oldest_notification_epoch.unwrap_or(now));
            *self.notification_routing.get_mut() = routing;
        }
    }

    pub(super) fn trim_for_reuse(&mut self, current: &Self) {
        let target = current.rows.len().max(32);
        if self.rows.capacity() > target.saturating_mul(4) {
            self.rows.shrink_to(target);
        }
        if self.changed_rows.capacity() > target.saturating_mul(4) {
            self.changed_rows.shrink_to(target);
        }
        let run_target = current.membership.runs.len().max(4);
        if self.membership.runs.capacity() > run_target.saturating_mul(4) {
            self.membership.runs.shrink_to(run_target);
        }
        let target = current.measurements.len().max(4);
        if self.measurements.capacity() > target * 4 {
            self.measurements.shrink_to(target);
        }
    }
}

pub(crate) type RowRevision = Rc<dyn Fn(usize) -> u64>;

#[derive(PartialEq)]
struct ListContext {
    owner: crate::EntityId,
    versioned: bool,
    identity: RowListIdentity,
    item_count: usize,
    available_space: Size<AvailableSpace>,
    inherited: RetainedPaintContext,
}

#[derive(PartialEq)]
pub(crate) enum RowListIdentity {
    Uniform(GlobalElementId, Size<Pixels>),
    Variable(StateRead),
}

pub(crate) struct RowPlan {
    key: u64,
    revision: u64,
    notification_epoch: u64,
    placement_only: bool,
    allow_reuse: bool,
    row_revision: Option<RowRevision>,
    context: Rc<ListContext>,
    // Rows captured by one plan share exactly one immutable context.
    // Compare its inherited style and list identity once per source group.
    previous_context: RefCell<Option<(Rc<ListContext>, bool)>>,
    hover_held: bool,
    // Both list implementations prepaint and paint rows in the same order.
    // The transient records therefore need no per-frame hash table.
    prepainted: VecDeque<(usize, RowPrepaint)>,
}

impl RowPlan {
    pub(crate) fn row_revision(&self, row: usize) -> u64 {
        self.row_revision.as_ref().map_or(0, |revision| revision(row))
    }

    fn matches_context(&self, previous: &Rc<ListContext>) -> bool {
        if Rc::ptr_eq(&self.context, previous) {
            return true;
        }
        let mut cached = self.previous_context.borrow_mut();
        if let Some((owner, matches)) = &*cached
            && Rc::ptr_eq(owner, previous)
        {
            return *matches;
        }
        let matches = *self.context == **previous;
        *cached = Some((Rc::clone(previous), matches));
        matches
    }

    pub(crate) fn record_prepaint(&mut self, row: usize, prepaint: RowPrepaint) {
        debug_assert!(self.prepainted.back().is_none_or(|(previous, _)| *previous < row));
        self.prepainted.push_back((row, prepaint));
    }

    fn take_prepaint(&mut self, row: usize) -> Option<RowPrepaint> {
        let (next, _) = self.prepainted.front()?;
        debug_assert!(*next >= row, "scroll prepaint records must follow paint order");
        (*next == row).then(|| self.prepainted.pop_front().unwrap().1)
    }

    pub(crate) fn can_capture(&self, item: &mut AnyElement) -> bool {
        if self.context.inherited.visual_content_mask.corner_radii == Default::default() {
            item.can_capture_scroll_paint()
        } else {
            item.can_capture_scrolling_subtree(ScrollCapture::RoundedViewport)
        }
    }
}

pub(crate) struct RowPrepaint {
    pub(crate) row_revision: u64,
    pub(crate) origin: Point<Pixels>,
    pub(crate) records: Range<PrepaintStateIndex>,
    pub(crate) reads: RenderDependencies,
    pub(crate) validation: StateReadValidation,
}

struct RowRecord {
    revision: u64,
    notification_epoch: u64,
    context: Rc<ListContext>,
    prepaint: RowPrepaint,
    paint: Range<PaintIndex>,
    content: Arc<ScrollPaint>,
}

struct RowMeasurement {
    owner: crate::EntityId,
    index: usize,
    count: usize,
    context: RetainedPaintContext,
    reads: RenderDependencies,
    notification_epoch: Cell<u64>,
    validation: StateReadValidation,
    size: Size<Pixels>,
}

// Both the committed and current owner path must agree. A directly dirty
// ancestor may have changed an otherwise untracked closure capture. A
// dirty descendant also prevents implicit reuse of the parent's row records.
// An explicit revision or a certified row-only notification may admit the
// directly dirty owner, never a directly dirty ancestor or a changed path.
fn clean_owner_path(
    owner: crate::EntityId,
    current: &[crate::EntityId],
    committed: impl Iterator<Item = crate::EntityId>,
    dirty: &FxHashSet<crate::EntityId>,
    direct: &FxHashSet<crate::EntityId>,
    versioned: bool,
) -> bool {
    current.last() == Some(&owner)
        && (!dirty.contains(&owner) || (versioned && direct.contains(&owner)))
        && current.iter().all(|id| (versioned && *id == owner) || !direct.contains(id))
        && committed.eq(current.iter().rev().copied())
}

impl Window {
    fn row_replay_admission(&self, versioned: bool) -> Option<(crate::EntityId, u64, bool)> {
        if !self.scroll_row_replay_is_enabled() || self.force_view_cache_refresh {
            return None;
        }
        let owner = self.rendered_entity_stack.last().copied()?;
        let (revision, placement_only) = self.invalidator.row_replay_revision()?;
        // An automatic proof never overrides a second independently dirty
        // descendant. Explicit revisions retain their existing contract.
        let allow_dirty_owner = versioned || (!placement_only
            && self.invalidator.row_only_dirty(owner)
            && self.direct_dirty_views.iter().all(|&dirty| dirty == owner
                || (self.rendered_frame.dispatch_tree.has_view_route(dirty)
                    && !self.rendered_frame.dispatch_tree.view_path_reversed(dirty)
                        .any(|ancestor| ancestor == owner))));
        if !placement_only
            && (!self.has_completed_rendered_frame
                || self.rendered_frame.focus != self.focus
                || self.rendered_frame.window_active != self.active.get()
                || !clean_owner_path(
                    owner, &self.rendered_entity_stack,
                    self.rendered_frame.dispatch_tree.view_path_reversed(owner),
                    &self.dirty_views, &self.direct_dirty_views,
                    allow_dirty_owner,
                ))
        {
            return None;
        }
        Some((owner, revision, placement_only))
    }

    pub(crate) fn scroll_row_replay_is_enabled(&self) -> bool {
        let rows = &self.next_frame.scroll_rows;
        rows.enabled && !rows.blocked
    }

    /// A uniform list need only rebuild its measurement sample when its
    /// content or inherited measurement context changes. Adapted from GPUI
    /// Fast's uniform-list measurement record.
    pub(crate) fn measure_uniform_row(
        &mut self,
        id: Option<&GlobalElementId>,
        index: usize,
        count: usize,
        cx: &mut App,
        measure: impl FnOnce(&mut Self, &mut App) -> (Size<Pixels>, bool),
    ) -> Size<Pixels> {
        let Some(id) = id else {
            return measure(self, cx).0;
        };
        let Some((owner, revision, placement_only)) = self.row_replay_admission(false) else {
            return measure(self, cx).0;
        };
        if let Some((previous_revision, previous)) = self.rendered_frame.scroll_rows.measurements.get(id)
            && previous.owner == owner
            && (!placement_only || *previous_revision == revision)
            && previous.index == index
            && previous.count == count
            && self.retained_paint_context_matches(&previous.context)
            && self.invalidator.row_dependencies_unchanged(previous.notification_epoch.get(), &previous.reads)
            && previous.validation.unchanged(&previous.reads.states)
        {
            let size = previous.size;
            // A successfully validated immutable measurement may advance
            // its notification proof without rebuilding or cloning reads.
            previous.notification_epoch.set(self.invalidator.row_notification_epoch());
            self.next_frame.scroll_rows.measurements.insert(id.clone(), (revision, previous.clone()));
            self.replayed_view_owners.insert(self.current_view());
            return size;
        }
        // Do not clone the inherited text style on a measurement hit.
        let context = self.current_retained_paint_context();
        let notification_epoch = self.invalidator.row_notification_epoch();
        let start = cx.entities.dependency_read_index();
        let (size, cacheable) = measure(self, cx);
        if cacheable {
            self.next_frame.scroll_rows.measurements.insert(id.clone(), (revision, Rc::new(RowMeasurement {
                owner,
                index,
                count,
                context,
                reads: cx.entities.dependency_reads_since(start),
                notification_epoch: Cell::new(notification_epoch),
                validation: StateReadValidation::default(),
                size,
            })));
        }
        size
    }

    /// A paint-only effect can change what a retained row emits without
    /// changing prepaint geometry. Its subtree must create fresh elements.
    pub(crate) fn with_scroll_replay_barrier<R>(
        &mut self,
        blocked: bool,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if !blocked || !self.scroll_row_replay_is_enabled() {
            return f(self);
        }
        self.next_frame.scroll_rows.blocked = true;
        let result = f(self);
        self.next_frame.scroll_rows.blocked = false;
        result
    }

    pub(crate) fn plan_scroll_rows(
        &self,
        identity: RowListIdentity,
        item_count: usize,
        available_space: Size<AvailableSpace>,
        row_revision: Option<RowRevision>,
        cx: &App,
    ) -> Option<RowPlan> {
        let versioned = row_revision.is_some();
        if !self.scroll_row_replay_is_enabled() || self.force_view_cache_refresh {
            return None;
        }
        let admitted = self.row_replay_admission(versioned);
        let allow_reuse = admitted.is_some();
        // A scoped content draw that cannot reuse old rows may still
        // capture freshly built rows for the next notification.
        let (owner, revision, placement_only) = match admitted {
            Some(admission) => admission,
            None => {
                let owner = self.rendered_entity_stack.last().copied()?;
                let (revision, placement_only) = self.invalidator.row_replay_revision()?;
                if placement_only { return None; }
                (owner, revision, false)
            }
        };
        let viewport = self.visual_content_mask();
        if self.visual_scale() != 1.
            || self.content_mask() != viewport
            || self.element_visual_transform.translation != Point::default()
            || viewport.bounds.is_empty()
            || cx.has_active_drag()
        {
            return None;
        }
        let debug = self.debug_visualization(cx);
        if debug.show_layout_bounds || debug.show_element_updates {
            return None;
        }
        #[cfg(any(feature = "inspector", debug_assertions))]
        if self.inspector.is_some() {
            return None;
        }
        #[cfg(debug_assertions)]
        if cx.has_global::<crate::DebugBelow>() {
            return None;
        }
        let key = crate::fast::layout_key::prepaint_layout_key(self)?;
        Some(RowPlan {
            key,
            revision,
            notification_epoch: self.invalidator.row_notification_epoch(),
            placement_only,
            allow_reuse,
            row_revision,
            hover_held: self.scroll_hover_is_held(cx),
            prepainted: VecDeque::new(),
            previous_context: RefCell::new(None),
            context: Rc::new(ListContext {
                owner,
                versioned,
                identity,
                item_count,
                available_space,
                inherited: self.current_retained_paint_context(),
            }),
        })
    }

    pub(crate) fn can_reuse_scroll_row(
        &self,
        plan: &RowPlan,
        row: usize,
        origin: Point<Pixels>,
    ) -> bool {
        if !plan.allow_reuse { return false; }
        let key = (plan.key, row);
        let rows = &self.rendered_frame.scroll_rows;
        let Some(record) = rows.rows.get(&key) else {
            return false;
        };
        if (plan.placement_only && record.revision != plan.revision)
            || !plan.matches_context(&record.context)
            || record.prepaint.row_revision != plan.row_revision(row)
        {
            return false;
        }
        {
            let mut routing = rows.notification_routing.borrow_mut();
            if !routing.unchanged(&rows.rows, key, record, &self.invalidator)
                || !routing.state_unchanged(&rows.rows, key, record, &self.invalidator)
            {
                return false;
            }
        }
        // Scoped records are only borrowed from the previous committed
        // frame, after the entire owner path was certified. A carried row
        // is stamped with the current revision when remembered below.
        let scale = self.scale_factor();
        let viewport = plan
            .context
            .inherited
            .visual_content_mask
            .scale(scale)
            .bounds;
        let Some(delta) = record.content.translation(origin, scale) else {
            return false;
        };
        let logical_delta = origin - record.prepaint.origin;
        let hitboxes = hitbox_range(&record.prepaint);
        record.content.covers(viewport, delta)
            && self.can_reuse_prepaint(&record.prepaint.records)
            && self.can_reuse_paint(&record.paint)
            && (plan.hover_held || !self.hits_scroll_row(&self.rendered_frame.hitboxes, hitboxes, logical_delta))
    }

    pub(crate) fn reuse_scroll_row(
        &mut self,
        plan: &RowPlan,
        row: usize,
        origin: Point<Pixels>,
    ) -> RowPrepaint {
        let record = &self.rendered_frame.scroll_rows.rows[&(plan.key, row)];
        let recorded = record.prepaint.records.clone();
        let reads = record.prepaint.reads.clone();
        let validation = record.prepaint.validation.clone();
        let row_revision = record.prepaint.row_revision;
        let start = self.prepaint_index();
        let delta = origin - record.prepaint.origin;
        if !hitbox_range(&record.prepaint).is_empty() {
            self.next_frame.input_clip_epoch = self.next_frame.input_clip_epoch.wrapping_add(1);
        }
        self.next_frame.hitboxes.share(
            &self.rendered_frame.hitboxes, hitbox_range(&record.prepaint), delta,
        );
        let parent = self.next_frame.dispatch_tree.active_node_id();
        self.replay_prepaint_records(recorded, |_| parent);
        self.replayed_view_owners.insert(self.current_view());
        RowPrepaint {
            row_revision,
            origin,
            records: start..self.prepaint_index(),
            reads,
            validation,
        }
    }

    pub(crate) fn paint_planned_scroll_row(
        &mut self,
        plan: Option<&mut RowPlan>,
        row: usize,
        item: Option<AnyElement>,
        cx: &mut App,
    ) {
        let prepaint = plan.and_then(|plan| {
            plan.take_prepaint(row).map(|prepaint| (plan, prepaint))
        });
        match (item, prepaint) {
            (Some(mut item), Some((plan, prepaint))) => {
                self.paint_scroll_row(plan, row, &mut item, prepaint, cx)
            }
            (None, Some((plan, prepaint))) => self.paint_reused_scroll_row(plan, row, prepaint, cx),
            (Some(mut item), None) => item.paint(self, cx),
            (None, None) => unreachable!("retained scroll row owns its prepaint records"),
        }
    }

    fn paint_scroll_row(
        &mut self,
        plan: &RowPlan,
        row: usize,
        item: &mut AnyElement,
        mut prepaint: RowPrepaint,
        cx: &mut App,
    ) {
        // Capturing inside an effect or renderer animation changes what the
        // child would emit. Leave those paths on their regular painter.
        let viewport = self.visual_content_mask();
        let hitboxes = hitbox_range(&prepaint);
        let suppress_hover = plan.hover_held && !hitboxes.is_empty();
        if self.next_frame.painter.is_capturing_blur()
            || self.scene_animation.is_some()
            || self.next_frame.hitboxes.slices(hitboxes.clone())
                .any(|(slice, _)| slice.iter().any(|hitbox| hitbox.content_mask != viewport))
            || (!suppress_hover && self.hits_scroll_row(&self.next_frame.hitboxes, hitboxes, Point::default()))
        {
            item.paint(self, cx);
            return;
        }
        let read_start = cx.entities.dependency_read_index();
        let start = self.paint_index();
        let previous_source = self.rendered_frame.scroll_rows.rows.get(&(plan.key, row))
            .map(|previous| Arc::clone(&previous.content));
        let content = self.capture_scroll_paint(prepaint.origin, item, suppress_hover,
            previous_source.as_deref(), ScrollPaintTarget::Row, cx);
        if suppress_hover {
            self.restore_scroll_row_after_wheel(cx);
        }
        let paint = start..self.paint_index();
        let paint_reads = cx.entities.row_reads_since(read_start);
        if !paint_reads.states.is_empty() {
            prepaint.validation = StateReadValidation::default();
        }
        prepaint.reads.merge_replayed(&paint_reads);
        self.remember_scroll_row(plan, row, prepaint, paint, content);
    }

    fn paint_reused_scroll_row(&mut self, plan: &RowPlan, row: usize, prepaint: RowPrepaint, cx: &App) {
        let previous = &self.rendered_frame.scroll_rows.rows[&(plan.key, row)];
        let content = previous.content.clone();
        let old_paint = previous.paint.clone();
        let same_placement = previous.prepaint.origin == prepaint.origin;
        let start = self.paint_index();
        let scale = self.scale_factor();
        let viewport = self.visual_content_mask();
        let delta = prepaint.origin.scale(scale) - content.origin.scale(scale);
        let old_range = old_paint.start.scene_index..old_paint.end.scene_index;
        let was_deferred = old_range.len() == 1 && matches!(
            self.rendered_frame.scene().paint_operations.view(old_range.start),
            Some(crate::scene::PaintOperationRef::DeferredScroll(_))
        );
        // Both List and UniformList enter here after validating input and
        // content dependencies. Keep the existing row's source alive and
        // replace only its native placement. New rows still paint normally.
        let deferred = (!same_placement || was_deferred)
            && self.platform_window.supports_deferred_scroll()
            && self.scene_animation.is_none()
            && content.native_row_placement(viewport, scale, delta)
                .is_some_and(|placement| self.next_frame.painter.push_scroll_placement_against(
                    self.rendered_frame.scene(), old_range.clone(), placement,
                ));
        if !deferred {
            if same_placement && !was_deferred {
                self.replay_validated_scene(old_range);
            } else {
                content.replay(&mut self.next_frame.painter, viewport.scale(scale), delta);
            }
        }
        self.replay_validated_paint_records(&old_paint);
        if !hitbox_range(&prepaint).is_empty()
            && (plan.hover_held || (delta != Point::default()
                && old_paint.start.mouse_listeners_index != old_paint.end.mouse_listeners_index))
        {
            self.restore_scroll_row_after_wheel(cx);
        }
        let paint = start..self.paint_index();
        self.remember_scroll_row(plan, row, prepaint, paint, content);
    }

    fn restore_scroll_row_after_wheel(&mut self, cx: &App) {
        let owner = self.current_view();
        self.scroll_hover.row_owners.insert(owner);
        self.schedule_scroll_hover_restore(cx);
    }

    fn remember_scroll_row(
        &mut self,
        plan: &RowPlan,
        row: usize,
        mut prepaint: RowPrepaint,
        paint: Range<PaintIndex>,
        content: Arc<ScrollPaint>,
    ) {
        let key = (plan.key, row);
        let previous = &self.rendered_frame.scroll_rows;
        let next = &mut self.next_frame.scroll_rows;
        let old = previous.rows.get(&key);
        if let Some(old) = old {
            let reads = &old.prepaint.reads;
            if !Rc::ptr_eq(&reads.entities, &prepaint.reads.entities)
                && reads.entities == prepaint.reads.entities
            {
                prepaint.reads.entities = reads.entities.clone();
            }
            if !Rc::ptr_eq(&reads.globals, &prepaint.reads.globals)
                && reads.globals == prepaint.reads.globals
            {
                prepaint.reads.globals = reads.globals.clone();
            }
            // Equal state observations are immutable and carry the same
            // proof. Adopt the old allocation even after a fresh capture.
            if !Rc::ptr_eq(&reads.states, &prepaint.reads.states)
                && reads.states == prepaint.reads.states
            {
                prepaint.reads.states = reads.states.clone();
                prepaint.validation = old.prepaint.validation.clone();
            }
        }
        let context = old.filter(|old| plan.matches_context(&old.context))
            .map_or_else(|| plan.context.clone(), |old| old.context.clone());
        if previous.rows.len() >= RowNotificationRouting::MIN_ROWS
            && !old.is_some_and(|old| {
                old.prepaint.reads.same_routes(&prepaint.reads)
                    && Rc::ptr_eq(&old.prepaint.reads.states, &prepaint.reads.states)
            })
        {
            next.changed_rows.push(key);
        }
        next.membership.record(key);
        if RowNotificationRouting::has_reads(&prepaint.reads) {
            let now = self.invalidator.row_notification_epoch();
            let epoch = plan.notification_epoch;
            if next.oldest_notification_epoch.is_none_or(|old| {
                now.wrapping_sub(epoch) > now.wrapping_sub(old)
            }) {
                next.oldest_notification_epoch = Some(epoch);
            }
        }
        next.rows.insert(
            key,
            RowRecord {
                revision: plan.revision,
                notification_epoch: plan.notification_epoch,
                context,
                prepaint,
                paint,
                content,
            },
        );
    }

    pub(super) fn hits_scroll_row(&self, hitboxes: &super::frame::HitboxStore, range: Range<usize>, delta: Point<Pixels>) -> bool {
        !self.pointer_outside && hitboxes.any_at(range, self.mouse_position, delta)
    }
}

fn hitbox_range(prepaint: &RowPrepaint) -> Range<usize> {
    prepaint.records.start.hitboxes_index..prepaint.records.end.hitboxes_index
}

#[cfg(test)]
mod scoped_row_tests {
    use super::*;
    use crate::EntityId;

    #[test]
    fn row_run_retirement_matches_exhaustive_membership_difference() {
        fn runs(mask: u32) -> RowRuns {
            let mut runs = RowRuns::default();
            for index in 0..6 {
                if mask & (1 << index) != 0 {
                    runs.record((1, index));
                }
            }
            runs.normalize();
            runs
        }
        for old in 0..64 {
            for new in 0..64 {
                let mut retired = 0;
                runs(old).visit_retired(&runs(new), |run| {
                    assert_eq!(run.list, 1);
                    for index in run.start..run.end {
                        assert_eq!(retired & (1 << index), 0);
                        retired |= 1 << index;
                    }
                });
                assert_eq!(retired, old & !new);
            }
        }
    }

    #[test]
    fn row_run_retirement_preserves_holes_and_multiple_list_identities() {
        let mut old = RowRuns::default();
        for key in [(9, 5), (9, 6), (3, 1), (3, 3), (3, 2), (9, 8), (3, 3)] {
            old.record(key);
        }
        old.normalize();
        assert_eq!(old.runs, [
            RowRun { list: 3, start: 1, end: 4 },
            RowRun { list: 9, start: 5, end: 7 },
            RowRun { list: 9, start: 8, end: 9 },
        ]);
        let mut new = RowRuns::default();
        for key in [(9, 6), (3, 2), (9, 7), (9, 8)] {
            new.record(key);
        }
        new.normalize();
        let mut retired = Vec::new();
        old.visit_retired(&new, |run| retired.extend((run.start..run.end).map(|row| (run.list, row))));
        assert_eq!(retired, [(3, 1), (3, 3), (9, 5)]);
        old.visit_retired(&old, |_| panic!("identical membership has no retirements"));
    }

    #[test]
    fn state_identity_routes_share_edges_across_versions_and_retire() {
        let state = crate::StateVersion::default();
        let other = crate::StateVersion::placement();
        let a = (1, 0);
        let b = (1, 1);
        let first = RenderDependencies {
            states: Rc::from([StateRead::new(&state), StateRead::new(&other)]),
            ..Default::default()
        };
        let second = RenderDependencies {
            states: Rc::from([StateRead::new(&state)]),
            ..Default::default()
        };
        let mut routing = RowNotificationRouting::default();
        routing.update(a, None, Some(&first));
        routing.update(b, None, Some(&second));
        let cursor = StateMutationCursor::current();
        state.bump();
        cursor.visit_since(16, |identity| routing.mark_state(identity)).unwrap();
        assert_eq!(routing.state_dirty, FxHashSet::from_iter([a, b]));
        let refreshed = RenderDependencies {
            states: Rc::from([StateRead::new(&other), StateRead::new(&state)]),
            ..Default::default()
        };
        routing.update(a, Some(&first), Some(&refreshed));
        assert_eq!(routing.edges[&RowDependencyKey::State(first.states[0].id())].len(), 2);
        routing.update(b, Some(&second), None);
        routing.state_dirty.clear();
        let cursor = StateMutationCursor::current();
        drop(other);
        cursor.visit_since(16, |identity| routing.mark_state(identity)).unwrap();
        assert_eq!(routing.state_dirty, FxHashSet::from_iter([a]));
        routing.update(a, Some(&refreshed), None);
        assert!(routing.edges.is_empty());
    }

    #[test]
    fn shared_row_readers_promote_and_retire_without_duplicates() {
        let mut readers = RowReaders::default();
        for index in 0..64 {
            readers.insert((1, index));
            readers.insert((1, index));
        }
        assert!(matches!(&readers, RowReaders::Many(_)));
        assert_eq!(readers.len(), 64);
        for index in 0..62 {
            readers.remove((1, index));
        }
        assert!(matches!(&readers, RowReaders::Small(_)));
        let mut remaining = Vec::new();
        readers.visit(|row| remaining.push(row));
        remaining.sort_unstable();
        assert_eq!(remaining, [(1, 62), (1, 63)]);
        readers.remove((1, 62));
        readers.remove((1, 63));
        assert!(readers.is_empty());
    }

    #[test]
    fn row_routes_preserve_common_edges_and_observation_order() {
        let model = EntityId::from(1);
        let other = EntityId::from(2);
        let global = TypeId::of::<u32>();
        let a = (10, 0);
        let b = (10, 1);
        let c = (10, 2);
        let reads = |entities, globals| RenderDependencies {
            entities, globals, ..Default::default()
        };
        let first = reads(Rc::from([model]), Rc::from([(global, Some(true))]));
        let second = reads(Rc::from([model]), Rc::from([(global, None)]));
        let third = reads(Rc::from([other]), Rc::from([(global, Some(false))]));
        let mut routing = RowNotificationRouting::default();
        routing.update(a, None, Some(&first));
        routing.update(b, None, Some(&second));
        routing.update(c, None, Some(&third));
        let epoch = |row| Some(if row == b { 12 } else { 10 });
        routing.route(RowNotification::Entity(model), 11, 12, epoch);
        assert_eq!(routing.dirty, FxHashSet::from_iter([a]));
        routing.route(RowNotification::Global(global, false), 12, 12, epoch);
        assert_eq!(routing.dirty, FxHashSet::from_iter([a]));
        routing.route(RowNotification::Global(global, true), 13, 13, epoch);
        assert_eq!(routing.dirty, FxHashSet::from_iter([a, b, c]));
        routing.update(a, Some(&first), Some(&second));
        assert!(!routing.edges.contains_key(&RowDependencyKey::Global(global, Some(true))));
        assert_eq!(routing.edges[&RowDependencyKey::Entity(model)].len(), 2);
        routing.update(b, Some(&second), None);
        let mut remaining = Vec::new();
        routing.edges[&RowDependencyKey::Entity(model)].visit(|row| remaining.push(row));
        assert_eq!(remaining, [a]);
        routing.reset_cursor(13);
        routing.route(RowNotification::Entity(model), 14, 14, |_| Some(13));
        assert_eq!(routing.dirty, FxHashSet::from_iter([a]));
        routing.reset_cursor(u64::MAX);
        routing.route(RowNotification::Entity(model), 0, 0, |_| Some(u64::MAX));
        assert_eq!(routing.dirty, FxHashSet::from_iter([a]));
    }

    #[test]
    fn unrelated_sibling_can_reuse_but_dirty_owner_or_ancestor_cannot() {
        let root = EntityId::from(1);
        let left = EntityId::from(2);
        let right = EntityId::from(3);
        let path = [root, right];
        let mut dirty = FxHashSet::from_iter([root, left]);
        let mut direct = FxHashSet::from_iter([left]);
        assert!(clean_owner_path(right, &path, [right, root].into_iter(), &dirty, &direct, false));
        direct.insert(root);
        assert!(!clean_owner_path(right, &path, [right, root].into_iter(), &dirty, &direct, false));
        direct.remove(&root);
        dirty.insert(right);
        assert!(!clean_owner_path(right, &path, [right, root].into_iter(), &dirty, &direct, false));
        direct.insert(right);
        assert!(clean_owner_path(right, &path, [right, root].into_iter(), &dirty, &direct, true));
        direct.insert(root);
        assert!(!clean_owner_path(right, &path, [right, root].into_iter(), &dirty, &direct, true));
        direct.remove(&root);
        direct.remove(&right);
        dirty.remove(&right);
        assert!(!clean_owner_path(right, &path, [right, left, root].into_iter(), &dirty, &direct, false));
        assert!(!clean_owner_path(right, &path, std::iter::empty(), &dirty, &direct, false));
    }
}
