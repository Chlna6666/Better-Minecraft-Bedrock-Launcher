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
use std::{any::TypeId, cell::{Cell, RefCell}, collections::VecDeque, ops::Range, rc::{Rc, Weak}, sync::Arc};
use super::lifecycle::{RowNotification, WindowInvalidator};
use super::scroll_dependencies::{MountedScrollSource, ScrollOffset};
use super::scroll_paint::ScrollPlacement;
use crate::diagnostics::scroll_placement::Outcome;

#[derive(Clone)]
struct RowPlacementBinding {
    owner: crate::EntityId,
    revision: u64,
    source: RowPlacementSource,
    sampled: Point<Pixels>,
    visible: Range<usize>,
    captured: Range<usize>,
    row_revision: Option<RowRevision>,
    proof: Option<Rc<RowPlacementTopology>>,
    builder: Option<UniformRowBuilder>,
}

#[derive(Clone)]
enum RowPlacementSource {
    Uniform {
        offset: Weak<ScrollOffset>,
        item_count: usize,
        item_height: Pixels,
        padded: Bounds<Pixels>,
        padding_top: Pixels,
        min_y: Pixels,
        measurement: Option<Rc<RowMeasurement>>,
    },
    Variable(crate::ListPlacementProof),
}

/// The factory owns captures independently of the short-lived list element.
/// No AnyElement, element arena, or mutable App borrow survives publication.
/// Only the existing buffered independent-row domain can retain a builder.
#[derive(Clone)]
#[allow(dead_code)] // Boundary execution must restore these native scopes.
struct UniformRowBuilder {
    renderer: UniformRowRenderer,
    context: Rc<ListContext>,
    layout_parent: u64,
    dispatch_parent: crate::DispatchNodeId,
    // The factory runs before the list's row-specific mask is pushed.
    // Preserve that scope separately from the clipped row context.
    application_prefix: GlobalElementId,
    owner_path: Vec<crate::EntityId>,
    factory_context: RetainedPaintContext,
    row_mask: crate::ContentMask<Pixels>,
    // Paint inherits the interactivity paint scope, which need not match
    // the factory's prepaint scope. Capture it from the live traversal.
    paint_context: Option<RetainedPaintContext>,
    paint_key: Option<u64>,
}

impl UniformRowBuilder {
    /// The independent path deliberately uses the same materialization
    /// implementation as the ordinary UniformList. The caller must hold
    /// the native speculative claim transaction and remap the parent first.
    #[allow(dead_code)]
    fn materialize(
        &self, window: &mut Window, parent: crate::DispatchNodeId,
        row: usize, visible: &Range<usize>, origin: Point<Pixels>,
        plan: &mut RowPlan, cx: &mut App,
    ) -> Option<AnyElement> {
        if row >= self.context.item_count || plan.key != self.layout_parent
            || !plan.matches_context(&self.context)
        {
            return None;
        }
        self.with_native_scope(window, parent, super::DrawPhase::Prepaint, &self.factory_context, |window| {
            let read_start = cx.entities.dependency_read_index();
            let item = (self.renderer)(row, window, cx);
            let reads = cx.entities.row_reads_since(read_start);
            window.with_content_mask(Some(self.row_mask), |window| {
                window.materialize_uniform_row(
                    item, row, visible, origin, self.context.available_space,
                    &reads, plan, cx,
                )
            })
        }).flatten()
    }

    /// Consume only a newly materialized Element from this speculative
    /// transaction. Keep native scroll capture and dependency recording on
    /// the ordinary paint path; do not invoke retained application code a
    /// second time or paint without its real row prepaint records.
    #[allow(dead_code)]
    fn paint_materialized(
        &self, window: &mut Window, parent: crate::DispatchNodeId,
        row: usize, item: AnyElement, plan: &mut RowPlan, cx: &mut App,
    ) -> Option<()> {
        let context = self.paint_context.as_ref()?;
        if !plan.matches_context(&self.context)
            || plan.prepainted.front().is_none_or(|(index, _)| *index != row)
        {
            return None;
        }
        self.with_native_scope(window, parent, super::DrawPhase::Paint, context, |window| {
            window.paint_planned_scroll_row(Some(plan), row, Some(item), cx);
        })?;
        let record = window.next_frame.scroll_rows.rows.get(&(plan.key, row))?;
        let marker = record.paint.start.scene_index;
        if record.paint.end.scene_index.checked_sub(marker) != Some(1)
            || !matches!(window.next_frame.painter.paint_operations.view(marker),
                Some(crate::scene::PaintOperationRef::DeferredScroll(_)))
        {
            return None;
        }
        Some(())
    }

    /// Restore only native scopes, not an Element or a second layout tree.
    /// The caller owns a live speculative prepaint transaction and has
    /// already relocated the dispatch parent into that transaction.
    /// In particular this must not run against the committed Frame.
    #[allow(dead_code)]
    fn with_native_scope<R>(
        &self, window: &mut Window, parent: crate::DispatchNodeId,
        phase: super::DrawPhase, context: &RetainedPaintContext,
        f: impl FnOnce(&mut Window) -> R,
    ) -> Option<R> {
        if window.invalidator.phase() != phase
            || window.next_frame.dispatch_tree.active_node_id() != Some(parent)
            || window.scale_factor() != context.device_scale_factor
            || self.owner_path.last().copied() != Some(self.context.owner)
            || window.scene_animation.is_some()
        {
            return None;
        }
        // Move the enclosing stacks aside. The captured immutable path
        // and effective values are sufficient; no copied ancestor Elements.
        use std::mem;
        let application = mem::replace(&mut window.element_id_stack,
            crate::element::ElementIdStack::from_prefix(self.application_prefix.clone()));
        let owners = mem::replace(&mut window.rendered_entity_stack, self.owner_path.clone());
        let text = mem::replace(&mut window.text_style_stack,
            crate::fast::text_style::TextStyleStack::with_inherited(&context.text_style));
        let logical = mem::replace(&mut window.content_mask_stack, vec![context.content_mask]);
        let visual = mem::replace(&mut window.visual_content_mask_stack, vec![context.visual_content_mask]);
        let offsets = mem::replace(&mut window.element_offset_stack, vec![context.element_offset]);
        let opacity = mem::replace(&mut window.element_opacity, context.opacity);
        let transform = mem::replace(&mut window.element_visual_transform,
            super::state::ElementVisualTransform { scale: context.scale, translation: context.translation });
        let raster = mem::replace(&mut window.scene_text_raster_scale, context.text_raster_scale);
        let result = window.with_rem_size(Some(context.rem_size), |window| {
            if phase == super::DrawPhase::Prepaint {
                let enclosing = crate::fast::layout_key::enter_prepaint_scope(window, self.layout_parent);
                let result = window.with_parent_layout_key(Some(self.layout_parent), f);
                crate::fast::layout_key::exit_prepaint_scope(window, enclosing);
                result
            } else {
                let previous = mem::replace(&mut window.paint_layout_key, self.paint_key);
                let result = f(window);
                window.paint_layout_key = previous;
                // Drawable::paint activates its own native prepaint node.
                window.next_frame.dispatch_tree.set_active_node(parent);
                result
            }
        });
        window.scene_text_raster_scale = raster;
        window.element_visual_transform = transform;
        window.element_opacity = opacity;
        window.element_offset_stack = offsets;
        window.visual_content_mask_stack = visual;
        window.content_mask_stack = logical;
        window.text_style_stack = text;
        window.rendered_entity_stack = owners;
        window.element_id_stack = application;
        Some(result)
    }
}

/// Fixed CPU record topology is certified once per committed frame.
/// Other position-only transactions preserve these slots and bounds.
struct RowPlacementTopology {
    slots: Vec<usize>,
    damage_limit: Option<Bounds<crate::ScaledPixels>>,
}

pub(crate) fn uniform_visible_range(
    offset: Pixels, item_height: Pixels, viewport_height: Pixels,
    padding_top: Pixels, count: usize,
) -> Range<usize> {
    let top = -offset.to_f64();
    let height = item_height.to_f64();
    let start = ((top - padding_top.to_f64()) / height).floor() as usize;
    let end = ((top + viewport_height.to_f64()) / height).ceil() as usize;
    start..end.min(count)
}

