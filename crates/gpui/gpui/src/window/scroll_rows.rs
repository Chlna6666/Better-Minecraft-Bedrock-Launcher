//! Uniform-list row reuse, from GPUI Fast's lists/paint phases.
//! Capture paint past the viewport so a translated row can reveal pixels
//! that the prior frame never submitted.

use super::{PaintIndex, PrepaintStateIndex, RetainedPaintContext, scroll_paint::{ScrollPaint, ScrollPaintTarget}};
use crate::{
    AnyElement, App, AvailableSpace, Bounds, GlobalElementId, Hitbox, Pixels, Point,
    ScrollCapture, Size, StateRead, StateReadStatus, Window,
};
use collections::FxHashMap;
use std::{collections::VecDeque, ops::Range, rc::Rc, sync::Arc};

#[derive(Default)]
pub(super) struct ScrollRows {
    rows: FxHashMap<(u64, usize), RowRecord>,
    measurements: FxHashMap<GlobalElementId, Rc<RowMeasurement>>,
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
        self.measurements.clear();
        self.enabled = false;
        self.blocked = false;
    }

    pub(super) fn trim_for_reuse(&mut self, current: &Self) {
        let target = current.rows.len().max(32);
        if self.rows.capacity() > target.saturating_mul(4) {
            self.rows.shrink_to(target);
        }
        let target = current.measurements.len().max(4);
        if self.measurements.capacity() > target * 4 {
            self.measurements.shrink_to(target);
        }
    }
}

#[derive(PartialEq)]
struct ListContext {
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
    context: Rc<ListContext>,
    hover_held: bool,
    // Both list implementations prepaint and paint rows in the same order.
    // The transient records therefore need no per-frame hash table.
    prepainted: VecDeque<(usize, RowPrepaint)>,
}

impl RowPlan {
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
    pub(crate) origin: Point<Pixels>,
    pub(crate) records: Range<PrepaintStateIndex>,
    pub(crate) reads: Rc<[StateRead]>,
}

struct RowRecord {
    revision: u64,
    context: Rc<ListContext>,
    prepaint: RowPrepaint,
    paint: Range<PaintIndex>,
    content: Arc<ScrollPaint>,
}

struct RowMeasurement {
    revision: u64,
    index: usize,
    count: usize,
    context: RetainedPaintContext,
    reads: Rc<[StateRead]>,
    size: Size<Pixels>,
}

impl Window {
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
        let Some(id) = id.filter(|_| self.scroll_row_replay_is_enabled() && !self.force_view_cache_refresh) else {
            return measure(self, cx).0;
        };
        let (revision, placement_only) = self.invalidator.scroll_content_revision();
        if !placement_only { return measure(self, cx).0; }
        if let Some(previous) = self.rendered_frame.scroll_rows.measurements.get(id)
            && previous.revision == revision
            && previous.index == index
            && previous.count == count
            && self.retained_paint_context_matches(&previous.context)
            && previous.reads.iter().all(|read| read.status() == StateReadStatus::Unchanged)
        {
            let size = previous.size;
            self.next_frame.scroll_rows.measurements.insert(id.clone(), previous.clone());
            self.replayed_view_owners.insert(self.current_view());
            return size;
        }
        // Do not clone the inherited text style on a measurement hit.
        let context = self.current_retained_paint_context();
        let start = cx.entities.state_read_index();
        let (size, cacheable) = measure(self, cx);
        if cacheable {
            self.next_frame.scroll_rows.measurements.insert(id.clone(), Rc::new(RowMeasurement {
                revision,
                index,
                count,
                context,
                reads: cx.entities.state_reads_since(start),
                size,
            }));
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
        cx: &App,
    ) -> Option<RowPlan> {
        let (revision, placement_only) = self.invalidator.scroll_content_revision();
        let viewport = self.visual_content_mask();
        if !placement_only || !self.scroll_row_replay_is_enabled()
            || self.force_view_cache_refresh
            || self.visual_scale() != 1.
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
            hover_held: self.scroll_hover_is_held(cx),
            prepainted: VecDeque::new(),
            context: Rc::new(ListContext {
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
        let Some(record) = self.rendered_frame.scroll_rows.rows.get(&(plan.key, row)) else {
            return false;
        };
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
        record.revision == plan.revision
            && *record.context == *plan.context
            && record.content.covers(viewport, delta)
            && record
                .prepaint
                .reads
                .iter()
                .all(|read| read.status() == StateReadStatus::Unchanged)
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
            origin,
            records: start..self.prepaint_index(),
            reads,
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
        let read_start = cx.entities.state_read_index();
        let start = self.paint_index();
        let previous_source = self.rendered_frame.scroll_rows.rows.get(&(plan.key, row))
            .map(|previous| Arc::clone(&previous.content));
        let content = self.capture_scroll_paint(prepaint.origin, item, suppress_hover,
            previous_source.as_deref(), ScrollPaintTarget::Row, cx);
        if suppress_hover {
            self.restore_scroll_row_after_wheel(cx);
        }
        let paint = start..self.paint_index();
        let paint_reads = cx.entities.state_reads_since(read_start);
        if !paint_reads.is_empty() {
            prepaint.reads = prepaint
                .reads
                .iter()
                .chain(paint_reads.iter())
                .cloned()
                .collect();
        }
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
        prepaint: RowPrepaint,
        paint: Range<PaintIndex>,
        content: Arc<ScrollPaint>,
    ) {
        self.next_frame.scroll_rows.rows.insert(
            (plan.key, row),
            RowRecord {
                revision: plan.revision,
                context: plan.context.clone(),
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
