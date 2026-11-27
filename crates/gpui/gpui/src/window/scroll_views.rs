//! ScrollPage-style views keep their layout and an overscanned paint source.
//! Adapted from GPUI Fast's scroll-layer paint/reuse phases.

use super::{
    PrepaintStateIndex, RetainedPaintContext, RetainedViewRecord,
    scroll_paint::{ScrollPaint, ScrollPaintTarget},
};
use crate::{
    AnyElement, App, Bounds, ContentMask, EntityId, Pixels, Point, StateRead, StateReadStatus,
    Task, Window,
};
use collections::{FxHashMap, FxHashSet};
use std::{ops::Range, rc::{Rc, Weak}, sync::Arc, time::{Duration, Instant}};
use super::scroll_dependencies::ScrollOffset;
use crate::diagnostics::scroll_placement::{self, Outcome};

const SCROLL_HOVER_DELAY: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(super) struct ScrollHover {
    deadline: Option<Instant>,
    timer: Option<Task<()>>,
    pub(super) painting: bool,
    pub(super) row_owners: FxHashSet<EntityId>,
}

#[derive(Default)]
pub(super) struct ScrollViews {
    viewport: Option<ScrollViewport>,
    views: FxHashMap<u64, ScrollView>,
}

impl ScrollViews {
    pub(super) fn collect_atlas_tile_ids_into(&self, live: &mut FxHashSet<(crate::AtlasTextureId, u32)>) {
        for view in self.views.values() {
            view.source.collect_atlas_tile_ids_into(live);
        }
    }

    pub(super) fn clear(&mut self) {
        self.viewport = None;
        self.views.clear();
    }

    pub(super) fn carry(&mut self, key: u64, previous: &Self) {
        if let Some(source) = previous.views.get(&key) {
            self.views.entry(key).or_insert_with(|| {
                let mut source = source.clone();
                // Whole-view replay has not re-established sole-child
                // ownership under the current scroller.
                source.binding.direct = None;
                source
            });
        }
    }
}

/// A viewport is not a scroll source identity. Two overlapping or
/// replacement scrollers can have exactly the same clip and position.
#[derive(Clone)]
pub(crate) struct ScrollBinding {
    offset: Weak<ScrollOffset>,
    sampled: Point<Pixels>,
    direct: Option<DirectScroll>,
}

#[derive(Clone)]
struct DirectScroll {
    owner: EntityId,
    child: EntityId,
    range: Option<TrackedScrollRange>,
}

/// The exact bounds version sampled after the scroller clamps its offset.
#[derive(Clone)]
pub(crate) struct TrackedScrollRange {
    min_y: Pixels,
    geometry: StateRead,
}

impl TrackedScrollRange {
    pub(crate) fn new(min_y: Pixels, version: &crate::StateVersion) -> Self {
        Self { min_y, geometry: StateRead::new(version) }
    }
}

impl ScrollBinding {
    fn new(offset: Weak<ScrollOffset>, sampled: Point<Pixels>) -> Option<Self> {
        let current = offset.upgrade()?;
        if *current.borrow() != sampled
            || !sampled.x.0.is_finite() || !sampled.y.0.is_finite()
        {
            return None;
        }
        Some(Self { offset, sampled, direct: None })
    }

    fn same_source(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.offset, &other.offset)
    }

    fn delta_from(&self, previous: &Self) -> Option<Point<Pixels>> {
        self.same_source(previous).then_some(self.sampled - previous.sampled)
    }

    fn sample_still_current(&self) -> bool {
        self.offset.upgrade().is_some_and(|offset| *offset.borrow() == self.sampled)
    }
}

struct ScrollViewport {
    clip: ContentMask<Pixels>,
    binding: ScrollBinding,
}

pub(crate) struct ScrollViewCapture {
    key: u64,
    revision: u64,
    viewport: ContentMask<Pixels>,
    binding: ScrollBinding,
    reads: Rc<[StateRead]>,
}

#[derive(Clone)]
struct ScrollView {
    capture_revision: u64,
    owner: EntityId,
    viewport: ContentMask<Pixels>,
    binding: ScrollBinding,
    source: Arc<ScrollPaint>,
    reads: Rc<[StateRead]>,
    stale_input: bool,
}

fn hitboxes(range: &Range<PrepaintStateIndex>) -> Range<usize> {
    range.start.hitboxes_index..range.end.hitboxes_index
}

// An owned candidate contains only the changed position, immutable Paint
// and identifiers into the same UI-owned frame. Preparing it never consumes
// invalidations or edits committed records.
struct PlacementMember {
    marker: usize,
    placement: super::scroll_paint::ScrollPlacement,
    key: u64,
    slot: usize,
    mounted_source: usize,
    offset: Rc<ScrollOffset>,
    sampled: Point<Pixels>,
    delta: Point<Pixels>,
    hitboxes: Range<usize>,
    damage: Bounds<crate::ScaledPixels>,
    stale_input: bool,
}