/// The ordinary list build and independent boundary preparation must
/// select exactly the same logical rows for an identical scroll sample.
fn uniform_capture_range(
    visible: &Range<usize>, count: usize, versioned: bool,
    sampled: Point<Pixels>, previous: Option<Point<Pixels>>,
) -> Range<usize> {
    let overscan = (visible.len() / 4).clamp(2, 8);
    let (before, after) = if versioned {
        let forward = (visible.len() / 2).clamp(3, 8);
        match previous.map(|previous| sampled.y.0.total_cmp(&previous.y.0)) {
            Some(std::cmp::Ordering::Less) => (overscan, forward),
            Some(std::cmp::Ordering::Greater) => (forward, overscan),
            _ => (forward, forward),
        }
    } else {
        (overscan, overscan)
    };
    visible.start.saturating_sub(before)..visible.end.saturating_add(after).min(count)
}

/// Pure membership preparation; native Frame ownership is still required
/// before any entering row can be materialized from this builder.
#[allow(dead_code)]
struct UniformBoundaryMembership {
    sampled: Point<Pixels>,
    version: crate::StateVersion,
    visible: Range<usize>,
    captured: Range<usize>,
    delta: VisibleRangeDelta,
}

impl RowPlacementBinding {
    fn measurement(&self) -> Option<&RowMeasurement> {
        match &self.source {
            RowPlacementSource::Uniform { measurement, .. } => measurement.as_deref(),
            RowPlacementSource::Variable(_) => None,
        }
    }

    fn buffered(&self) -> bool {
        self.measurement().is_some()
            || self.row_revision.is_some()
            || matches!(&self.source, RowPlacementSource::Variable(proof) if proof.buffered())
    }

    fn sample_uniform(&self) -> Option<(Point<Pixels>, crate::StateVersion, Range<usize>)> {
        let RowPlacementSource::Uniform { offset, item_count, item_height, padded, padding_top, min_y, .. } =
            &self.source else { return None; };
        let offset = offset.upgrade()?;
        let sampled = *offset.borrow();
        if sampled.x != Pixels::ZERO || !sampled.y.0.is_finite()
            || sampled.y < *min_y || sampled.y > Pixels::ZERO
        {
            return None;
        }
        let visible = uniform_visible_range(
            sampled.y, *item_height, padded.size.height, *padding_top, *item_count,
        );
        (!visible.is_empty()).then(|| (sampled, offset.version.clone(), visible))
    }

    fn sample(&self) -> Option<(Point<Pixels>, crate::StateVersion, Range<usize>)> {
        match &self.source {
            RowPlacementSource::Variable(proof) => proof.sample(),
            RowPlacementSource::Uniform { .. } => {
                let (sampled, version, visible) = self.sample_uniform()?;
                if if self.buffered() {
                    visible.start < self.captured.start || visible.end > self.captured.end
                } else {
                    visible != self.captured
                } {
                    return None;
                }
                Some((sampled, version, visible))
            }
        }
    }

    #[allow(dead_code)] // The final boundary commit will consume the entering runs.
    fn boundary_membership(&self) -> Option<UniformBoundaryMembership> {
        let builder = self.builder.as_ref()?;
        let (sampled, version, visible) = self.sample_uniform()?;
        if !self.buffered() || builder.context.owner != self.owner
            || (visible.start >= self.captured.start && visible.end <= self.captured.end)
        {
            return None;
        }
        let captured = uniform_capture_range(
            &visible, builder.context.item_count, self.row_revision.is_some(), sampled, Some(self.sampled),
        );
        let delta = VisibleRangeDelta::between(&self.captured, &captured);
        Some(UniformBoundaryMembership { sampled, version, visible, captured, delta })
    }

    fn origin(&self, row: usize, offset: Point<Pixels>, previous: Point<Pixels>) -> Point<Pixels> {
        match &self.source {
            RowPlacementSource::Variable(_) => previous + (offset - self.sampled),
            RowPlacementSource::Uniform { item_height, padded, padding_top, .. } =>
                padded.origin + crate::point(offset.x, Pixels::from(
                    item_height.to_f64() * row as f64 + offset.y.to_f64() + padding_top.to_f64()
                )),
        }
    }
}

pub(super) struct RowPlacementCandidate {
    key: u64,
    version: crate::StateVersion,
    pub(super) sample_read: StateRead,
    pub(super) sampled: Point<Pixels>,
    visible: Range<usize>,
    pub(super) owner: crate::EntityId,
    pub(super) mounted_source: usize,
    pub(super) stale_input: bool,
    pub(super) input_moves: SmallVec<[(Range<usize>, Point<Pixels>); 4]>,
    pub(super) scene_span: Range<usize>,
    pub(super) placements: Vec<(usize, ScrollPlacement)>,
    origins: Vec<(RowKey, Point<Pixels>)>,
    proof: Rc<RowPlacementTopology>,
    pub(super) damage: Bounds<crate::ScaledPixels>,
}

impl RowPlacementCandidate {
    pub(super) fn commit(self, frame: &mut super::Frame) {
        for (key, origin) in self.origins {
            frame.scroll_rows.rows.get_mut(&key).expect("validated row").prepaint.origin = origin;
        }
        let binding = frame.scroll_rows.row_placements.get_mut(&self.key)
            .expect("validated row binding");
        binding.sampled = self.sampled;
        binding.visible = self.visible;
        if binding.proof.is_none() {
            for &slot in &self.proof.slots {
                if let Some(record) = frame.view_retention_records[slot].record.as_mut() {
                    record.splice_safe = false;
                }
            }
            binding.proof = Some(self.proof);
        }
        frame.mounted_scroll_sources[self.mounted_source].acknowledge(&self.version);
    }
}

/// Difference of two contiguous visible ranges. Retained hidden rows are
/// not marker replacements; entering/leaving rows change visibility and
/// the intersection changes position. No per-row membership set needed.
#[derive(Debug, PartialEq, Eq)]
struct VisibleRangeDelta {
    kept: Range<usize>,
    entered: [Range<usize>; 2],
    exited: [Range<usize>; 2],
}

impl VisibleRangeDelta {
    fn between(old: &Range<usize>, new: &Range<usize>) -> Self {
        let start = old.start.max(new.start);
        let end = old.end.min(new.end);
        if start >= end {
            return Self { kept: start..start, entered: [new.clone(), 0..0], exited: [old.clone(), 0..0] };
        }
        Self {
            kept: start..end,
            entered: [new.start..start, end..new.end],
            exited: [old.start..start, end..old.end],
        }
    }

    fn changes_marker(&self, row: usize) -> bool {
        self.kept.contains(&row)
            || self.entered.iter().any(|range| range.contains(&row))
            || self.exited.iter().any(|range| range.contains(&row))
    }

    fn marker_count(&self) -> usize {
        self.kept.len() + self.entered.iter().map(Range::len).sum::<usize>()
            + self.exited.iter().map(Range::len).sum::<usize>()
    }
}

/// Adjacent records with the same translation share a single hitbox span
/// update. Reject overlap rather than translate a record twice.
fn append_row_input_move(
    moves: &mut SmallVec<[(Range<usize>, Point<Pixels>); 4]>,
    input: Range<usize>, delta: Point<Pixels>,
) -> Result<(), Outcome> {
    if let Some((previous, previous_delta)) = moves.last_mut() {
        if input.start < previous.end {
            return Err(Outcome::InputRecords);
        }
        if input.start == previous.end && delta == *previous_delta {
            previous.end = input.end;
            return Ok(());
        }
    }
    moves.push((input, delta));
    Ok(())
}

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
    row_placements: FxHashMap<u64, RowPlacementBinding>,
    oldest_notification_epoch: Option<u64>,
    measurements: FxHashMap<GlobalElementId, (u64, Rc<RowMeasurement>)>,
    pub(super) enabled: bool,
    blocked: bool,
}

