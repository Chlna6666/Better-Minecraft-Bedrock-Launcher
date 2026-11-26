//! Uniform-list row reuse, from GPUI Fast's lists/paint phases.
//! Capture paint past the viewport so a translated row can reveal pixels
//! that the prior frame never submitted.

use super::{PaintIndex, PrepaintStateIndex, RetainedPaintContext};
use crate::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, GlobalElementId, Hitbox, Pixels, Point,
    ScaledPixels, Size, StateRead, StateReadStatus, Window, point,
    scene::{Operations, PaintOperation, Painter, Primitive},
};
use collections::FxHashMap;
use smallvec::SmallVec;
use std::{ops::Range, rc::Rc};

#[derive(Default)]
pub(super) struct ScrollRows {
    rows: FxHashMap<(u64, usize), RowRecord>,
    pub(super) enabled: bool,
    blocked: bool,
}

impl ScrollRows {
    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.enabled = false;
        self.blocked = false;
    }

    pub(super) fn trim_for_reuse(&mut self, current: &Self) {
        let target = current.rows.len().max(32);
        if self.rows.capacity() > target.saturating_mul(4) {
            self.rows.shrink_to(target);
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
    pub(crate) prepainted: FxHashMap<usize, RowPrepaint>,
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
    content: Rc<ScrollPaint>,
}

pub(super) struct ScrollPaint {
    origin: Point<Pixels>,
    coverage: Bounds<ScaledPixels>,
    operations: Operations,
}

impl Window {
    pub(crate) fn scroll_row_replay_is_enabled(&self) -> bool {
        let rows = &self.next_frame.scroll_rows;
        rows.enabled && !rows.blocked
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
        let (revision, _) = self.invalidator.scroll_content_revision();
        let viewport = self.visual_content_mask();
        if !self.scroll_row_replay_is_enabled()
            || self.force_view_cache_refresh
            || self.visual_scale() != 1.
            || self.content_mask() != viewport
            || self.element_visual_transform.translation != Point::default()
            || viewport.bounds.is_empty()
            || viewport.corner_radii != Default::default()
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
            prepainted: FxHashMap::default(),
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
        let coverage = moved(record.content.coverage, delta);
        let logical_delta = origin - record.prepaint.origin;
        let hitboxes = &self.rendered_frame.hitboxes[hitbox_range(&record.prepaint)];
        record.revision == plan.revision
            && *record.context == *plan.context
            && coverage.intersect(&viewport) == viewport
            && record
                .prepaint
                .reads
                .iter()
                .all(|read| read.status() == StateReadStatus::Unchanged)
            && self.can_reuse_prepaint(&record.prepaint.records)
            && self.can_reuse_paint(&record.paint)
            && !self.hits_scroll_row(hitboxes, logical_delta)
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
        self.next_frame.hitboxes.extend(
            self.rendered_frame.hitboxes[hitbox_range(&record.prepaint)]
                .iter()
                .cloned()
                .map(|mut hitbox| {
                    hitbox.bounds.origin += delta;
                    hitbox
                }),
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
            plan.prepainted
                .remove(&row)
                .map(|prepaint| (plan, prepaint))
        });
        match (item, prepaint) {
            (Some(mut item), Some((plan, prepaint))) => {
                self.paint_scroll_row(plan, row, &mut item, prepaint, cx)
            }
            (None, Some((plan, prepaint))) => self.paint_reused_scroll_row(plan, row, prepaint),
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
        let hitboxes = &self.next_frame.hitboxes[hitbox_range(&prepaint)];
        if self.next_frame.painter.is_capturing_blur()
            || self.scene_animation.is_some()
            || hitboxes
                .iter()
                .any(|hitbox| hitbox.content_mask != viewport)
            || self.hits_scroll_row(hitboxes, Point::default())
        {
            item.paint(self, cx);
            return;
        }
        let read_start = cx.entities.state_read_index();
        let start = self.paint_index();
        let content = self.capture_scroll_paint(prepaint.origin, item, cx);
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

    fn paint_reused_scroll_row(&mut self, plan: &RowPlan, row: usize, prepaint: RowPrepaint) {
        let previous = &self.rendered_frame.scroll_rows.rows[&(plan.key, row)];
        let content = previous.content.clone();
        let old_paint = previous.paint.clone();
        let start = self.paint_index();
        let scale = self.scale_factor();
        let viewport = self.visual_content_mask().scale(scale);
        let delta = prepaint.origin.scale(scale) - content.origin.scale(scale);
        content.replay(&mut self.next_frame.painter, viewport, delta);
        self.replay_validated_paint_records(&old_paint);
        let paint = start..self.paint_index();
        self.remember_scroll_row(plan, row, prepaint, paint, content);
    }

    fn remember_scroll_row(
        &mut self,
        plan: &RowPlan,
        row: usize,
        prepaint: RowPrepaint,
        paint: Range<PaintIndex>,
        content: Rc<ScrollPaint>,
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

    pub(super) fn hits_scroll_row(&self, hitboxes: &[Hitbox], delta: Point<Pixels>) -> bool {
        !self.pointer_outside
            && hitboxes.iter().any(|hitbox| {
                hitbox.content_mask.bounds.contains(&self.mouse_position)
                    && Bounds::new(hitbox.bounds.origin + delta, hitbox.bounds.size)
                        .contains(&self.mouse_position)
            })
    }

    pub(super) fn capture_scroll_paint(
        &mut self,
        origin: Point<Pixels>,
        item: &mut AnyElement,
        cx: &mut App,
    ) -> Rc<ScrollPaint> {
        let viewport = self.visual_content_mask();
        let height = viewport.bounds.size.height;
        let capture = ContentMask::new(Bounds::new(
            viewport.bounds.origin - point(Pixels::ZERO, height),
            crate::size(viewport.bounds.size.width, height * 3.),
        ));
        self.next_frame.painter.begin_scroll_capture();
        self.content_mask_stack.push(capture);
        self.visual_content_mask_stack.push(capture);
        item.paint(self, cx);
        self.visual_content_mask_stack.pop();
        self.content_mask_stack.pop();
        let operations = self.next_frame.painter.end_scroll_capture();
        let scale = self.scale_factor();
        let content = Rc::new(ScrollPaint {
            origin,
            coverage: capture.scale(scale).bounds,
            operations,
        });
        content.replay(
            &mut self.next_frame.painter,
            viewport.scale(scale),
            Point::default(),
        );
        content
    }
}

fn hitbox_range(prepaint: &RowPrepaint) -> Range<usize> {
    prepaint.records.start.hitboxes_index..prepaint.records.end.hitboxes_index
}

impl ScrollPaint {
    pub(super) fn translation(
        &self,
        origin: Point<Pixels>,
        scale: f32,
    ) -> Option<Point<ScaledPixels>> {
        let delta = origin.scale(scale) - self.origin.scale(scale);
        [delta.x.0, delta.y.0]
            .into_iter()
            .all(|v| v.is_finite() && v.fract() == 0.)
            .then_some(delta)
    }

    pub(super) fn covers(
        &self,
        viewport: Bounds<ScaledPixels>,
        delta: Point<ScaledPixels>,
    ) -> bool {
        moved(self.coverage, delta).intersect(&viewport) == viewport
    }

    pub(super) fn replay(
        &self,
        painter: &mut Painter,
        viewport: ContentMask<ScaledPixels>,
        delta: Point<ScaledPixels>,
    ) {
        let mut layers = SmallVec::<[bool; 8]>::new();
        let source_viewport = Bounds::new(viewport.bounds.origin - delta, viewport.bounds.size);
        for operation in self.operations.iter() {
            match operation {
                PaintOperation::StartLayer(bounds) => {
                    let bounds = moved(*bounds, delta).intersect(&viewport.bounds);
                    layers.push(!bounds.is_empty());
                    if !bounds.is_empty() {
                        painter.push_layer(bounds);
                    }
                }
                PaintOperation::EndLayer => {
                    if layers.pop() == Some(true) {
                        painter.pop_layer();
                    }
                }
                PaintOperation::Primitive(primitive) => {
                    if primitive.visual_bounds().intersects(&source_viewport) {
                        painter.insert_primitive(project_primitive(
                            primitive,
                            delta,
                            viewport.bounds,
                        ));
                    }
                }
                PaintOperation::StartBlur(_) | PaintOperation::EndBlur => {
                    unreachable!("scroll row capture admits only built-in unfiltered paint")
                }
            }
        }
    }
}

fn moved(bounds: Bounds<ScaledPixels>, delta: Point<ScaledPixels>) -> Bounds<ScaledPixels> {
    Bounds::new(bounds.origin + delta, bounds.size)
}

fn project_primitive(
    primitive: &Primitive,
    delta: Point<ScaledPixels>,
    viewport: Bounds<ScaledPixels>,
) -> Primitive {
    // Based on GPUI Fast fast/layers/scene.rs move_primitive. The row clip
    // moves with the content, then intersects the stationary viewport.
    let mask = |mut mask: ContentMask<ScaledPixels>| {
        mask.bounds = moved(mask.bounds, delta).intersect(&viewport);
        if mask.corner_radii != Default::default() {
            mask.corner_bounds = moved(mask.corner_bounds, delta);
        }
        mask
    };
    macro_rules! project {
        ($primitive:ident, $kind:ident) => {{
            let mut primitive = *$primitive;
            primitive.bounds = moved(primitive.bounds, delta);
            primitive.content_mask = mask(primitive.content_mask);
            Primitive::$kind(primitive)
        }};
    }
    match primitive {
        Primitive::Quad(quad) => project!(quad, Quad),
        Primitive::Underline(underline) => project!(underline, Underline),
        Primitive::MonochromeSprite(sprite) => project!(sprite, MonochromeSprite),
        Primitive::PolychromeSprite(sprite) => project!(sprite, PolychromeSprite),
        Primitive::Shadow(shadow) => {
            let mut shadow = *shadow;
            shadow.bounds = moved(shadow.bounds, delta);
            shadow.content_mask = mask(shadow.content_mask);
            Primitive::Shadow(shadow)
        }
        _ => unreachable!("unsupported primitive in built-in scroll row"),
    }
}