struct PlacementTransaction {
    epoch: super::lifecycle::PlacementEpoch,
    owners: smallvec::SmallVec<[EntityId; 4]>,
    source_revision: u64,
    members: smallvec::SmallVec<[PlacementMember; 1]>,
    paint: Arc<super::Paint>,
}

impl Window {
    /// Direct commit for independent native vertical scrollers with one
    /// complete retained child each. No nested captures, virtual-list rows,
    /// external effects, or partially captured content are admitted.
    fn prepare_scroll_placement(&self, cx: &App) -> Result<PlacementTransaction, Outcome> {
        let (epoch, owners) = self.invalidator.pending_placement().ok_or(Outcome::Invalidation)?;
        // Scheduling an ordinary wheel frame sets refreshing. Its pending
        // placement epoch, rather than the scheduling flag, proves the work.
        if !self.has_completed_rendered_frame || self.force_full_redraw.get()
            || self.force_view_cache_refresh || self.recovering_degraded_draw
            || self.rendered_frame.window_active != self.active.get()
            || self.rendered_frame.focus != self.focus
        {
            return Err(Outcome::FrameState);
        }
        if self.live_resize_active || self.viewport_epoch != self.committed_epoch
            || self.scale_factor != self.committed_scale_factor
        {
            return Err(Outcome::Viewport);
        }
        if !self.dirty_views.is_empty() || !self.direct_dirty_views.is_empty() {
            return Err(Outcome::DirtyViews);
        }
        if self.prompt.is_some() || self.tooltip_bounds.is_some()
            || !self.rendered_frame.deferred_draws.is_empty()
            || self.rendered_frame.tooltip_requests.iter().any(Option::is_some)
            || cx.has_active_drag()
        {
            return Err(Outcome::InteractiveOverlay);
        }
        if !self.animation_dirty_region.is_empty() || self.scene_animation_needs_commit.get() {
            return Err(Outcome::Animation);
        }
        if self.is_inspector_picking(cx) {
            return Err(Outcome::Debug);
        }
        if !self.platform_window.supports_deferred_scroll() {
            return Err(Outcome::UnsupportedBackend);
        }
        #[cfg(any(feature = "inspector", debug_assertions))]
        if self.inspector.is_some() { return Err(Outcome::Debug); }
        #[cfg(debug_assertions)]
        if cx.has_global::<crate::DebugBelow>() { return Err(Outcome::Debug); }
        let debug = self.debug_visualization(cx);
        if debug.show_layout_bounds || debug.show_element_updates
            || debug.frame_time_overlay != super::DebugFrameOverlayMode::Hidden
        {
            return Err(Outcome::Debug);
        }

        let frame = &self.rendered_frame;
        let old_paint = frame.published_paint_ref().ok_or(Outcome::MissingPaint)?;
        let mut members = smallvec::SmallVec::<[PlacementMember; 1]>::new();
        let changed: smallvec::SmallVec<[_; 4]> = self.mounted_scroll_routing.borrow_mut()
            .changed_indices(&frame.mounted_scroll_sources).iter()
            .map(|&index| (index, &frame.mounted_scroll_sources[index])).collect();
        if changed.len() != owners.len() {
            return Err(Outcome::MountedSources);
        }
        for &owner in &owners {
            let member = self.prepare_placement_member(cx, epoch, owner, old_paint, &changed)?;
            if members.iter().any(|previous| {
                previous.marker == member.marker
                    || previous.slot == member.slot
                    || previous.mounted_source == member.mounted_source
                    || (previous.hitboxes.start < member.hitboxes.end
                        && member.hitboxes.start < previous.hitboxes.end)
                    || !previous.damage.intersect(&member.damage).is_empty()
            }) {
                return Err(Outcome::MultipleCaptures);
            }
            members.push(member);
        }
        members.sort_unstable_by_key(|member| member.marker);
        let placements: smallvec::SmallVec<[_; 1]> = members.iter()
            .map(|member| (member.marker, member.placement.clone())).collect();
        let paint = old_paint.with_scroll_placements(&placements,
            self.platform_window.background_appearance() == crate::WindowBackgroundAppearance::Opaque)
            .ok_or(Outcome::ScenePublication)?;
        Ok(PlacementTransaction {
            epoch, owners, source_revision: old_paint.scene.revision, members, paint,
        })
    }