impl ScrollRows {
    #[cfg(test)]
    pub(super) fn retained_uniform_builder_count(&self) -> usize {
        self.row_placements.values().filter(|binding| binding.builder.is_some()).count()
    }

    #[cfg(test)]
    pub(super) fn uniform_boundary_runs(&self) -> Option<(Range<usize>, Range<usize>, Vec<usize>, Vec<usize>)> {
        let mut candidates = self.row_placements.values().filter_map(RowPlacementBinding::boundary_membership);
        let boundary = candidates.next()?;
        if candidates.next().is_some() { return None; }
        Some((boundary.visible, boundary.captured,
            boundary.delta.entered.into_iter().flatten().collect(),
            boundary.delta.exited.into_iter().flatten().collect()))
    }

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
        self.row_placements.clear();
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
        let binding_target = current.row_placements.len().max(4);
        if self.row_placements.capacity() > binding_target.saturating_mul(4) {
            self.row_placements.shrink_to(binding_target);
        }
        let target = current.measurements.len().max(4);
        if self.measurements.capacity() > target * 4 {
            self.measurements.shrink_to(target);
        }
    }
}

pub(crate) type RowRevision = Rc<dyn Fn(usize) -> u64>;
/// Independent row factory, shared with its committed placement binding.
pub(crate) type UniformRowRenderer = Rc<dyn Fn(usize, &mut Window, &mut App) -> AnyElement>;

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
    row_placement: Option<RowPlacementBinding>,
    buffered_visible: Option<Range<usize>>,
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

pub(crate) struct UniformRowPlacement<'a> {
    pub offset: &'a Rc<ScrollOffset>,
    pub sampled: Point<Pixels>,
    pub item_height: Pixels,
    pub padded: Bounds<Pixels>,
    pub padding_top: Pixels,
    pub min_y: Pixels,
    pub fixed_height: bool,
}

impl RowPlan {
    pub(crate) fn buffer_rows(&mut self, visible: Range<usize>) {
        self.buffered_visible = Some(visible);
    }

    fn paint_target(&self, row: usize) -> ScrollPaintTarget {
        self.buffered_visible.as_ref()
            .map_or(ScrollPaintTarget::Row, |visible| ScrollPaintTarget::BufferedRow {
                visible: visible.contains(&row),
            })
    }

    pub(crate) fn bind_variable_placement(
        &mut self, proof: crate::ListPlacementProof, captured: Range<usize>,
    ) {
        if self.row_revision.is_none() && let Some((sampled, _, visible)) = proof.sample() {
            self.row_placement = Some(RowPlacementBinding {
                owner: self.context.owner, revision: self.revision,
                source: RowPlacementSource::Variable(proof), sampled, visible, captured,
                row_revision: None, proof: None, builder: None,
            });
        }
    }

    pub(crate) fn row_revision(&self, row: usize) -> u64 {
        self.row_revision.as_ref().map_or(0, |revision| revision(row))
    }

    pub(crate) fn has_row_revisions(&self) -> bool {
        self.row_revision.is_some()
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
    /// No nested input clip or visual transform changed during capture.
    pub(crate) fixed_input_clip: bool,
    pub(crate) origin: Point<Pixels>,
    pub(crate) records: Range<PrepaintStateIndex>,
    pub(crate) reads: RenderDependencies,
    pub(crate) validation: StateReadValidation,
    /// Native layout ownership is optional: ordinary paint replay remains
    /// valid even when a row has transient nodes or no detached claim proof.
    pub(crate) layout_claims: Option<(crate::LayoutId, crate::layout::LayoutClaimBlocks)>,
}

struct RowRecord {
    revision: u64,
    notification_epoch: u64,
    context: Rc<ListContext>,
    prepaint: RowPrepaint,
    paint: Range<PaintIndex>,
    content: Arc<ScrollPaint>,
}

/// First runtime boundary domain: static, fixed-height independent rows.
/// The surrounding list may remain interactive. Only the rows being
/// spliced must have no additional input, state, text or nested view
/// records. This deliberately preserves all other Frame stream offsets.
struct StaticUniformBoundary {
    key: u64,
    binding: RowPlacementBinding,
    membership: UniformBoundaryMembership,
    sample_read: StateRead,
    mounted_source: usize,
    scene: Range<usize>,
    dispatch: Range<usize>,
    prepaint_base: PrepaintStateIndex,
    paint_base: PaintIndex,
    retired_claims: Vec<(crate::LayoutId, crate::layout::LayoutClaimBlocks)>,
}

struct BoundaryRowUpdate {
    key: RowKey,
    origin: Point<Pixels>,
    prepaint: Range<PrepaintStateIndex>,
    paint: Range<PaintIndex>,
    fresh_claims: Option<(crate::LayoutId, crate::layout::LayoutClaimBlocks)>,
}

struct BoundaryRowAdoption {
    key: u64,
    binding: RowPlacementBinding,
    updates: Vec<BoundaryRowUpdate>,
    retired_keys: Vec<RowKey>,
    retired: Vec<RowRecord>,
}

impl BoundaryRowAdoption {
    /// Move only entered/retired row owners; kept records remain in the
    /// existing map. Their content and dependency allocation stay intact.
    fn adopt(mut self, old: &mut ScrollRows, replacement: &mut ScrollRows, now: u64) -> Vec<RowRecord> {
        let routing = old.notification_routing.get_mut();
        for key in self.retired_keys {
            let row = old.rows.remove(&key).expect("validated retired row");
            if routing.cursor.is_some() {
                routing.update(key, Some(&row.prepaint.reads), None);
                routing.dirty.remove(&key);
                routing.state_dirty.remove(&key);
            }
            self.retired.push(row);
        }
        for update in self.updates {
            let key = update.key;
            let row = if let Some(claims) = update.fresh_claims {
                let mut row = replacement.rows.remove(&key).expect("validated entering row");
                row.prepaint.layout_claims = Some(claims);
                if routing.cursor.is_some() {
                    routing.update(key, None, Some(&row.prepaint.reads));
                }
                old.rows.insert(key, row);
                old.rows.get_mut(&key).expect("inserted row")
            } else {
                old.rows.get_mut(&key).expect("validated kept row")
            };
            row.prepaint.origin = update.origin;
            row.prepaint.records = update.prepaint;
            row.paint = update.paint;
        }
        let oldest = RowNotificationRouting::oldest_epoch(&old.rows, now);
        if routing.cursor.is_some() {
            routing.reset_cursor(oldest);
        }
        old.oldest_notification_epoch = Some(oldest);
        old.membership.runs.clear();
        old.membership.runs.push(RowRun {
            list: self.key, start: self.binding.captured.start, end: self.binding.captured.end,
        });
        old.changed_rows.clear();
        old.row_placements.insert(self.key, self.binding);
        self.retired
    }
}

impl RowRecord {
    /// Return the unchanged side-stream checkpoints for the initial
    /// detached domain. Paint and prepaint text checkpoints are separate.
    fn static_boundary_checkpoints(&self) -> Option<(PrepaintStateIndex, PaintIndex)> {
        let mut prepaint = self.prepaint.records.clone();
        prepaint.start.dispatch_tree_index = 0;
        prepaint.end.dispatch_tree_index = 0;
        let mut paint = self.paint.clone();
        paint.start.scene_index = 0;
        paint.end.scene_index = 0;
        if prepaint.start != prepaint.end || paint.start != paint.end
            || !self.prepaint.fixed_input_clip
            || self.prepaint.layout_claims.is_none()
            || self.paint.start.scene_index.checked_add(1)? != self.paint.end.scene_index
            || self.prepaint.records.start.dispatch_tree_index >= self.prepaint.records.end.dispatch_tree_index
        {
            return None;
        }
        Some((prepaint.start, paint.start))
    }

    fn dispatch_span(&self) -> Range<usize> {
        self.prepaint.records.start.dispatch_tree_index..self.prepaint.records.end.dispatch_tree_index
    }

