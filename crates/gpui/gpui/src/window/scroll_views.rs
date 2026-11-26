//! ScrollPage-style views keep their layout and an overscanned paint source.
//! Adapted from GPUI Fast's scroll-layer paint/reuse phases.

use super::{
    PrepaintStateIndex, RetainedPaintContext, RetainedViewRecord, scroll_rows::ScrollPaint,
};
use crate::{
    AnyElement, App, Bounds, ContentMask, EntityId, Pixels, Point, StateRead, StateReadStatus,
    Window,
};
use collections::FxHashMap;
use std::{ops::Range, rc::Rc};

#[derive(Default)]
pub(super) struct ScrollViews {
    viewport: Option<ContentMask<Pixels>>,
    views: FxHashMap<u64, ScrollView>,
}

impl ScrollViews {
    pub(super) fn clear(&mut self) {
        self.viewport = None;
        self.views.clear();
    }

    pub(super) fn carry(&mut self, key: u64, previous: &Self) {
        if let Some(source) = previous.views.get(&key) {
            self.views.entry(key).or_insert_with(|| source.clone());
        }
    }
}

pub(crate) struct ScrollViewCapture {
    key: u64,
    revision: u64,
    viewport: ContentMask<Pixels>,
    reads: Rc<[StateRead]>,
}

#[derive(Clone)]
struct ScrollView {
    capture_revision: u64,
    owner: EntityId,
    viewport: ContentMask<Pixels>,
    source: Rc<ScrollPaint>,
    reads: Rc<[StateRead]>,
    stale_input: bool,
}

fn hitboxes(range: &Range<PrepaintStateIndex>) -> Range<usize> {
    range.start.hitboxes_index..range.end.hitboxes_index
}

impl Window {
    pub(crate) fn with_scroll_viewport<R>(
        &mut self,
        vertical_scroll: bool,
        offset: Point<Pixels>,
        prepaint: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if !vertical_scroll || !self.scroll_row_replay_is_enabled() {
            return prepaint(self);
        }
        let viewport = self.visual_content_mask();
        let previous = self.next_frame.scroll_views.viewport.take();
        if previous.is_none()
            && self.visual_scale() == 1.
            && self.element_visual_transform.translation == Point::default()
            && self.content_mask() == viewport
            && viewport.corner_radii == Default::default()
            && !viewport.bounds.is_empty()
            && offset.x == Pixels::ZERO
        {
            self.next_frame.scroll_views.viewport = Some(viewport);
        }
        let result = prepaint(self);
        self.next_frame.scroll_views.viewport = previous;
        result
    }

    fn scroll_viewport(&self, cx: &App) -> Option<ContentMask<Pixels>> {
        if !self.scroll_row_replay_is_enabled()
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
            .viewport
            .filter(|&clip| clip == self.visual_content_mask())
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
        let viewport = self.scroll_viewport(cx)?;
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
    ) -> Option<Range<PrepaintStateIndex>> {
        let delta = self.scroll_view_translation(previous, current, bounds, cx)?;
        let viewport = current.visual_content_mask;
        let source = self.rendered_frame.scroll_views.views.get(&key)?;
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
        let moved_hitboxes = &self.rendered_frame.hitboxes[hitboxes(&previous.prepaint_range)];
        if self.hits_scroll_row(moved_hitboxes, delta) {
            return None;
        }
        let start = self.prepaint_index();
        if !moved_hitboxes.is_empty() {
            self.next_frame.input_clip_epoch = self.next_frame.input_clip_epoch.wrapping_add(1);
            self.next_frame
                .hitboxes
                .extend(moved_hitboxes.iter().cloned().map(|mut hitbox| {
                    hitbox.bounds.origin += delta;
                    hitbox
                }));
        }
        let parent = self.next_frame.dispatch_tree.active_node_id();
        self.replay_prepaint_records(previous.prepaint_range.clone(), |_| parent);
        Some(start..self.prepaint_index())
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
        let viewport = self.scroll_viewport(cx)?;
        // A small view within a long page leaves useful retention to the
        // enclosing view that covers the scrolling viewport.
        if bounds.size.height < viewport.bounds.size.height
            || !bounds.intersects(&viewport.bounds)
            || !self.can_capture_scroll_view_input(prepaint, viewport)
            || !element.can_capture_scrolling_view()
        {
            return None;
        }
        Some(Box::new(ScrollViewCapture {
            key: key?,
            revision: self.invalidator.scroll_content_revision().0,
            viewport,
            reads: cx.entities.state_reads_since(read_start),
        }))
    }

    fn can_capture_scroll_view_input(
        &self,
        prepaint: &Range<PrepaintStateIndex>,
        viewport: ContentMask<Pixels>,
    ) -> bool {
        let input = &self.next_frame.hitboxes[hitboxes(prepaint)];
        prepaint.start.deferred_draws_index == prepaint.end.deferred_draws_index
            && prepaint.start.tooltips_index == prepaint.end.tooltips_index
            && input.iter().all(|hitbox| hitbox.content_mask == viewport)
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
        let input = &self.next_frame.hitboxes[hitboxes(prepaint)];
        if self.visual_content_mask() != capture.viewport
            || self.scene_animation.is_some()
            || self.next_frame.painter.is_capturing_blur()
            || self.hits_scroll_row(input, Point::default())
        {
            element.paint(self, cx);
            return;
        }
        let read_start = cx.entities.state_read_index();
        let source = self.capture_scroll_paint(context.element_offset, element, cx);
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
                source,
                reads,
                stale_input: false,
            },
        );
    }

    pub(crate) fn paint_scrolled_view(
        &mut self,
        key: u64,
        previous: &RetainedViewRecord,
        context: &RetainedPaintContext,
    ) {
        let mut source = self.rendered_frame.scroll_views.views[&key].clone();
        let scale = self.scale_factor();
        let delta = source
            .source
            .translation(context.element_offset, scale)
            .expect("prepaint proved integral scroll");
        source.source.replay(
            &mut self.next_frame.painter,
            source.viewport.scale(scale),
            delta,
        );
        self.replay_validated_paint_records(&previous.paint_range);
        source.stale_input |= delta != Point::default()
            && previous.paint_range.start.mouse_listeners_index
                != previous.paint_range.end.mouse_listeners_index;
        self.next_frame.scroll_views.views.insert(key, source);
    }

    /// Mouse closures may compare against the original prepaint bounds.
    /// Restore those callbacks before dispatching positional input. Native
    /// wheels remain on the enclosing scroller and use translated hitboxes.
    pub(super) fn refresh_scrolled_view_input(&mut self, cx: &mut App) {
        let mut dirty = false;
        for source in self.rendered_frame.scroll_views.views.values() {
            if source.stale_input {
                dirty |= self.invalidator.invalidate_owned_view(source.owner);
            }
        }
        if dirty {
            self.draw(cx).clear();
        }
    }
}