    fn prepare_placement_member(
        &self,
        cx: &App,
        epoch: super::lifecycle::PlacementEpoch,
        scroll_owner: EntityId,
        old_paint: &super::Paint,
        changed: &[(usize, &super::scroll_dependencies::MountedScrollSource)],
    ) -> Result<PlacementMember, Outcome> {
        let frame = &self.rendered_frame;
        let mut moved = frame.scroll_views.views.iter().filter_map(|(&key, view)| {
            let direct = view.binding.direct.as_ref()?;
            if direct.owner != scroll_owner || direct.child != view.owner {
                return None;
            }
            let offset = view.binding.offset.upgrade()?;
            let sampled = *offset.borrow();
            (sampled != view.binding.sampled).then_some((key, view, direct, offset, sampled))
        });
        let (key, view, direct, offset, sampled) = moved.next().ok_or(Outcome::MissingCapture)?;
        if moved.next().is_some() {
            return Err(Outcome::MultipleCaptures);
        }
        if view.capture_revision != epoch.content
            || view.reads.iter().any(|read| read.status() != StateReadStatus::Unchanged)
        {
            return Err(Outcome::CaptureDependencies);
        }
        // Every changed source must be matched exactly once by a member.
        // Geometry changes, dead sources and ambiguous ownership revoke proof.
        let mut matching = changed.iter().filter(|(_, source)| {
            source.changed_for(scroll_owner, &offset.version)
        });
        let (mounted_source, _) = matching.next().copied().ok_or(Outcome::MountedSources)?;
        if matching.next().is_some() {
            return Err(Outcome::MountedSources);
        }
        let delta = sampled - view.binding.sampled;
        if delta.x != Pixels::ZERO || delta.y == Pixels::ZERO
            || !sampled.y.0.is_finite() || !delta.y.0.is_finite() || sampled.y > Pixels::ZERO
            || direct.range.as_ref().is_some_and(|range| {
                range.geometry.status() != StateReadStatus::Unchanged
                    || !range.min_y.0.is_finite()
                    || sampled.y < range.min_y
            })
        {
            return Err(Outcome::OffsetRange);
        }

        let slot = *frame.view_retentions.get(&key).ok_or(Outcome::RetentionRecord)?;
        let entry = frame.view_retention_records.get(slot).ok_or(Outcome::RetentionRecord)?;
        if entry.key != key || entry.owner != view.owner { return Err(Outcome::RetentionRecord); }
        let record = entry.record.as_ref().ok_or(Outcome::RetentionRecord)?;
        let input = hitboxes(&record.prepaint_range);
        let pre = &record.prepaint_range;
        let paint = &record.paint_range;
        if !record.nested.is_empty() || !record.scene_segments.is_empty() {
            return Err(Outcome::NestedRetention);
        }
        if !frame.hitboxes.contains(&input)
            || record.context.visual_content_mask != view.viewport
            || pre.start.tooltips_index != pre.end.tooltips_index
            || pre.start.deferred_draws_index != pre.end.deferred_draws_index
            || paint.start.input_handlers_index != paint.end.input_handlers_index
            || paint.start.window_control_hitboxes_index != paint.end.window_control_hitboxes_index
            || paint.start.tab_handle_index != paint.end.tab_handle_index
            || paint.start.cursor_styles_index != paint.end.cursor_styles_index
        {
            return Err(Outcome::InputRecords);
        }
        #[cfg(any(test, feature = "test-support"))]
        if paint.start.debug_bounds_index != paint.end.debug_bounds_index { return Err(Outcome::InputRecords); }
        let listeners = paint.start.mouse_listeners_index != paint.end.mouse_listeners_index;
        let hover_held = self.scroll_hover.deadline
            .is_some_and(|end| cx.background_executor().now() < end);
        if !hover_held && (listeners || view.stale_input
            || self.hits_scroll_row(&frame.hitboxes, input.clone(), Point::default())
            || self.hits_scroll_row(&frame.hitboxes, input.clone(), delta))
        {
            return Err(Outcome::Hover);
        }
        let marker = paint.start.scene_index;
        if paint.end.scene_index != marker + 1 { return Err(Outcome::Marker); }
        let crate::scene::PaintOperationRef::DeferredScroll(old) =
            old_paint.scene.paint_operations.view(marker).ok_or(Outcome::Marker)? else { return Err(Outcome::Marker); };
        let scale = self.scale_factor();
        let old_delta = view.source.translation(record.context.element_offset, scale).ok_or(Outcome::PreviousTranslation)?;
        let old_expected = view.source.placement(view.viewport, scale, old_delta).ok_or(Outcome::PreviousPlacement)?;
        if !old.same_paint(&old_expected) { return Err(Outcome::PreviousMarker); }
        let physical = view.source.translation(record.context.element_offset + delta, scale).ok_or(Outcome::FractionalTranslation)?;
        let placement = view.source.placement(view.viewport, scale, physical).ok_or_else(|| {
            if !view.source.covers(view.viewport.bounds.scale(scale), physical) {
                Outcome::Coverage
            } else {
                Outcome::PlacementGeometry
            }
        })?;
        let damage = placement.visual_bounds();
        // The View and enclosing damage records keep the same fixed
        // viewport. No stale damage bounds may survive the transaction.
        let segment = frame.retained_scene_segments.get(record.scene_segments.end).ok_or(Outcome::Damage)?;
        if segment.entity_id != view.owner || segment.scene_range != (marker..marker + 1)
            || segment.bounds != damage
        {
            return Err(Outcome::Damage);
        }
        Ok(PlacementMember {
            marker, placement, key, slot,
            mounted_source, offset, sampled, delta, hitboxes: input,
            damage,
            stale_input: view.stale_input || listeners || (hover_held && !hitboxes(pre).is_empty()),
        })
    }