    fn dependency_free(&self) -> bool {
        let reads = &self.prepaint.reads;
        reads.entities.is_empty() && reads.globals.is_empty() && reads.states.is_empty()
    }
}

impl Window {
    /// Build and paint exactly the entering rows into the existing scratch
    /// Frame. All unsupported side streams reject the candidate before
    /// any committed CPU/GPU owner can move.
    fn execute_static_uniform_boundary(
        &mut self, boundary: StaticUniformBoundary, epoch: super::lifecycle::PlacementEpoch,
        cx: &mut App,
    ) -> Option<(Arc<super::Paint>, Bounds<crate::ScaledPixels>, super::frame::RetiredBoundaryRecords, Vec<RowRecord>)> {
        use super::frame::{BoundaryIndexSplices, RecordSplicePlan};
        let source_revision = self.rendered_frame.scene().revision;
        let key = boundary.key;
        let builder = boundary.binding.builder.as_ref()?.clone();
        let owner = boundary.binding.owner;
        let membership = &boundary.membership;
        let old_range = boundary.binding.captured.clone();
        let new_range = membership.captured.clone();
        let scale = self.scale_factor();
        let viewport = builder.context.inherited.visual_content_mask;
        let opaque = self.platform_window.background_appearance() == crate::WindowBackgroundAppearance::Opaque;
        let text_start = self.text_system.layout_index();
        let read_start = cx.entities.dependency_read_index();

        let mut binding = boundary.binding.clone();
        binding.sampled = membership.sampled;
        binding.visible = membership.visible.clone();
        binding.captured = new_range.clone();
        binding.proof = None;
        let mut plan = RowPlan {
            key, revision: epoch.content, notification_epoch: self.invalidator.row_notification_epoch(),
            placement_only: true, allow_reuse: false,
            row_placement: Some(binding.clone()), buffered_visible: Some(membership.visible.clone()),
            row_revision: binding.row_revision.clone(), context: builder.context.clone(),
            previous_context: RefCell::new(None), hover_held: true, prepainted: VecDeque::new(),
        };
        self.invalidator.set_phase(super::DrawPhase::Prepaint);
        let anchor = self.next_frame.dispatch_tree.push_node();
        let mut items = Vec::new();
        for row in new_range.clone().filter(|row| !old_range.contains(row)) {
            let origin = binding.origin(row, membership.sampled, Point::default());
            let item = builder.materialize(self, anchor, row, &membership.visible, origin, &mut plan, cx)?;
            items.push((row, item));
        }
        self.invalidator.set_phase(super::DrawPhase::Paint);
        for (row, item) in items {
            builder.paint_materialized(self, anchor, row, item, &mut plan, cx)?;
        }
        if !plan.prepainted.is_empty() || self.next_frame.dispatch_tree.active_node_id() != Some(anchor) {
            return None;
        }
        self.next_frame.dispatch_tree.pop_node();
        self.invalidator.set_phase(super::DrawPhase::None);

        // Do not extend the App's committed owner dependency graph from an
        // incomplete traversal. The first production experiment admits
        // only rows without new entity/global/state dependencies.
        let reads = cx.entities.dependency_reads_since(read_start);
        if !reads.entities.is_empty() || !reads.globals.is_empty() || !reads.states.is_empty()
            || !cx.entities.frame_entity_scratch.is_empty()
            || self.text_system.layout_index() != text_start
            || self.rendered_frame.scene().revision != source_revision
            || self.draw_was_degraded
            || self.next_frame.accessed_element_states.len() != 0
            || !self.next_frame.hitboxes.is_empty()
            || !self.next_frame.mouse_listeners.is_empty()
            || !self.next_frame.mounted_scroll_sources.is_empty()
            || !self.next_frame.input_handlers.is_empty()
            || !self.next_frame.cursor_styles.is_empty()
            || !self.next_frame.window_control_hitboxes.is_empty()
            || !self.next_frame.tooltip_requests.is_empty()
            || !self.next_frame.deferred_draws.is_empty()
            || !self.next_frame.layer_inputs.is_empty()
            || self.next_frame.tab_stops.paint_index() != 0
            || self.next_frame.focus.is_some()
            || !self.next_frame.view_retention_records.is_empty()
            || !self.next_frame.retained_scene_segments.is_empty()
            || !self.next_frame.scroll_rows.row_placements.is_empty()
            || !self.next_input_readers.borrow().is_empty()
        {
            return None;
        }
        #[cfg(any(test, feature = "test-support"))]
        if !self.next_frame.debug_bounds_records.is_empty() { return None; }
        let old = &self.rendered_frame;
        let fresh = &self.next_frame;
        let entered_count = new_range.clone().filter(|row| !old_range.contains(row)).count();
        if fresh.scroll_rows.rows.len() != entered_count
            || fresh.painter.len() != entered_count
            || !fresh.dispatch_tree.inert_row_range(0..fresh.dispatch_tree.len())
            || boundary.prepaint_base.line_layout_index.rebased_from(
                &boundary.prepaint_base.line_layout_index, &text_start).is_none()
            || boundary.paint_base.line_layout_index.rebased_from(
                &boundary.paint_base.line_layout_index, &text_start).is_none()
        {
            return None;
        }

        // Entering rows are materialized in logical order. Their native
        // dispatch records must be contiguous beneath the temporary anchor.
        let mut fresh_dispatch_end = anchor.0.checked_add(1)?;
        for (marker, row) in new_range.clone().filter(|row| !old_range.contains(row)).enumerate() {
            let record = fresh.scroll_rows.rows.get(&(key, row))?;
            record.static_boundary_checkpoints()?;
            let span = record.dispatch_span();
            if !record.dependency_free() || record.paint.start.scene_index != marker
                || span.start != fresh_dispatch_end
                || record.prepaint.origin != binding.origin(row, membership.sampled, Point::default())
                || binding.row_revision.as_ref()?(row) != record.prepaint.row_revision
            {
                return None;
            }
            fresh_dispatch_end = span.end;
        }
        if fresh_dispatch_end != fresh.dispatch_tree.len() { return None; }

        // The left and right gaps are independent native stream edits.
        // A jump with no retained intersection replaces the domain once.
        let kept = &membership.delta.kept;
        let mut groups = SmallVec::<[(Range<usize>, Range<usize>); 2]>::new();
        if kept.is_empty() {
            groups.push((old_range.clone(), new_range.clone()));
        } else {
            groups.push((old_range.start..kept.start, new_range.start..kept.start));
            groups.push((kept.end..old_range.end, kept.end..new_range.end));
        }
        let mut scene_edits = Vec::with_capacity(groups.len());
        let mut dispatch_edits = Vec::with_capacity(groups.len());
        let old_dispatch_at = |row: usize| -> Option<usize> {
            if row == old_range.end { Some(boundary.dispatch.end) }
            else { Some(old.scroll_rows.rows.get(&(key, row))?.dispatch_span().start) }
        };
        for (retired, entered) in groups {
            if retired.is_empty() && entered.is_empty() { continue; }
            let start = boundary.scene.start.checked_add(retired.start.checked_sub(old_range.start)?)?;
            let end = start.checked_add(retired.len())?;
            let old_dispatch = old_dispatch_at(retired.start)?..old_dispatch_at(retired.end)?;
            let (new_scene, new_dispatch) = if entered.is_empty() {
                (0..0, 0..0)
            } else {
                let first = fresh.scroll_rows.rows.get(&(key, entered.start))?;
                let last = fresh.scroll_rows.rows.get(&(key, entered.end - 1))?;
                (first.paint.start.scene_index..last.paint.end.scene_index,
                    first.dispatch_span().start..last.dispatch_span().end)
            };
            scene_edits.push((start..end, new_scene));
            dispatch_edits.push((old_dispatch, new_dispatch));
        }

        self.next_frame.painter.finish();
        self.next_frame.dispatch_tree.freeze();
        self.next_frame.hitboxes.freeze();
        self.next_frame.accessed_element_states.freeze();
        let old = &self.rendered_frame;
        let fresh = &self.next_frame;
        let scene = RecordSplicePlan::new(old.scene().len(), fresh.scene().len(), &scene_edits)?;
        let dispatch = RecordSplicePlan::new(old.dispatch_tree.len(), fresh.dispatch_tree.len(), &dispatch_edits)?;
        let hitboxes = RecordSplicePlan::new(old.hitboxes.len(), 0, &[])?;
        let states = RecordSplicePlan::new(old.accessed_element_states.len(), 0, &[])?;
        let mouse = RecordSplicePlan::new(old.mouse_listeners.len(), 0, &[])?;
        let ime = RecordSplicePlan::new(old.input_handlers.len(), 0, &[])?;
        let maps = BoundaryIndexSplices {
            scene: &scene, dispatch: &dispatch, hitboxes: &hitboxes, states: &states,
            mouse: &mouse, ime: &ime, text: None,
        };
        let mut candidate = old.prepare_boundary_records_under(
            fresh, &scene_edits, &hitboxes, &states, &dispatch, builder.dispatch_parent,
            Some(anchor), &mouse, &ime, opaque,
        )?;

        let mut damage: Option<Bounds<crate::ScaledPixels>> = None;
        let mut include_damage = |bounds: Bounds<crate::ScaledPixels>| {
            if !bounds.is_empty() { damage = Some(damage.map_or(bounds, |old| old.union(&bounds))); }
        };
        for marker in boundary.scene.clone() {
            let Some(crate::scene::PaintOperationRef::DeferredScroll(row)) = old.scene().paint_operations.view(marker)
            else { return None; };
            include_damage(row.visual_bounds());
        }
        let mut updates = Vec::with_capacity(new_range.len());
        let mut placements = Vec::new();
        for row in new_range.clone() {
            let row_key = (key, row);
            let entering = !old_range.contains(&row);
            let record = if entering { fresh.scroll_rows.rows.get(&row_key)? }
                else { old.scroll_rows.rows.get(&row_key)? };
            let source_scene = record.paint.start.scene_index..record.paint.end.scene_index;
            let source_dispatch = record.dispatch_span();
            let (marker, node_span) = if entering {
                (scene.replacement_span(source_scene)?.start, dispatch.replacement_span(source_dispatch)?)
            } else {
                (scene.retained_range(source_scene)?.start, dispatch.retained_range(source_dispatch)?)
            };
            let origin = binding.origin(row, membership.sampled, record.prepaint.origin);
            let delta = record.content.translation(origin, scale)?;
            let placement = ScrollPaintTarget::BufferedRow { visible: membership.visible.contains(&row) }
                .placement(&record.content, viewport, scale, delta)?;
            let Some(crate::scene::PaintOperationRef::DeferredScroll(previous)) =
                candidate.paint.scene.paint_operations.view(marker) else { return None; };
            include_damage(placement.visual_bounds());
            if !placement.same_paint(previous) {
                if entering { return None; }
                placements.push((marker, placement));
            }
            let fresh_claims = if entering {
                let (root, local) = record.prepaint.layout_claims.as_ref()?;
                Some((*root, self.layout_engine.as_ref()?.detached_boundary_row_claims(*root, local)?))
            } else { None };
            let mut start = boundary.prepaint_base.clone();
            let mut end = start.clone();
            start.dispatch_tree_index = node_span.start;
            end.dispatch_tree_index = node_span.end;
            let mut paint_start = boundary.paint_base.clone();
            let mut paint_end = paint_start.clone();
            paint_start.scene_index = marker;
            paint_end.scene_index = marker.checked_add(1)?;
            updates.push(BoundaryRowUpdate { key: row_key, origin, prepaint: start..end,
                paint: paint_start..paint_end, fresh_claims });
        }
        let damage = damage?;
        if !placements.is_empty() {
            candidate.paint = candidate.paint.with_scroll_placements(&placements, opaque)?;
        }
        candidate.prepare_view_records(old, fresh, &maps, self.layout_engine.as_ref()?, owner, damage)?;
        let parent_index = builder.dispatch_parent.0;
        binding.builder.as_mut()?.dispatch_parent = crate::DispatchNodeId(
            dispatch.retained_range(parent_index..parent_index.checked_add(1)?)?.start);
        let retired_keys: Vec<_> = old_range.clone().filter(|row| !new_range.contains(row))
            .map(|row| (key, row)).collect();
        let retired = Vec::with_capacity(retired_keys.len());
        let rows = BoundaryRowAdoption { key, binding, updates, retired_keys, retired };

        // All fallible work completes before consuming the placement
        // epoch. The native owners cannot change between these checks
        // and the swaps on the exclusive UI thread.
        self.rendered_frame.scroll_rows.rows.try_reserve(entered_count).ok()?;
        if !self.global_ids.borrow().can_finish_partial_frame()
            || boundary.sample_read.status() != crate::StateReadStatus::Unchanged
            || self.rendered_frame.scene().revision != source_revision
            || self.focus != self.rendered_frame.focus
            || self.viewport_epoch != self.committed_epoch
            || self.scale_factor != self.committed_scale_factor
            || self.force_full_redraw.get() || self.force_view_cache_refresh
        { return None; }
        self.layout_engine.as_mut()?.prepare_detached_boundary_commit()?;
        if !self.invalidator.commit_placement(epoch, &[owner]) { return None; }
        self.layout_engine.as_mut().expect("validated layout engine").commit_detached_boundary()
            .expect("reserved detached layout commit");
        let paint = candidate.paint.clone();
        let retired_records = candidate.adopt(&mut self.rendered_frame, &mut self.next_frame)
            .expect("validated exclusive Frame adoption");
        let retired_rows = rows.adopt(&mut self.rendered_frame.scroll_rows,
            &mut self.next_frame.scroll_rows, self.invalidator.row_notification_epoch());
        self.rendered_frame.mounted_scroll_sources[boundary.mounted_source].acknowledge(&membership.version);
        self.mounted_scroll_routing.get_mut().reset_frame(self.rendered_frame.mounted_scroll_sources.len());
        self.global_ids.get_mut().finish_partial_frame();
        Some((paint, damage, retired_records, retired_rows))
    }

    /// This runs only after the common position-transaction admission.
    /// Do not execute any application callback until native ownership of
    /// the old domain and all retired rows has been certified.
    fn prepare_static_uniform_boundary(
        &self, cx: &App, epoch: super::lifecycle::PlacementEpoch, owner: crate::EntityId,
    ) -> Option<StaticUniformBoundary> {
        let frame = &self.rendered_frame;
        let rows = &frame.scroll_rows;
        if rows.row_placements.len() != 1 || !self.scroll_hover_deadline_active(cx) {
            return None;
        }
        let (&key, binding) = rows.row_placements.iter().next()?;
        let builder = binding.builder.as_ref()?;
        if binding.owner != owner || binding.revision != epoch.content
            || binding.row_revision.is_none() || builder.paint_context.is_none()
            || !matches!(&binding.source, RowPlacementSource::Uniform { measurement: None, .. })
            || rows.rows.len() != binding.captured.len()
            || rows.membership.runs.as_slice() != [RowRun {
                list: key, start: binding.captured.start, end: binding.captured.end,
            }]
        {
            return None;
        }
        let membership = binding.boundary_membership()?;
        let sample_read = StateRead::new(&membership.version);
        let mut changed = frame.mounted_scroll_sources.iter().enumerate()
            .filter(|(_, source)| !source.unchanged());
        let (mounted_source, source) = changed.next()?;
        if changed.next().is_some() || !source.changed_for(owner, &membership.version) {
            return None;
        }
        let first = rows.rows.get(&(key, binding.captured.start))?;
        let (prepaint_base, paint_base) = first.static_boundary_checkpoints()?;
        let scene_start = first.paint.start.scene_index;
        let dispatch_start = first.dispatch_span().start;
        if builder.dispatch_parent.0 >= dispatch_start { return None; }
        let scale = self.scale_factor();
        let viewport = builder.context.inherited.visual_content_mask;
        let mut dispatch_end = dispatch_start;
        let mut retired_claims = Vec::new();
        for (index, row) in binding.captured.clone().enumerate() {
            let record = rows.rows.get(&(key, row))?;
            let marker = scene_start.checked_add(index)?;
            let span = record.dispatch_span();
            if record.static_boundary_checkpoints()? != (prepaint_base.clone(), paint_base.clone())
                || !Rc::ptr_eq(&record.context, &builder.context) && *record.context != *builder.context
                || record.revision != epoch.content
                || binding.row_revision.as_ref()?(row) != record.prepaint.row_revision
                || record.prepaint.origin != binding.origin(row, binding.sampled, record.prepaint.origin)
                || !record.dependency_free()
                || record.paint.start.scene_index != marker
                || span.start != dispatch_end
                || !frame.dispatch_tree.inert_row_range(span.clone())
            {
                return None;
            }
            let Some(crate::scene::PaintOperationRef::DeferredScroll(old)) = frame.scene().paint_operations.view(marker)
            else { return None; };
            let delta = record.content.translation(record.prepaint.origin, scale)?;
            let expected = ScrollPaintTarget::BufferedRow { visible: binding.visible.contains(&row) }
                .placement(&record.content, viewport, scale, delta)?;
            if !old.same_paint(&expected) { return None; }
            dispatch_end = span.end;
            if !membership.captured.contains(&row) {
                retired_claims.push(record.prepaint.layout_claims.as_ref()?.clone());
            }
        }
        Some(StaticUniformBoundary {
            key, binding: binding.clone(), membership, sample_read, mounted_source,
            scene: scene_start..scene_start.checked_add(binding.captured.len())?,
            dispatch: dispatch_start..dispatch_end, prepaint_base, paint_base, retired_claims,
        })
    }
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
    pub(crate) fn can_buffer_scroll_rows(&self, plan: &RowPlan, visible: &Range<usize>) -> bool {
        let viewport = plan.context.inherited.visual_content_mask;
        if !plan.placement_only || !plan.hover_held
            || !self.platform_window.supports_deferred_scroll()
        {
            return false;
        }
        let scale = self.scale_factor();
        // Begin buffering only after the list has produced reusable paint.
        visible.clone().any(|row| {
            self.rendered_frame.scroll_rows.rows.get(&(plan.key, row))
                .filter(|record| plan.matches_context(&record.context))
                .is_some_and(|record| record.content.supports_buffered_row(viewport, scale))
        })
    }