    /// Commit before begin_draw_cycle. All rejection paths are side-effect
    /// free with respect to the frame and leave the dirty request available
    /// for normal rendering. No app callbacks execute between the final
    /// generation check and committing the CPU records + immutable Paint.
    pub(super) fn try_commit_scroll_placement(&mut self, cx: &mut App) -> bool {
        if self.invalidator.pending_placement().is_none() { return false; }
        // Also covers synchronous Window::draw callers outside platform RAF.
        self.invalidate_scroll_reads();
        let started = Instant::now();
        let transaction = match self.prepare_scroll_placement(cx) {
            Ok(transaction) => transaction,
            Err(outcome) => {
                scroll_placement::record(outcome);
                log::trace!(target: "gpui::diagnostics", "gpui placement rejected: {outcome:?}");
                return false;
            }
        };
        if self.rendered_frame.scene().revision != transaction.source_revision
            || transaction.members.iter().any(|member| *member.offset.borrow() != member.sampled)
            || !self.invalidator.commit_placement(transaction.epoch, &transaction.owners)
        {
            scroll_placement::record(Outcome::CommitRace);
            return false;
        }
        let frame = &mut self.rendered_frame;
        for member in &transaction.members {
            let record = frame.view_retention_records[member.slot].record.as_mut()
                .expect("validated retained view");
            record.bounds.origin += member.delta;
            Rc::make_mut(&mut record.context).element_offset += member.delta;
            record.splice_safe = false;
            let view = frame.scroll_views.views.get_mut(&member.key).expect("validated scroll view");
            view.binding.sampled = member.sampled;
            view.stale_input = member.stale_input;
            frame.mounted_scroll_sources[member.mounted_source].acknowledge(&member.offset.version);
        }
        frame.commit_scroll_placements(Arc::clone(&transaction.paint),
            transaction.members.iter().map(|member| (member.hitboxes.clone(), member.delta)));
        self.mouse_hit_test = self.pointer_hit_test(&self.rendered_frame, self.mouse_position);
        if let Some(started_at) = self.invalidator.take_dirty_to_present_start() {
            self.active_dirty_to_present_started_at = Some(
                self.active_dirty_to_present_started_at.map_or(started_at, |active| active.min(started_at)));
        }
        if !self.needs_present.get() {
            self.render_dirty_region = super::DirtyRegion::empty();
        }
        for member in &transaction.members {
            self.render_dirty_region.push(member.damage);
        }
        let viewport = Bounds::new(Point::default(), self.viewport_size).scale(self.scale_factor);
        self.render_dirty_region.coalesce_if_large(viewport, super::DIRTY_REGION_FULL_REDRAW_RATIO);
        self.render_present_mode = if self.render_dirty_region.is_full() {
            super::PartialPresentMode::FullRedraw
        } else { super::PartialPresentMode::Partial };
        self.render_trim_policy = super::RetainedResourceTrimPolicy::None;
        self.idle_render_frames = 0;
        self.backdrop_blur_damage = Default::default();
        self.presentation_state.publish_pending(transaction.paint, Vec::new());
        self.needs_present.set(true);
        self.refreshing = false;
        self.dirty_frame_scheduled = false;
        self.draw_was_degraded = false;
        self.frame_can_degrade = false;
        self.draw_deadline = None;
        self.checked_scroll_generation = Some(crate::state_generation());
        self.mounted_scroll_routing.get_mut().acknowledge_all();
        self.last_generation_stats = super::state::FrameGenerationStats::default();
        self.last_generation_stats.scene = self.rendered_frame.scene().frame_metrics();
        self.last_generation_stats.frame_retained_capacity = self.rendered_frame.retained_capacity();
        self.last_generation_stats.scene_finish_elapsed = started.elapsed();
        self.last_generation_stats.finalize_elapsed = started.elapsed();
        super::record_dirty_region_metrics(self.render_dirty_region.rect_count(), self.render_dirty_region.area() as usize);
        super::record_scene_frame_metrics(self.last_generation_stats.scene);
        super::record_frame_retained_capacity(self.last_generation_stats.frame_retained_capacity);
        self.reset_cursor_style(cx);
        if transaction.members.iter().any(|member| member.stale_input) { self.schedule_scroll_hover_restore(cx); }
        scroll_placement::record(Outcome::Committed);
        log::trace!(target: "gpui::diagnostics", "gpui placement-only frame committed without root traversal");
        true
    }