    pub(crate) fn bind_uniform_row_placement(
        &self,
        plan: &mut RowPlan,
        geometry: UniformRowPlacement<'_>,
        visible: Range<usize>,
    ) -> Range<usize> {
        let UniformRowPlacement { offset, sampled, item_height, padded, padding_top, min_y, fixed_height } =
            geometry;
        if visible.is_empty() || sampled.x != Pixels::ZERO
            || !item_height.0.is_finite() || item_height <= Pixels::ZERO
            || !min_y.0.is_finite() || !sampled.y.0.is_finite()
            || sampled.y < min_y || sampled.y > Pixels::ZERO
        {
            return visible;
        }
        let mut captured = visible.clone();
        let measurement = if fixed_height {
            None
        } else {
            let RowListIdentity::Uniform(id, size) = &plan.context.identity else { return visible; };
            let Some((_, measure)) = self.next_frame.scroll_rows.measurements.get(id)
                .filter(|(revision, measure)| *revision == plan.revision && measure.size == *size)
            else { return visible; };
            Some(Rc::clone(measure))
        };
        if measurement.is_some() || plan.row_revision.is_some() {
            if !self.can_buffer_scroll_rows(plan, &visible) { return visible; }
            let previous = self.rendered_frame.scroll_rows.row_placements.get(&plan.key);
            captured = uniform_capture_range(
                &visible, plan.context.item_count, plan.row_revision.is_some(),
                sampled, previous.map(|previous| previous.sampled),
            );
            plan.buffer_rows(visible.clone());
        }
        plan.row_placement = Some(RowPlacementBinding {
            owner: plan.context.owner,
            revision: plan.revision,
            source: RowPlacementSource::Uniform {
                offset: Rc::downgrade(offset),
                item_count: plan.context.item_count,
                item_height,
                padded,
                padding_top,
                min_y,
                measurement,
            },
            sampled,
            visible,
            captured: captured.clone(),
            row_revision: plan.row_revision.clone(),
            proof: None,
            builder: None,
        });
        captured
    }

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
            row_placement: None,
            buffered_visible: None,
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

    /// Capture the factory in the actual list prepaint scope, not when the
    /// element is constructed. The same native owner/revision and inherited
    /// context gates that admitted buffering constrain future execution.
    pub(crate) fn retain_uniform_row_builder(
        &self, plan: &mut RowPlan, renderer: &UniformRowRenderer,
        row_mask: crate::ContentMask<Pixels>,
    ) {
        let Some(binding) = plan.row_placement.as_mut() else { return; };
        if !binding.buffered() || !plan.allow_reuse || !plan.placement_only {
            return;
        }
        let Some(layout_parent) = crate::fast::layout_key::layout_key(self) else { return; };
        let Some(dispatch_parent) = self.next_frame.dispatch_tree.active_node_id() else { return; };
        if layout_parent != plan.key { return; }
        binding.builder = Some(UniformRowBuilder {
            renderer: Rc::clone(renderer), context: Rc::clone(&plan.context),
            layout_parent, dispatch_parent,
            application_prefix: self.element_id_stack.global_id(),
            owner_path: self.rendered_entity_stack.clone(),
            factory_context: self.current_retained_paint_context(),
            row_mask,
            paint_context: None,
            paint_key: None,
        });
    }

    pub(crate) fn remember_row_placement(&mut self, plan: &RowPlan, cx: &App) {
        if plan.buffered_visible.is_some() {
            self.restore_scroll_row_after_wheel(cx);
        }
        if let Some(binding) = &plan.row_placement {
            let mut binding = binding.clone();
            if let Some(builder) = binding.builder.as_mut() {
                if self.element_id_stack.global_id() == builder.application_prefix
                    && self.rendered_entity_stack == builder.owner_path
                    && self.scene_animation.is_none()
                {
                    builder.paint_context = Some(self.current_retained_paint_context());
                    builder.paint_key = self.paint_layout_key;
                } else {
                    binding.builder = None;
                }
            }
            // A factory is not sufficient proof of the committed domain.
            // Do not retain captures if any row failed native capture.
            if binding.builder.is_some() && !binding.captured.clone().all(|row| {
                self.next_frame.scroll_rows.rows.get(&(plan.key, row)).is_some_and(|record| {
                    record.paint.end.scene_index.checked_sub(record.paint.start.scene_index) == Some(1)
                        && matches!(self.next_frame.painter.paint_operations.view(record.paint.start.scene_index),
                            Some(crate::scene::PaintOperationRef::DeferredScroll(_)))
                })
            }) {
                binding.builder = None;
            }
            self.next_frame.scroll_rows.row_placements.insert(plan.key, binding);
        }
    }