    pub(crate) fn with_scroll_viewport<R>(
        &mut self,
        scroll_source: Option<Weak<ScrollOffset>>,
        offset: Point<Pixels>,
        direct_child: Option<(EntityId, Option<TrackedScrollRange>)>,
        prepaint: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let Some(scroll_source) = scroll_source.filter(|_| self.scroll_row_replay_is_enabled()) else {
            return prepaint(self);
        };
        let viewport = self.visual_content_mask();
        let previous = self.next_frame.scroll_views.viewport.take();
        if previous.is_none()
            && self.visual_scale() == 1.
            && self.element_visual_transform.translation == Point::default()
            && self.content_mask() == viewport
            && !viewport.bounds.is_empty()
            && offset.x == Pixels::ZERO
            && let Some(mut binding) = ScrollBinding::new(scroll_source, offset)
        {
            let owner = self.current_view();
            binding.direct = direct_child.filter(|(child, _)| *child != owner)
                .map(|(child, range)| DirectScroll { owner, child, range });
            self.next_frame.scroll_views.viewport = Some(ScrollViewport { clip: viewport, binding });
        }
        let result = prepaint(self);
        self.next_frame.scroll_views.viewport = previous;
        result
    }

    fn scroll_viewport(&self, cx: &App) -> Option<&ScrollViewport> {
        if !self.scroll_row_replay_is_enabled()
            || !self.invalidator.scroll_content_revision().1
            || self.force_view_cache_refresh
            || cx.has_active_drag()
            || self.is_inspector_picking(cx)
        {
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
        let debug = self.debug_visualization(cx);
        if debug.show_layout_bounds || debug.show_element_updates {
            return None;
        }
        self.next_frame
            .scroll_views
            .viewport.as_ref()
            .filter(|scope| scope.clip == self.visual_content_mask())
    }

    /// The request-layout proof still holds for a view whose only inherited
    /// change is its placement in the scrolling content.
    pub(crate) fn scroll_view_translation(
        &self,
        previous: &RetainedViewRecord,
        current: &RetainedPaintContext,
        bounds: Bounds<Pixels>,
        cx: &App,
    ) -> Option<Point<Pixels>> {
        let viewport = self.scroll_viewport(cx)?.clip;
        let offset = current.element_offset - previous.context.element_offset;
        if offset.x != Pixels::ZERO
            || bounds.size != previous.bounds.size
            || bounds.origin != previous.bounds.origin + offset
            || viewport != previous.context.visual_content_mask
        {
            return None;
        }
        let mut translated = (*previous.context).clone();
        translated.element_offset = current.element_offset;
        (translated == *current).then_some(offset)
    }

    pub(crate) fn prepaint_scrolled_view(
        &mut self,
        key: u64,
        owner: EntityId,
        previous: &RetainedViewRecord,
        current: &RetainedPaintContext,
        bounds: Bounds<Pixels>,
        cx: &App,
    ) -> Option<(Range<PrepaintStateIndex>, ScrollBinding)> {
        let delta = self.scroll_view_translation(previous, current, bounds, cx)?;
        let binding = &self.scroll_viewport(cx)?.binding;
        let viewport = current.visual_content_mask;
        let source = self.rendered_frame.scroll_views.views.get(&key)?;
        // Reject a new scroll handle or movement not explained by the
        // exact committed offset. Parent placement is not a scroll delta.
        if binding.delta_from(&source.binding) != Some(delta) {
            return None;
        }
        let scale = self.scale_factor();
        let physical = source.source.translation(current.element_offset, scale)?;
        if source.owner != owner
            || source.viewport != viewport
            || source.capture_revision != self.invalidator.scroll_content_revision().0
            || source
                .reads
                .iter()
                .any(|read| read.status() != StateReadStatus::Unchanged)
            || !source.source.covers(viewport.scale(scale).bounds, physical)
            || !previous.nested.is_empty()
            || !self.can_reuse_prepaint(&previous.prepaint_range)
            || !self.can_reuse_paint(&previous.paint_range)
        {
            return None;
        }
        let moved_hitboxes = hitboxes(&previous.prepaint_range);
        if !self.scroll_hover_is_held(cx)
            && self.hits_scroll_row(&self.rendered_frame.hitboxes, moved_hitboxes.clone(), delta)
        {
            return None;
        }
        let mut binding = binding.clone();
        binding.direct = binding.direct.filter(|direct| direct.child == owner);
        let start = self.prepaint_index();
        if !moved_hitboxes.is_empty() {
            self.next_frame.input_clip_epoch = self.next_frame.input_clip_epoch.wrapping_add(1);
            self.next_frame.hitboxes.share(&self.rendered_frame.hitboxes, moved_hitboxes, delta);
        }
        let parent = self.next_frame.dispatch_tree.active_node_id();
        self.replay_prepaint_records(previous.prepaint_range.clone(), |_| parent);
        Some((start..self.prepaint_index(), binding))
    }

    pub(crate) fn plan_scroll_view_capture(
        &self,
        key: Option<u64>,
        element: &mut AnyElement,
        prepaint: &Range<PrepaintStateIndex>,
        bounds: Bounds<Pixels>,
        read_start: usize,
        cx: &App,
    ) -> Option<Box<ScrollViewCapture>> {
        let scope = self.scroll_viewport(cx)?;
        let viewport = scope.clip;
        // A small view within a long page leaves useful retention to the
        // enclosing view that covers the scrolling viewport.
        if bounds.size.height < viewport.bounds.size.height
            || !bounds.intersects(&viewport.bounds)
            || !self.can_capture_scroll_view_input(prepaint, viewport)
            || !element.can_capture_scrolling_view()
        {
            return None;
        }
        let mut binding = scope.binding.clone();
        binding.direct = binding.direct.filter(|direct| direct.child == self.current_view());
        Some(Box::new(ScrollViewCapture {
            key: key?,
            revision: self.invalidator.scroll_content_revision().0,
            viewport,
            binding,
            reads: cx.entities.state_reads_since(read_start),
        }))
    }

    fn can_capture_scroll_view_input(
        &self,
        prepaint: &Range<PrepaintStateIndex>,
        viewport: ContentMask<Pixels>,
    ) -> bool {
        let input = hitboxes(prepaint);
        prepaint.start.deferred_draws_index == prepaint.end.deferred_draws_index
            && prepaint.start.tooltips_index == prepaint.end.tooltips_index
            && self.next_frame.hitboxes.slices(input).all(|(slice, _)|
                slice.iter().all(|hitbox| hitbox.content_mask == viewport))
    }

    pub(crate) fn paint_scroll_view(
        &mut self,
        capture: ScrollViewCapture,
        owner: EntityId,
        element: &mut AnyElement,
        prepaint: &Range<PrepaintStateIndex>,
        context: &RetainedPaintContext,
        cx: &mut App,
    ) {
        let input = hitboxes(prepaint);
        let suppress_hover = !input.is_empty() && self.scroll_hover_is_held(cx);
        if self.visual_content_mask() != capture.viewport
            || !capture.binding.sample_still_current()
            || self.scene_animation.is_some()
            || self.next_frame.painter.is_capturing_blur()
            || (!suppress_hover && self.hits_scroll_row(&self.next_frame.hitboxes, input, Point::default()))
        {
            element.paint(self, cx);
            return;
        }
        let read_start = cx.entities.state_read_index();
        let previous_source = self.rendered_frame.scroll_views.views.get(&capture.key)
            .filter(|previous| previous.owner == owner && previous.viewport == capture.viewport
                && previous.binding.same_source(&capture.binding))
            .map(|previous| Arc::clone(&previous.source));
        let target = if capture.binding.direct.is_some() {
            ScrollPaintTarget::DirectView
        } else {
            ScrollPaintTarget::View
        };
        let source = self.capture_scroll_paint(context.element_offset, element, suppress_hover,
            previous_source.as_deref(), target, cx);
        if suppress_hover {
            self.schedule_scroll_hover_restore(cx);
        }
        let paint_reads = cx.entities.state_reads_since(read_start);
        let reads = if paint_reads.is_empty() {
            capture.reads
        } else {
            capture
                .reads
                .iter()
                .chain(paint_reads.iter())
                .cloned()
                .collect()
        };
        self.next_frame.scroll_views.views.insert(
            capture.key,
            ScrollView {
                capture_revision: capture.revision,
                owner,
                viewport: capture.viewport,
                binding: capture.binding,
                source,
                reads,
                stale_input: suppress_hover,
            },
        );
    }

    pub(crate) fn paint_scrolled_view(
        &mut self,
        key: u64,
        binding: ScrollBinding,
        previous: &RetainedViewRecord,
        context: &RetainedPaintContext,
        cx: &App,
    ) {
        let mut source = self.rendered_frame.scroll_views.views[&key].clone();
        // Use the placement proven in Prepaint, not a fresh read after
        // application callbacks may have mutated the scroll handle.
        source.binding = binding;
        let scale = self.scale_factor();
        let delta = source
            .source
            .translation(context.element_offset, scale)
            .expect("prepaint proved integral scroll");
        let unchanged = previous.context.element_offset == context.element_offset;
        let old_range = previous.paint_range.start.scene_index..previous.paint_range.end.scene_index;
        let was_deferred = old_range.len() == 1 && matches!(
            self.rendered_frame.scene().paint_operations.view(old_range.start),
            Some(crate::scene::PaintOperationRef::DeferredScroll(_))
        );
        let viewport = source.viewport.scale(scale);
        let target = if source.binding.direct.is_some() {
            ScrollPaintTarget::DirectView
        } else {
            ScrollPaintTarget::View
        };
        // Only Nova consumes logical roots. An independently ordered
        // frozen source can be emitted without projecting any primitives.
        let deferred = (!unchanged || was_deferred)
            && self.platform_window.supports_deferred_scroll()
            && self.scene_animation.is_none()
            && target.placement(&source.source, source.viewport, scale, delta)
                .is_some_and(|placement| self.next_frame.painter.push_scroll_placement_against(
                    self.rendered_frame.scene(), old_range.clone(), placement,
                ));
        if !deferred {
            if unchanged && !was_deferred {
                self.replay_validated_scene(old_range);
            } else {
                source.source.replay(&mut self.next_frame.painter, viewport, delta);
            }
        }
        self.replay_validated_paint_records(&previous.paint_range);
        if !hitboxes(&previous.prepaint_range).is_empty() && self.scroll_hover_is_held(cx) {
            source.stale_input = true;
            self.schedule_scroll_hover_restore(cx);
        }
        source.stale_input |= delta != Point::default()
            && previous.paint_range.start.mouse_listeners_index
                != previous.paint_range.end.mouse_listeners_index;
        self.next_frame.scroll_views.views.insert(key, source);
    }

    /// Mouse closures may compare against the original prepaint bounds.
    /// Restore those callbacks before dispatching positional input. Native
    /// wheels remain on the enclosing scroller and use translated hitboxes.
    pub(super) fn refresh_scrolled_view_input(&mut self, cx: &mut App) {
        self.scroll_hover.deadline = None;
        self.scroll_hover.timer.take();
        if self.invalidate_scrolled_view_input() {
            self.draw(cx).clear();
        }
    }

    pub(super) fn note_scroll_wheel_for_hover(&mut self, cx: &App) {
        self.scroll_hover.deadline = Some(cx.background_executor().now() + SCROLL_HOVER_DELAY);
    }

    pub(super) fn scroll_view_hover_suppressed(&self) -> bool {
        self.scroll_hover.painting
    }

    pub(super) fn scroll_hover_is_held(&self, cx: &App) -> bool {
        self.invalidator.scroll_content_revision().1
            && self.scroll_hover.deadline.is_some_and(|end| cx.background_executor().now() < end)
    }

    fn invalidate_scrolled_view_input(&mut self) -> bool {
        let row_owners = self.scroll_hover.row_owners.drain();
        self.invalidator.invalidate_dependent_views(
            self.rendered_frame.scroll_views.views.values()
                .filter(|view| view.stale_input)
                .map(|view| view.owner)
                .chain(row_owners),
        )
    }

    pub(super) fn schedule_scroll_hover_restore(&mut self, cx: &App) {
        if self.scroll_hover.timer.is_some() || self.scroll_hover.deadline.is_none() {
            return;
        }
        // One timer per window. Later wheel events extend its deadline without
        // allocating another task. Only committed captured views need repaint.
        self.scroll_hover.timer = Some(self.spawn(cx, async |cx| {
            let mut delay = SCROLL_HOVER_DELAY;
            loop {
                cx.background_executor.timer(delay).await;
                let Ok(Some(remaining)) = cx.update(|window, cx| {
                    let deadline = window.scroll_hover.deadline?;
                    if let Some(remaining) = deadline.checked_duration_since(cx.background_executor().now())
                        && !remaining.is_zero()
                    {
                        return Some(remaining);
                    }
                    window.scroll_hover.deadline = None;
                    window.scroll_hover.timer.take();
                    if window.invalidate_scrolled_view_input() {
                        window.schedule_dirty_frame();
                    }
                    None
                }) else {
                    break;
                };
                delay = remaining;
            }
        }));
    }
}


#[cfg(test)]
mod binding_tests {
    use super::*;
    use crate::{point, px};