    /// Shared native row build/measure/prepaint path. The caller supplies
    /// the original list scope and row-specific clip. No frame-local
    /// Element is retained across a publication.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn materialize_uniform_row(
        &mut self, mut item: AnyElement, row: usize, visible: &Range<usize>,
        origin: Point<Pixels>, available_space: Size<AvailableSpace>,
        build_reads: &RenderDependencies, plan: &mut RowPlan, cx: &mut App,
    ) -> Option<AnyElement> {
        let capture = plan.can_capture(&mut item);
        if !capture && !visible.contains(&row) {
            return None;
        }
        let before = capture.then(|| (
            self.prepaint_index(), cx.entities.dependency_read_index(),
            self.next_frame.input_clip_epoch,
        ));
        // Capture native ownership before buffering first becomes eligible.
        // Otherwise a row replayed from the initial visible-only frame
        // can never acquire a detached claim proof.
        let claim_start = (capture && matches!(&plan.context.identity, RowListIdentity::Uniform(..)))
            .then(|| self.layout_engine.as_ref().map(|engine| (engine.claim_log_len(), engine.transient_count())))
            .flatten();
        self.with_list_item_layout_key(row, |window| {
            item.layout_as_root(available_space, window, cx);
            item.prepaint_at(origin, window, cx);
        });
        let layout_claims = claim_start.and_then(|(start, transients)| {
            let engine = self.layout_engine.as_ref()?;
            if engine.transient_count() != transients { return None; }
            engine.capture_detached_row_range(start..engine.claim_log_len())
        });
        if let Some((start, read_start, clip_epoch)) = before {
            let extra = cx.entities.row_reads_since(read_start);
            let mut reads = build_reads.clone();
            reads.merge_replayed(&extra);
            plan.record_prepaint(row, RowPrepaint {
                row_revision: plan.row_revision(row),
                fixed_input_clip: clip_epoch == self.next_frame.input_clip_epoch,
                origin,
                records: start..self.prepaint_index(),
                reads,
                validation: Default::default(),
                layout_claims,
            });
        }
        Some(item)
    }