    #[gpui::test]
    fn logical_placement_skips_root_when_shadow_requires_owner_replay(cx: &mut crate::TestAppContext) {
        use crate::{AppContext as _, Context, Entity, InteractiveElement, IntoElement,
            ParentElement, PlatformInput, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent,
            StatefulInteractiveElement, Styled, div, point, px};
        use std::cell::Cell;

        struct Page;
        impl Render for Page {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().relative().w(px(200.)).h(px(512.)).flex_shrink_0().child(
                    div().absolute().left(px(20.)).top(px(128.)).size(px(32.)).bg(crate::red()).shadow_sm()
                )
            }
        }
        struct Scroller {
            child: Entity<Page>,
            builds: Rc<Cell<usize>>,
            scroll: ScrollHandle,
        }
        impl Render for Scroller {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.builds.set(self.builds.get() + 1);
                div().id("direct-scroller").w(px(200.)).h(px(128.)).overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().w_full().pt(px(12.)).child(self.child.clone()))
            }
        }
        let builds = Rc::new(Cell::new(0));
        let scroll = ScrollHandle::new();
        let (_, cx) = cx.add_window_view(|window, cx| {
            window.platform_window.as_test().unwrap().set_deferred_scroll_support(true);
            Scroller { child: cx.new(|_| Page), builds: builds.clone(), scroll: scroll.clone() }
        });
        let wheel = |delta| PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: point(px(30.), px(71.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            ..Default::default()
        });
        cx.update(|window, app| {
            window.draw(app).clear();
            window.dispatch_event(wheel(-32.), app);
            window.draw(app).clear();
            assert!(window.rendered_frame.scene().paint_operations.has_deferred_scroll());
            assert!(window.test_present().is_accepted());
            let before = builds.get();
            let old = window.rendered_frame.published_paint();
            assert!(old.layers.frames.is_empty(), "blurred shadow needs owner replay");
            window.dispatch_event(wheel(-32.), app);
            assert!(window.refreshing, "the native wheel scheduled a frame");
            assert!(window.prepare_scroll_placement(app).is_ok(), "complete source must admit placement");
            window.draw(app).clear();
            let new = window.rendered_frame.published_paint();
            assert_eq!(builds.get(), before, "root Render must not execute");
            assert_eq!(window.last_generation_stats.prepaint_elapsed, Duration::ZERO);
            assert_eq!(window.last_generation_stats.paint_elapsed, Duration::ZERO);
            assert_ne!(old.scene.revision, new.scene.revision);
            assert!(!window.invalidator.is_dirty());
            assert!(!window.refreshing);
            assert_eq!(old.scene.len(), new.scene.len());
            assert!(new.layers.frames.is_empty());
            let old_raster = old.owner_fallback().unwrap().unwrap();
            let new_raster = new.owner_fallback().unwrap().unwrap();
            assert!(!old_raster.scene.shadows.is_empty());
            assert_eq!(old_raster.scene.shadows.len(), new_raster.scene.shadows.len());
            for (old, new) in old_raster.scene.shadows.iter().zip(new_raster.scene.shadows.iter()) {
                assert_eq!(
                    new.bounds.origin.y - old.bounds.origin.y,
                    px(-32.).scale(window.scale_factor()),
                );
            }
            let before = builds.get();
            assert!(window.test_present().is_accepted());
            window.dispatch_event(wheel(-250.), app);
            assert!(matches!(
                window.prepare_scroll_placement(app),
                Err(Outcome::Coverage)
            ), "coverage miss must be classified before falling back");
            window.draw(app).clear();
            assert!(builds.get() > before);

            // A scroll request which must run during prepaint revokes the
            // prior position proof even if the same wheel listener moves.
            window.dispatch_event(wheel(20.), app);
            window.draw(app).clear();
            scroll.scroll_to_top_of_item(0);
            window.dispatch_event(wheel(12.), app);
            assert!(window.prepare_scroll_placement(app).is_err());
            window.draw(app).clear();
            assert_eq!(scroll.offset().y, Pixels::ZERO);
        });
    }

    #[test]
    fn placement_requires_the_same_live_source_and_exact_sample() {
        let first = Rc::new(ScrollOffset::default());
        let second = Rc::new(ScrollOffset::default());
        let old = ScrollBinding::new(Rc::downgrade(&first), Point::default()).unwrap();
        let replacement = ScrollBinding::new(Rc::downgrade(&second), Point::default()).unwrap();
        assert_eq!(replacement.delta_from(&old), None);
        *first.borrow_mut() = point(px(0.), px(-20.));
        assert!(!old.sample_still_current());
        assert!(ScrollBinding::new(Rc::downgrade(&first), Point::default()).is_none());
        let new = ScrollBinding::new(Rc::downgrade(&first), point(px(0.), px(-20.))).unwrap();
        assert_eq!(new.delta_from(&old), Some(point(px(0.), px(-20.))));
        assert!(new.sample_still_current());
        drop(first);
        assert!(!new.sample_still_current());
    }
}