    /// Stable-membership measured lists use the same atomic publication
    /// as full scrolling views. Fixed-clip hitboxes can move in place, but
    /// arbitrary coordinate-capturing listeners and independently retained
    /// descendants still require normal row replay.
    pub(super) fn prepare_row_placement(
        &self, cx: &App, epoch: super::lifecycle::PlacementEpoch, owner: crate::EntityId,
        changed: &[(usize, &MountedScrollSource)], paint: &super::Paint,
    ) -> Result<RowPlacementCandidate, Outcome> {
        let frame = &self.rendered_frame;
        let rows = &frame.scroll_rows;
        let mut bindings = rows.row_placements.iter().filter_map(|(&key, binding)| {
            if binding.owner != owner { return None; }
            let (sampled, version, visible) = binding.sample()?;
            (sampled != binding.sampled).then_some((key, binding, version, sampled, visible))
        });
        let (key, binding, version, sampled, visible) = bindings.next().ok_or(Outcome::MissingCapture)?;
        if bindings.next().is_some() { return Err(Outcome::MultipleCaptures); }
        if binding.revision != epoch.content { return Err(Outcome::CaptureDependencies); }
        let mut matching = changed.iter().filter(|(_, source)| source.changed_for(owner, &version));
        let (mounted_source, _) = matching.next().copied().ok_or(Outcome::MountedSources)?;
        if matching.next().is_some() { return Err(Outcome::MountedSources); }
        let hover_held = self.scroll_hover_deadline_active(cx);
        let buffered = binding.buffered();
        if buffered && !hover_held {
            return Err(Outcome::Hover);
        }
        if let Some(measurement) = binding.measurement() {
            if !self.invalidator.row_dependencies_unchanged(measurement.notification_epoch.get(), &measurement.reads)
                || !measurement.validation.unchanged(&measurement.reads.states)
            {
                return Err(Outcome::CaptureDependencies);
            }
        }
        let visible_delta = VisibleRangeDelta::between(&binding.visible, &visible);
        let mut placements = Vec::with_capacity(visible_delta.marker_count());
        let mut origins = Vec::with_capacity(binding.captured.len());
        let mut damage: Option<Bounds<crate::ScaledPixels>> = None;
        let mut input_moves = SmallVec::<[(Range<usize>, Point<Pixels>); 4]>::new();
        let mut stale_input = false;
        let scale = self.scale_factor();
        let mut routing = rows.notification_routing.borrow_mut();
        // Topology covers the entire captured range, not just the markers
        // touched this frame. Later transactions can reveal hidden rows.
        let mut first = usize::MAX;
        let mut end = 0;
        let mut previous_marker = None;
        for row in binding.captured.clone() {
            let row_key = (key, row);
            let record = rows.rows.get(&row_key).ok_or(Outcome::MissingCapture)?;
            if record.revision != epoch.content || record.context.owner != owner
                || binding.row_revision.as_ref().is_some_and(|revision| {
                    revision(row) != record.prepaint.row_revision
                })
                || matches!(&record.context.identity, RowListIdentity::Variable(read)
                    if read.status() != crate::StateReadStatus::Unchanged)
                || record.prepaint.origin != binding.origin(row, binding.sampled, record.prepaint.origin)
                || !routing.unchanged(&rows.rows, row_key, record, &self.invalidator)
                || !routing.state_unchanged(&rows.rows, row_key, record, &self.invalidator)
            {
                return Err(Outcome::CaptureDependencies);
            }
            let pre = &record.prepaint.records;
            let span = &record.paint;
            let has_mouse_listeners = span.start.mouse_listeners_index != span.end.mouse_listeners_index;
            if pre.start.tooltips_index != pre.end.tooltips_index
                || pre.start.deferred_draws_index != pre.end.deferred_draws_index
                || (has_mouse_listeners && !buffered)
                || span.start.input_handlers_index != span.end.input_handlers_index
                || span.start.window_control_hitboxes_index != span.end.window_control_hitboxes_index
                || span.start.cursor_styles_index != span.end.cursor_styles_index
                || span.start.tab_handle_index != span.end.tab_handle_index
            {
                return Err(Outcome::InputRecords);
            }
            #[cfg(any(test, feature = "test-support"))]
            if span.start.debug_bounds_index != span.end.debug_bounds_index {
                return Err(Outcome::InputRecords);
            }
            let marker = span.start.scene_index;
            if span.end.scene_index != marker + 1 { return Err(Outcome::Marker); }
            if previous_marker.is_some_and(|previous| previous >= marker) {
                return Err(Outcome::MultipleCaptures);
            }
            previous_marker = Some(marker);
            first = first.min(marker);
            end = end.max(marker + 1);
            let Some(crate::scene::PaintOperationRef::DeferredScroll(old)) =
                paint.scene.paint_operations.view(marker) else { return Err(Outcome::Marker); };
            let viewport = record.context.inherited.visual_content_mask;
            let old_delta = record.content.translation(record.prepaint.origin, scale)
                .ok_or(Outcome::PreviousTranslation)?;
            let old_target = if buffered {
                ScrollPaintTarget::BufferedRow { visible: binding.visible.contains(&row) }
            } else {
                ScrollPaintTarget::Row
            };
            let expected = old_target.placement(&record.content, viewport, scale, old_delta)
                .ok_or(Outcome::PreviousPlacement)?;
            if !old.same_paint(&expected) { return Err(Outcome::PreviousMarker); }
            let origin = binding.origin(row, sampled, record.prepaint.origin);
            let logical_delta = origin - record.prepaint.origin;
            let input = hitbox_range(&record.prepaint);
            if !input.is_empty() {
                if !record.prepaint.fixed_input_clip
                    || !frame.hitboxes.contains(&input)
                    || frame.hitboxes.slices(input.clone()).any(|(slice, _)| {
                        slice.iter().any(|hitbox| hitbox.content_mask != viewport)
                    })
                {
                    return Err(Outcome::InputRecords);
                }
                if !hover_held && (self.hits_scroll_row(&frame.hitboxes, input.clone(), Point::default())
                    || self.hits_scroll_row(&frame.hitboxes, input.clone(), logical_delta))
                {
                    return Err(Outcome::Hover);
                }
                stale_input |= hover_held;
                append_row_input_move(&mut input_moves, input, logical_delta)?;
            }
            let delta = record.content.translation(origin, scale).ok_or(Outcome::FractionalTranslation)?;
            stale_input |= has_mouse_listeners;
            origins.push((row_key, origin));
            if buffered && !visible_delta.changes_marker(row) {
                // Both old and new markers are hidden. Their immutable
                // canonical page remains valid; do not clone a placement.
                continue;
            }
            let new_target = if buffered {
                ScrollPaintTarget::BufferedRow { visible: visible.contains(&row) }
            } else {
                ScrollPaintTarget::Row
            };
            let placement = new_target.placement(&record.content, viewport, scale, delta)
                .ok_or(Outcome::PlacementGeometry)?;
            if placement.same_paint(old) { return Err(Outcome::FractionalTranslation); }
            for bounds in [old.visual_bounds(), placement.visual_bounds()] {
                if !bounds.is_empty() {
                    damage = Some(damage.map_or(bounds, |current| current.union(&bounds)));
                }
            }
            placements.push((marker, placement));
        }
        drop(routing);
        if placements.is_empty() { return Err(Outcome::MissingCapture); }
        let damage = damage.ok_or(Outcome::Damage)?;
        // Retained view records must be fixed enclosing views. Do not
        // translate nested view/layout records or widen existing fragment
        // bounds in this initial transaction.
        let proof = if let Some(proof) = &binding.proof {
            Rc::clone(proof)
        } else {
            let mut slots = Vec::new();
            let ancestors = frame.dispatch_tree.view_path(owner);
            for (slot, entry) in frame.view_retention_records.iter().enumerate() {
                let Some(record) = &entry.record else { continue; };
                let start = record.paint_range.start.scene_index;
                let stop = record.paint_range.end.scene_index;
                let ancestor = entry.owner == owner || ancestors.contains(&entry.owner);
                if !ancestor && frame.dispatch_tree.view_path_reversed(entry.owner).any(|id| id == owner) {
                    return Err(Outcome::NestedRetention);
                }
                if start < end && first < stop {
                    if !ancestor || start > first || stop < end {
                        return Err(Outcome::NestedRetention);
                    }
                    slots.push(slot);
                }
            }
            let mut damage_limit: Option<Bounds<crate::ScaledPixels>> = None;
            for segment in &frame.retained_scene_segments {
                if segment.scene_range.start < end && first < segment.scene_range.end {
                    if segment.scene_range.start > first || segment.scene_range.end < end {
                        return Err(Outcome::Damage);
                    }
                    damage_limit = Some(damage_limit.map_or(segment.bounds, |limit| limit.intersect(&segment.bounds)));
                }
            }
            Rc::new(RowPlacementTopology { slots, damage_limit })
        };
        if proof.damage_limit.is_some_and(|limit| limit.intersect(&damage) != damage) {
            return Err(Outcome::Damage);
        }
        Ok(RowPlacementCandidate {
            key, sample_read: StateRead::new(&version), version, sampled, visible, owner, mounted_source,
            placements, origins, proof, damage, input_moves, stale_input, scene_span: first..end,
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
        let fixed_input_clip = record.prepaint.fixed_input_clip;
        let layout_claims = record.prepaint.layout_claims.as_ref().and_then(|(root, spans)| {
            self.layout_engine.as_mut()?.try_keep_detached_row_claims(spans, *root)
                .map(|kept| (*root, kept))
        });
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
            fixed_input_clip,
            origin,
            records: start..self.prepaint_index(),
            reads,
            validation,
            layout_claims,
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
        let target = plan.paint_target(row);
        if self.next_frame.painter.is_capturing_blur()
            || self.scene_animation.is_some()
            || self.next_frame.hitboxes.slices(hitboxes.clone())
                .any(|(slice, _)| slice.iter().any(|hitbox| hitbox.content_mask != viewport))
            || (!suppress_hover && self.hits_scroll_row(&self.next_frame.hitboxes, hitboxes, Point::default()))
        {
            if !matches!(target, ScrollPaintTarget::BufferedRow { visible: false }) {
                item.paint(self, cx);
            }
            return;
        }
        let read_start = cx.entities.dependency_read_index();
        let start = self.paint_index();
        let previous_source = self.rendered_frame.scroll_rows.rows.get(&(plan.key, row))
            .map(|previous| Arc::clone(&previous.content));
        let content = self.capture_scroll_paint(prepaint.origin, suppress_hover,
            previous_source.as_deref(), target, cx, |window, cx| item.paint(window, cx));
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
        let target = plan.paint_target(row);
        let deferred = (!same_placement || was_deferred || matches!(target, ScrollPaintTarget::BufferedRow { .. }))
            && self.platform_window.supports_deferred_scroll()
            && self.scene_animation.is_none()
            && target.placement(&content, viewport, scale, delta)
                .is_some_and(|placement| self.next_frame.painter.push_scroll_placement_against(
                    self.rendered_frame.scene(), old_range.clone(), placement,
                ));
        if !deferred && !matches!(target, ScrollPaintTarget::BufferedRow { visible: false }) {
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
        if let Some((_, claims)) = prepaint.layout_claims.as_mut() {
            self.layout_engine.as_ref().expect("native layout engine")
                .resolve_claim_blocks_in_place(claims);
        }
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
    fn visible_range_delta_matches_membership_union_and_difference() {
        for a in 0..8 {
            for b in a..8 {
                for c in 0..8 {
                    for d in c..8 {
                        let old = a..b;
                        let new = c..d;
                        let delta = VisibleRangeDelta::between(&old, &new);
                        for row in 0..8 {
                            assert_eq!(delta.kept.contains(&row), old.contains(&row) && new.contains(&row));
                            assert_eq!(delta.entered.iter().any(|r| r.contains(&row)), new.contains(&row) && !old.contains(&row));
                            assert_eq!(delta.exited.iter().any(|r| r.contains(&row)), old.contains(&row) && !new.contains(&row));
                            assert_eq!(delta.changes_marker(row), old.contains(&row) || new.contains(&row));
                        }
                        assert_eq!(delta.marker_count(), (0..8).filter(|&row| old.contains(&row) || new.contains(&row)).count());
                    }
                }
            }
        }
    }

    #[test]
    fn row_input_moves_coalesce_adjacent_spans_but_reject_overlap() {
        let mut moves = SmallVec::new();
        let delta = crate::point(crate::px(0.), crate::px(-2.));
        let other = crate::point(crate::px(0.), crate::px(-3.));
        append_row_input_move(&mut moves, 1..3, delta).unwrap();
        append_row_input_move(&mut moves, 3..5, delta).unwrap();
        append_row_input_move(&mut moves, 7..8, delta).unwrap();
        append_row_input_move(&mut moves, 8..9, other).unwrap();
        assert_eq!(moves.as_slice(), &[(1..5, delta), (7..8, delta), (8..9, other)]);
        assert!(matches!(append_row_input_move(&mut moves, 8..10, other), Err(Outcome::InputRecords)));
    }

    #[test]
    fn independent_boundary_membership_matches_directional_overscan() {
        use crate::{point, px};
        let old = uniform_capture_range(&(10..16), 100, true, point(px(0.), px(-240.)), None);
        assert_eq!(old, 7..19);
        let forward = uniform_capture_range(&(18..24), 100, true,
            point(px(0.), px(-432.)), Some(point(px(0.), px(-240.))));
        assert_eq!(forward, 16..27);
        let delta = VisibleRangeDelta::between(&old, &forward);
        assert_eq!(delta.kept, 16..19);
        assert_eq!(delta.entered, [16..16, 19..27]);
        assert_eq!(delta.exited, [7..16, 19..19]);
        let reverse = uniform_capture_range(&(0..6), 100, true,
            Point::default(), Some(point(px(0.), px(-432.))));
        assert_eq!(reverse, 0..8);
        let jump = VisibleRangeDelta::between(&forward, &reverse);
        assert!(jump.kept.is_empty());
        assert_eq!(jump.entered[0], reverse);
        assert_eq!(jump.exited[0], forward);
        assert_eq!(uniform_capture_range(&(96..100), 100, false, Point::default(), None), 94..100);
    }

    #[test]
    fn uniform_placement_membership_uses_the_normal_materialization_formula() {
        use crate::px;
        let range = |y| uniform_visible_range(px(y), px(24.), px(120.), px(0.), 100);
        assert_eq!(range(0.), 0..5);
        assert_eq!(range(-1.), 0..6);
        assert_eq!(range(-2.), 0..6);
        assert_eq!(range(-23.), 0..6);
        assert_eq!(range(-24.), 1..6);
        assert_eq!(range(-25.), 1..7);
        assert_eq!(uniform_visible_range(px(-100.), px(24.), px(120.), px(4.), 6), 4..6);
    }

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
