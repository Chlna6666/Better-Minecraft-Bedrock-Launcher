//! Content-space paint shared by retained views and list rows.

use crate::{
    App, Bounds, ContentMask, Pixels, Point, ScaledPixels, Window, point,
    scene::{Operations, PaintOperationRef, Painter, Primitive, PrimitiveRef},
};
use smallvec::SmallVec;
use std::sync::Arc;

const OPERATIONS_PER_BLOCK: usize = 32;
const MIN_INDEXED_OPERATIONS: usize = 128;

pub(in crate::window) struct ScrollPaint {
    pub(super) origin: Point<Pixels>,
    coverage: Bounds<ScaledPixels>,
    operations: Arc<Operations>,
    replayable: bool,
    static_order: bool,
    scoped_order: bool,
    // Optional compact, immutable row texture in captured physical pixels.
    row_raster_bounds: Option<Bounds<ScaledPixels>>,
    // Allocated only for a source actually published as a native layer.
    native_id: std::sync::OnceLock<u64>,
    native_generation: u64,
    // Logical origin at the first publication of this source identity.
    native_anchor: Point<Pixels>,
    atlas_tiles: Arc<[(crate::AtlasTextureId, u32)]>,
    has_lcd_glyphs: bool,
    has_paths: bool,
    block_bounds: Box<[Option<PaintBlockBounds>]>,
}

#[derive(Clone, Copy)]
pub(super) enum ScrollPaintTarget {
    Row,
    BufferedRow { visible: bool },
    View,
    DirectView,
}

impl ScrollPaintTarget {
    pub(super) fn placement(
        self,
        source: &Arc<ScrollPaint>,
        viewport: ContentMask<Pixels>,
        scale: f32,
        delta: Point<ScaledPixels>,
    ) -> Option<ScrollPlacement> {
        match self {
            Self::Row => source.native_row_placement(viewport, scale, delta).or_else(|| {
                source.has_paths.then_some(())?;
                source.buffered_row_placement(viewport, scale, delta, true)
            }),
            Self::BufferedRow { visible } => source.buffered_row_placement(viewport, scale, delta, visible),
            Self::View => source.native_placement(viewport, scale, delta),
            Self::DirectView => source.placement(viewport, scale, delta),
        }
    }
}

/// Exact corners avoid inward rounding when aggregating widely separated
/// primitives (as in Zed's native BoundsTree). A known block contains only
/// nonempty, finite primitive bounds and no paint scope markers.
#[derive(Clone, Copy)]
struct PaintBlockBounds {
    left: ScaledPixels,
    top: ScaledPixels,
    right: ScaledPixels,
    bottom: ScaledPixels,
}

impl PaintBlockBounds {
    fn new(bounds: Bounds<ScaledPixels>) -> Self {
        Self { left: bounds.left(), top: bounds.top(), right: bounds.right(), bottom: bounds.bottom() }
    }

    fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    fn intersects(self, other: Self) -> bool {
        self.left < other.right && self.right > other.left
            && self.top < other.bottom && self.bottom > other.top
    }

    fn contained_by(self, other: Self) -> bool {
        self.left >= other.left && self.right <= other.right
            && self.top >= other.top && self.bottom <= other.bottom
    }
}

impl Window {
    pub(super) fn capture_scroll_paint(
        &mut self,
        origin: Point<Pixels>,
        suppress_hover: bool,
        previous: Option<&ScrollPaint>,
        target: ScrollPaintTarget,
        cx: &mut App,
        paint: impl FnOnce(&mut Self, &mut App),
    ) -> Arc<ScrollPaint> {
        let viewport = self.visual_content_mask();
        let height = viewport.bounds.size.height;
        let capture = ContentMask::new(Bounds::new(
            viewport.bounds.origin - point(Pixels::ZERO, height),
            crate::size(viewport.bounds.size.width, height * 3.),
        ));
        self.next_frame.painter.begin_scroll_capture();
        self.content_mask_stack.push(capture);
        self.visual_content_mask_stack.push(capture);
        self.scroll_hover.painting = suppress_hover;
        paint(self, cx);
        self.scroll_hover.painting = false;
        self.visual_content_mask_stack.pop();
        self.content_mask_stack.pop();
        let operations = self.next_frame.painter.end_scroll_capture();
        let scale = self.scale_factor();
        let mut content = ScrollPaint::new(origin, capture.scale(scale).bounds, operations);
        if let Some(previous) = previous {
            content.inherit_native_identity(previous, scale);
        }
        let content = Arc::new(content);
        let deferred = self.platform_window.supports_deferred_scroll()
            && self.scene_animation.is_none()
            && target.placement(&content, viewport, scale, Point::default())
                .is_some_and(|placement| self.next_frame.painter.push_deferred_scroll(placement));
        if !deferred && !matches!(target, ScrollPaintTarget::BufferedRow { visible: false }) {
            content.replay(&mut self.next_frame.painter, viewport.scale(scale), Point::default());
        }
        content
    }
}

impl ScrollPaint {
    fn new(origin: Point<Pixels>, coverage: Bounds<ScaledPixels>, mut operations: Operations) -> Self {
        operations.finish();
        // Qualify once, not on each scrolled frame or GPU owner request.
        let mut atlas_tiles = collections::FxHashSet::default();
        let mut has_lcd_glyphs = false;
        let mut has_paths = false;
        let mut order = super::frame::StaticPaintOrder::default();
        let mut static_order = true;
        let mut scoped = super::frame::StaticPaintOrder::default();
        let mut scoped_order = scoped.accept(PaintOperationRef::StartLayer(&coverage));
        let mut visual: Option<PaintBlockBounds> = None;
        let mut replayable = operations.len() != 0
            && operations.range_is_replayable(0..operations.len());
        let mut raster_eligible = replayable;
        // Residency belongs to the frozen source even when GPU raster
        // admission fails. Do not stop gathering dependencies at the first
        // unsupported operation: later offscreen images can scroll into view.
        for operation in operations.views() {
            replayable &= matches!(operation,
                PaintOperationRef::StartLayer(_) | PaintOperationRef::EndLayer |
                PaintOperationRef::Primitive(PrimitiveRef::Quad(_) | PrimitiveRef::Shadow(_)
                    | PrimitiveRef::Underline(_) | PrimitiveRef::MonochromeSprite(_)
                    | PrimitiveRef::PolychromeSprite(_) | PrimitiveRef::Path(_)));
            match operation {
                PaintOperationRef::Primitive(PrimitiveRef::MonochromeSprite(sprite)) => {
                    has_lcd_glyphs |= sprite.tile.texture_id.kind == crate::AtlasTextureKind::Subpixel;
                    atlas_tiles.insert((sprite.tile.texture_id, sprite.tile.tile_id.0));
                }
                PaintOperationRef::Primitive(PrimitiveRef::PolychromeSprite(sprite)) => {
                    atlas_tiles.insert((sprite.tile.texture_id, sprite.tile.tile_id.0));
                }
                PaintOperationRef::Primitive(PrimitiveRef::Path(_)) => has_paths = true,
                _ => {}
            }
            raster_eligible = raster_eligible
                && super::frame::static_scroll_raster_operation(operation, coverage);
            static_order = static_order && order.accept(operation);
            scoped_order = scoped_order && scoped.accept(operation);
            if replayable
                && let PaintOperationRef::Primitive(primitive) = operation
            {
                let bounds = primitive.visual_bounds();
                if !bounds.is_empty() {
                    let bounds = PaintBlockBounds::new(bounds);
                    visual = Some(visual.map_or(bounds, |old| old.union(bounds)));
                }
            }
        }
        let block_bounds = if operations.len() >= MIN_INDEXED_OPERATIONS {
            Self::blocks(&operations).map(Self::block_bounds).collect()
        } else {
            Box::default()
        };
        let static_order = raster_eligible && static_order && order.complete();
        let scoped_order = static_order && scoped_order && scoped.accept(PaintOperationRef::EndLayer) && scoped.complete();
        let row_raster_bounds = if replayable {
            visual.and_then(|visual| compact_row_raster(visual, coverage))
        } else { None };
        Self {
            origin,
            coverage,
            operations: Arc::new(operations),
            replayable,
            static_order,
            scoped_order,
            row_raster_bounds,
            native_id: std::sync::OnceLock::new(),
            native_generation: 1,
            native_anchor: origin,
            atlas_tiles: atlas_tiles.into_iter().collect::<Vec<_>>().into(),
            has_lcd_glyphs,
            has_paths,
            block_bounds,
        }
    }

    /// Keep the original content-space anchor across integral recaptures.
    /// LayerFrame proves damage and common coverage before adopting pixels.
    fn inherit_native_identity(&mut self, previous: &Self, scale: f32) -> bool {
        let offset = self.origin.scale(scale) - previous.native_anchor.scale(scale);
        if !self.static_order || !previous.static_order || !scale.is_finite() || scale <= 0.
            || [offset.x.0, offset.y.0].into_iter().any(|v| !v.is_finite() || v.fract() != 0. || v.abs() > 1_048_576.)
        {
            return false;
        }
        let (Some(&id), Some(generation)) = (previous.native_id.get(), previous.native_generation.checked_add(1))
            else { return false; };
        if self.native_id.set(id).is_err() { return false; }
        self.native_generation = generation;
        self.native_anchor = previous.native_anchor;
        true
    }

    pub(super) fn collect_atlas_tile_ids_into(&self, live: &mut collections::FxHashSet<(crate::AtlasTextureId, u32)>) {
        live.extend(self.atlas_tiles.iter().copied());
    }

    fn blocks(operations: &Operations) -> impl Iterator<Item = crate::scene::OperationRun<'_>> {
        operations.all().runs().flat_map(|run| {
            (0..run.len()).step_by(OPERATIONS_PER_BLOCK)
                .map(move |start| run.range(start..(start + OPERATIONS_PER_BLOCK).min(run.len())))
        })
    }

    fn block_bounds(operations: crate::scene::OperationRun<'_>) -> Option<PaintBlockBounds> {
        let mut union: Option<PaintBlockBounds> = None;
        for operation in operations.iter() {
            // Always replay scope markers in painter order, even when the
            // block's visible primitives are all clipped.
            let PaintOperationRef::Primitive(primitive) = operation else {
                return None;
            };
            let bounds = primitive.visual_bounds();
            if ![bounds.left(), bounds.right(), bounds.top(), bounds.bottom()]
                .into_iter()
                .all(|edge| edge.0.is_finite())
            {
                return None;
            }
            // Degenerate geometry must keep the original per-operation
            // intersection semantics; it cannot prove a fully visible block.
            if bounds.is_empty() {
                return None;
            }
            let bounds = PaintBlockBounds::new(bounds);
            union = Some(union.map_or(bounds, |previous| previous.union(bounds)));
        }
        union
    }

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

    /// Native layers are named by the immutable source lifetime and use
    /// its original physical coordinates as content space. No enclosing
    /// scroll container identity or measured paint range is required.
    pub(super) fn native_placement(self: &Arc<Self>, viewport: ContentMask<Pixels>,
        scale: f32, delta: Point<ScaledPixels>) -> Option<ScrollPlacement> {
        self.static_order.then_some(())?;
        self.placement(viewport, scale, delta)
    }

    /// Logical placement can be replayed by the render owner even when the
    /// source order cannot be flattened into a raster tile.
    pub(super) fn placement(self: &Arc<Self>, viewport: ContentMask<Pixels>,
        scale: f32, delta: Point<ScaledPixels>) -> Option<ScrollPlacement> {
        if viewport.corner_radii != Default::default() {
            return None;
        }
        self.placement_in_viewport(viewport, scale, delta, true)
    }

    fn placement_in_viewport(self: &Arc<Self>, viewport: ContentMask<Pixels>,
        scale: f32, delta: Point<ScaledPixels>, visible: bool) -> Option<ScrollPlacement> {
        if !self.replayable || !scale.is_finite() || scale <= 0.
            || [delta.x.0, delta.y.0].into_iter().any(|v| v.abs() > 1_048_576.)
        {
            return None;
        }
        let viewport = viewport.scale(scale);
        if (visible && !self.covers(viewport.bounds, delta))
            || ![delta.x.0, delta.y.0].into_iter().all(|v| v.is_finite() && v.fract() == 0.)
        {
            return None;
        }
        let offset = self.origin.scale(scale) - self.native_anchor.scale(scale);
        let translation = offset + delta;
        if [offset.x.0, offset.y.0, translation.x.0, translation.y.0].into_iter()
            .any(|v| !v.is_finite() || v.fract() != 0. || v.abs() > 1_048_576.) {
            return None;
        }
        let id = *self.native_id.get_or_init(super::frame::next_layer_id);
        Some(ScrollPlacement {
            source: Arc::clone(self),
            viewport,
            delta,
            buffered_row: false,
            visible,
            native: NativeScrollGeometry {
                id,
                generation: self.native_generation,
                scale,
                source_origin: [offset.x.0 as i32, offset.y.0 as i32],
                row_bounds: None,
            },
        })
    }

    /// A row owns a compact, fixed raster rectangle rather than one
    /// viewport-sized strip for every visible item. Only placement and
    /// clipping change when the row crosses the edge of the list.
    pub(super) fn native_row_placement(self: &Arc<Self>, viewport: ContentMask<Pixels>,
        scale: f32, delta: Point<ScaledPixels>) -> Option<ScrollPlacement> {
        let row_bounds = self.row_raster_bounds?;
        let source_viewport = viewport.scale(scale);
        let visible = moved(row_bounds, delta).intersect(&source_viewport.bounds);
        if visible.is_empty() {
            return None;
        }
        let viewport = if viewport.corner_radii == Default::default() {
            viewport
        } else if inside_rounded_clip_body(source_viewport, visible) {
            // In this band the rounded mask is fully opaque at every
            // fragment sample. Edge rows keep the original per-primitive clip.
            ContentMask::new(viewport.bounds)
        } else {
            return None;
        };
        let mut placement = self.native_placement(viewport, scale, delta)?;
        placement.native.row_bounds = Some(row_bounds);
        Some(placement)
    }

    /// Retain an overscan row's marker even while it contributes no visible
    /// pixels. Owner replay preserves row order as it enters the viewport.
    pub(super) fn buffered_row_placement(
        self: &Arc<Self>,
        viewport: ContentMask<Pixels>,
        scale: f32,
        delta: Point<ScaledPixels>,
        visible: bool,
    ) -> Option<ScrollPlacement> {
        let row_bounds = self.row_raster_bounds?;
        // Rounded list clips are fixed; the owner reapplies the mask to
        // each primitive when a row's own rectangular clips move.
        // Hidden rows have no projected pixels or native composition
        // slot. Keep their marker canonical until they become visible.
        // Their logical origin and input records still move in the Frame.
        let delta = if visible { delta } else { Point::default() };
        let mut placement = self.placement_in_viewport(viewport, scale, delta, visible)?;
        placement.native.row_bounds = Some(row_bounds);
        placement.buffered_row = true;
        Some(placement)
    }

    pub(super) fn supports_buffered_row(&self, viewport: ContentMask<Pixels>, scale: f32) -> bool {
        self.row_raster_bounds.is_some() && self.covers(viewport.scale(scale).bounds, Point::default())
    }

    pub(super) fn replay(
        &self,
        painter: &mut Painter,
        viewport: ContentMask<ScaledPixels>,
        delta: Point<ScaledPixels>,
    ) {
        let mut layers = SmallVec::<[bool; 8]>::new();
        let source_viewport = Bounds::new(viewport.bounds.origin - delta, viewport.bounds.size);
        let viewport_edges = PaintBlockBounds::new(source_viewport);
        let nonempty_viewport = !source_viewport.is_empty()
            && [viewport_edges.left, viewport_edges.top, viewport_edges.right, viewport_edges.bottom]
                .into_iter().all(|edge| edge.0.is_finite());
        for (index, block) in Self::blocks(&self.operations).enumerate() {
            let mut fully_visible = false;
            if let Some(Some(bounds)) = self.block_bounds.get(index) {
                if !bounds.intersects(viewport_edges) {
                    continue;
                }
                fully_visible = nonempty_viewport && bounds.contained_by(viewport_edges);
            }
            for operation in block.iter() {
                match operation {
                    PaintOperationRef::StartLayer(bounds) => {
                        let bounds = moved(*bounds, delta).intersect(&viewport.bounds);
                        layers.push(!bounds.is_empty());
                        if !bounds.is_empty() {
                            painter.push_layer(bounds);
                        }
                    }
                    PaintOperationRef::EndLayer => {
                        if layers.pop() == Some(true) {
                            painter.pop_layer();
                        }
                    }
                    PaintOperationRef::Primitive(primitive) => {
                        if fully_visible || primitive.visual_bounds().intersects(&source_viewport) {
                            painter.insert_primitive(project_primitive(primitive, delta, viewport));
                        }
                    }
                    PaintOperationRef::DeferredScroll(_) | PaintOperationRef::StartBlur(_) | PaintOperationRef::EndBlur => {
                        unreachable!("scroll capture admits only built-in unfiltered paint")
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(in crate::window) struct NativeScrollGeometry {
    pub(in crate::window) id: u64,
    pub(in crate::window) generation: u64,
    pub(in crate::window) scale: f32,
    pub(in crate::window) source_origin: [i32; 2],
    pub(in crate::window) row_bounds: Option<Bounds<ScaledPixels>>,
}

/// Immutable placement, suitable for a published Paint generation.
#[derive(Clone)]
pub(crate) struct ScrollPlacement {
    source: Arc<ScrollPaint>,
    viewport: ContentMask<ScaledPixels>,
    delta: Point<ScaledPixels>,
    buffered_row: bool,
    visible: bool,
    native: NativeScrollGeometry,
}

impl ScrollPlacement {
    pub(in crate::window) fn raster_source(&self) -> (&Arc<Operations>, Bounds<ScaledPixels>,
        Point<ScaledPixels>, &Arc<[(crate::AtlasTextureId, u32)]>) {
        (&self.source.operations, self.source.coverage, self.delta, &self.source.atlas_tiles)
    }

    pub(in crate::window) fn viewport(&self) -> ContentMask<ScaledPixels> { self.viewport }

    pub(in crate::window) fn native_geometry(&self) -> NativeScrollGeometry { self.native }

    pub(in crate::window) fn has_static_order(&self) -> bool {
        self.visible && self.source.static_order && self.viewport.corner_radii == Default::default()
    }

    pub(in crate::window) fn has_lcd_glyphs(&self) -> bool {
        self.source.has_lcd_glyphs
    }

    pub(crate) fn has_scoped_order(&self) -> bool { self.has_static_order() && self.source.scoped_order }

    pub(crate) fn is_visible(&self) -> bool { self.visible }

    pub(crate) fn can_defer_in_scope(&self) -> bool {
        self.buffered_row || self.native.row_bounds.is_none() || self.has_scoped_order()
    }

    /// A changed translation is a new paint, but does not change the
    /// immutable content, clip, native source identity or draw topology.
    pub(crate) fn same_source_placement_domain(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
            && self.viewport == other.viewport
            && self.native == other.native
            && self.buffered_row == other.buffered_row
    }

    pub(crate) fn same_paint(&self, other: &Self) -> bool {
        self.same_source_placement_domain(other) && self.delta == other.delta && self.visible == other.visible
    }

    /// The viewport is a conservative damage bound for every projected
    /// primitive, including a newly exposed part of the overscan.
    pub(crate) fn is_row_placement(&self) -> bool {
        self.native.row_bounds.is_some()
    }

    pub(crate) fn visual_bounds(&self) -> Bounds<ScaledPixels> {
        if !self.visible {
            return Bounds::new(self.viewport.bounds.origin, crate::size(ScaledPixels(0.), ScaledPixels(0.)));
        }
        self.native.row_bounds
            .map_or(self.viewport.bounds, |row| moved(row, self.delta).intersect(&self.viewport.bounds))
    }

    pub(crate) fn collect_atlas_tile_ids_into(&self, live: &mut collections::FxHashSet<(crate::AtlasTextureId, u32)>) {
        self.source.collect_atlas_tile_ids_into(live);
    }

    pub(crate) fn replay(&self, painter: &mut Painter) {
        if self.visible {
            self.source.replay(painter, self.viewport, self.delta);
        }
    }
}

#[cfg(all(test, target_os = "windows", any(feature = "nova-gfx-dx11", feature = "nova-gfx-dx12", feature = "nova-gfx-vulkan", feature = "nova-gfx-opengl")))]
pub(crate) fn native_scroll_fixture(operations: Operations, coverage: Bounds<ScaledPixels>,
    viewport: ContentMask<Pixels>) -> ScrollPlacement {
    Arc::new(ScrollPaint::new(Point::default(), coverage, operations))
        .native_placement(viewport, 1., Point::default()).expect("certified native scroll source")
}

/// Conservative AA guard, rounded out once in the source coordinate
/// system. Never sample outside the captured coverage. Bound texture size
/// independently of the list viewport and fall back for oversized rows.
fn compact_row_raster(visual: PaintBlockBounds, coverage: Bounds<ScaledPixels>) -> Option<Bounds<ScaledPixels>> {
    let left = (visual.left.0 - 2.).floor().max(coverage.left().0.ceil());
    let top = (visual.top.0 - 2.).floor().max(coverage.top().0.ceil());
    let right = (visual.right.0 + 2.).ceil().min(coverage.right().0.floor());
    let bottom = (visual.bottom.0 + 2.).ceil().min(coverage.bottom().0.floor());
    if ![left, top, right, bottom].into_iter().all(|v| v.is_finite() && v.abs() <= 1_048_576.)
        || right <= left || bottom <= top || right - left > 4096. || bottom - top > 512.
    {
        return None;
    }
    // Rounding inward at a fractional capture edge would drop pixels.
    if left > visual.left.0 || top > visual.top.0 || right < visual.right.0 || bottom < visual.bottom.0 {
        return None;
    }
    Some(Bounds::new(point(ScaledPixels(left), ScaledPixels(top)),
        crate::size(ScaledPixels(right - left), ScaledPixels(bottom - top))))
}

fn inside_rounded_clip_body(
    mask: ContentMask<ScaledPixels>,
    visible: Bounds<ScaledPixels>,
) -> bool {
    let bounds = mask.corner_bounds;
    let corners = mask.corner_radii;
    if [corners.top_left, corners.top_right, corners.bottom_right, corners.bottom_left]
        .into_iter().any(|radius| !radius.0.is_finite() || radius.0 < 0.)
    {
        return false;
    }
    let left = bounds.left();
    let right = bounds.right();
    let guard = ScaledPixels(1.);
    // Integral vertical sides leave at least half a pixel of SDF coverage
    // at the nearest fragment center. A fractional edge keeps a full guard.
    let left = if left.0.fract() == 0. { left } else { left + guard };
    let right = if right.0.fract() == 0. { right } else { right - guard };
    visible.left() >= left
        && visible.right() <= right
        && visible.top() >= bounds.top() + corners.top_left.max(corners.top_right) + guard
        && visible.bottom() <= bounds.bottom() - corners.bottom_left.max(corners.bottom_right) - guard
}

#[cfg(test)]
pub(crate) fn native_row_fixture(operations: Operations, coverage: Bounds<ScaledPixels>,
    viewport: ContentMask<Pixels>, delta: Point<ScaledPixels>) -> ScrollPlacement {
    Arc::new(ScrollPaint::new(Point::default(), coverage, operations))
        .native_row_placement(viewport, 1., delta).expect("compact native row source")
}

fn moved(bounds: Bounds<ScaledPixels>, delta: Point<ScaledPixels>) -> Bounds<ScaledPixels> {
    Bounds::new(bounds.origin + delta, bounds.size)
}

fn project_primitive(
    primitive: PrimitiveRef<'_>,
    delta: Point<ScaledPixels>,
    viewport: ContentMask<ScaledPixels>,
) -> Primitive {
    // Adapted from GPUI Fast's move_primitive. Local clip bounds move with
    // the content, while the list/view viewport stays fixed.
    let mask = |mut mask: ContentMask<ScaledPixels>| {
        mask.bounds = moved(mask.bounds, delta).intersect(&viewport.bounds);
        if mask.corner_radii != Default::default() {
            mask.corner_bounds = moved(mask.corner_bounds, delta);
        } else {
            mask.corner_bounds = viewport.corner_bounds;
            mask.corner_radii = viewport.corner_radii;
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
        PrimitiveRef::Quad(quad) => project!(quad, Quad),
        PrimitiveRef::Underline(underline) => project!(underline, Underline),
        PrimitiveRef::MonochromeSprite(sprite) => project!(sprite, MonochromeSprite),
        PrimitiveRef::PolychromeSprite(sprite) => project!(sprite, PolychromeSprite),
        PrimitiveRef::Shadow(shadow) => project!(shadow, Shadow),
        PrimitiveRef::Path(path) => {
            let mut projected = path.clone().transform_uniform(1., delta);
            projected.content_mask = mask(path.content_mask);
            Primitive::Path(projected)
        }
        _ => unreachable!("unsupported primitive in built-in scroll capture"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::PaintOperation;
    use crate::{Quad, px, size};

    #[test]
    fn retained_path_row_moves_vertices_and_keeps_fixed_viewport() {
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(40.))));
        let coverage = Bounds::new(point(px(0.), px(-40.)), size(px(40.), px(120.))).scale(1.);
        let mut path = crate::Path::new(point(px(2.), px(12.)));
        path.push_triangle(
            (point(px(2.), px(12.)), point(px(6.), px(12.)), point(px(2.), px(18.))),
            (point(0., 0.), point(1., 0.), point(0., 1.)),
        );
        let mut path = path.scale(1.);
        path.content_mask = ContentMask::new(coverage);
        let mut operations = Operations::default();
        operations.push(PaintOperation::Primitive(Primitive::Path(path.clone())));
        let source = Arc::new(ScrollPaint::new(Point::default(), coverage, operations));
        let delta = point(ScaledPixels(0.), ScaledPixels(-8.));
        let placement = ScrollPaintTarget::Row.placement(&source, viewport, 1., delta).unwrap();
        assert!(!placement.has_static_order());
        let mut painter = Painter::default();
        placement.replay(&mut painter);
        painter.finish();
        let scene = painter.into_scene();
        let projected = scene.paths.iter().next().unwrap();
        assert_eq!(projected.content_mask, viewport.scale(1.));
        for (before, after) in path.vertices.iter().zip(projected.vertices.iter()) {
            assert_eq!(after.xy_position, before.xy_position + delta);
            assert_eq!(after.st_position, before.st_position);
        }

        // A fixed path such as a chart can share the root with the moving row.
        let initial = ScrollPaintTarget::Row.placement(&source, viewport, 1., Point::default()).unwrap();
        let mut painter = Painter::for_frame();
        painter.insert_primitive(path);
        assert!(painter.push_deferred_scroll(initial));
        painter.finish();
        let previous = painter.into_scene();
        let next = previous.with_scroll_placements(&[(1, placement)]).unwrap();
        assert_eq!(previous.paths, next.paths);
        assert!(Arc::ptr_eq(&previous.paths[0].vertices, &next.paths[0].vertices));

        let outside_coverage = point(ScaledPixels(0.), ScaledPixels(-48.));
        let hidden = source.buffered_row_placement(viewport, 1., outside_coverage, false).unwrap();
        assert!(hidden.visual_bounds().is_empty());
        let hidden_again = source.buffered_row_placement(viewport, 1., Point::default(), false).unwrap();
        assert!(hidden.same_paint(&hidden_again));
        assert!(source.buffered_row_placement(viewport, 1., outside_coverage, true).is_none());
    }

    #[test]
    fn block_edges_keep_the_far_corner_without_reconstructing_width() {
        let bounds = |x, width| Bounds::new(point(px(x), px(0.)), size(px(width), px(1.))).scale(1.);
        let left = PaintBlockBounds::new(bounds(-16_777_216., 1.));
        let right = PaintBlockBounds::new(bounds(0.5, 0.5));
        let union = left.union(right);
        assert!(union.intersects(PaintBlockBounds::new(bounds(0.5, 0.25))));
        assert!(!union.contained_by(right));
    }

    #[test]
    fn compact_row_placement_inherits_flat_root_proof_when_bounds_move() {
        let coverage = Bounds::new(point(px(0.), px(-64.)), size(px(40.), px(192.))).scale(1.);
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))));
        let quad = Quad {
            bounds: Bounds::new(point(px(0.), px(16.)), size(px(40.), px(16.))).scale(1.),
            content_mask: ContentMask::new(coverage), ..Default::default()
        };
        let mut ops = Operations::default();
        ops.push(PaintOperation::Primitive(Primitive::Quad(quad)));
        let source = Arc::new(ScrollPaint::new(Point::default(), coverage, ops));
        let old = source.native_row_placement(viewport, 1., Point::default()).unwrap();
        let moved = source.native_row_placement(viewport, 1., point(ScaledPixels(0.), ScaledPixels(-4.))).unwrap();
        assert_ne!(old.visual_bounds(), moved.visual_bounds());
        let mut painter = Painter::for_frame();
        assert!(painter.push_deferred_scroll(old.clone()));
        painter.finish();
        let mut scene = painter.into_scene();
        let mut retention = crate::window::retained::RetainedPaint::default();
        retention.finish(&mut scene);
        let mut paint = crate::window::Paint::new(scene, Arc::new(retention), Default::default());
        paint.layers = crate::window::SceneLayers::capture(&paint.scene, None, true);
        let direct = paint.layers.with_scroll_placements_checked(&[(0, moved.clone())], true, true)
            .expect("flat row without LCD underlay inherits root proof");
        let next = paint.with_scroll_placement(0, moved.clone(), true).expect("row marker moves");
        assert_eq!(direct.frames.len(), 1);
        assert!(Arc::ptr_eq(&direct.frames[0].content, &paint.layers.frames[0].content));
        let rebuilt = crate::window::SceneLayers::capture(&next.scene, Some(&paint.layers), true);
        assert_eq!(direct.frames[0].root_static_raster(), rebuilt.frames[0].root_static_raster());
        assert_eq!(direct.frames[0].root_static_raster(), next.layers.frames[0].root_static_raster());
        assert!(matches!(paint.scene.paint_operations.view(0), Some(PaintOperationRef::DeferredScroll(marker)) if marker.same_paint(&old)));
        assert!(matches!(next.scene.paint_operations.view(0), Some(PaintOperationRef::DeferredScroll(marker)) if marker.same_paint(&moved)));
        assert!(next.scene.paint_operations.shares_boundary_storage(&paint.scene.paint_operations));
        let expanded = next.owner_fallback().unwrap().unwrap();
        assert_eq!(expanded.scene.quads[0].bounds.origin.y, px(12.).scale(1.));
    }

    #[test]
    fn buffered_row_visibility_splices_flat_layers_against_full_capture() {
        let coverage = Bounds::new(point(px(0.), px(-64.)), size(px(40.), px(192.))).scale(1.);
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))));
        let source = |y| {
            let mut ops = Operations::default();
            ops.push(PaintOperation::Primitive(Primitive::Quad(Quad {
                bounds: Bounds::new(point(px(0.), px(y)), size(px(40.), px(16.))).scale(1.),
                content_mask: ContentMask::new(coverage), ..Default::default()
            })));
            Arc::new(ScrollPaint::new(Point::default(), coverage, ops))
        };
        let sources = [source(0.), source(16.), source(64.), source(80.)];
        let old = sources.iter().enumerate().map(|(i, source)| {
            source.buffered_row_placement(viewport, 1., Point::default(), i < 2).unwrap()
        }).collect::<Vec<_>>();
        let mut painter = Painter::for_frame();
        for placement in &old {
            assert!(painter.push_deferred_scroll(placement.clone()));
        }
        painter.finish();
        let mut scene = painter.into_scene();
        let mut retention = crate::window::retained::RetainedPaint::default();
        retention.finish(&mut scene);
        let mut paint = crate::window::Paint::new(scene, Arc::new(retention), Default::default());
        paint.layers = crate::window::SceneLayers::capture(&paint.scene, None, true);
        let delta = point(ScaledPixels(0.), ScaledPixels(-16.));
        let replacements = (0..3).map(|i| {
            (i, sources[i].buffered_row_placement(viewport, 1., delta, i != 0).unwrap())
        }).collect::<Vec<_>>();
        let direct = paint.layers.with_row_visibility_delta(&replacements, true)
            .expect("flat boundary splice");
        let next = paint.with_scroll_placements(&replacements, true).unwrap();
        let rebuilt = crate::window::SceneLayers::capture(&next.scene, Some(&paint.layers), true);
        assert_eq!(direct.frames.len(), 2);
        assert_eq!(direct.frames.len(), rebuilt.frames.len());
        for (actual, expected) in direct.frames.iter().zip(&rebuilt.frames) {
            assert_eq!(actual.id, expected.id);
            assert_eq!(actual.generation, expected.generation);
            assert_eq!(actual.root_static_span(), expected.root_static_span());
            assert_eq!(actual.root_static_raster(), expected.root_static_raster());
            assert_eq!(actual.native_row_raster(), expected.native_row_raster());
        }
        assert!(Arc::ptr_eq(&direct.frames[0].content, &paint.layers.frames[1].content));
        assert!(Arc::ptr_eq(&next.layers.frames[0].content, &paint.layers.frames[1].content));
        assert!(matches!(next.scene.paint_operations.view(3), Some(PaintOperationRef::DeferredScroll(marker)) if marker.same_paint(&old[3])));
        assert!(next.scene.paint_operations.shares_boundary_storage(&paint.scene.paint_operations));
        assert!(paint.layers.with_row_visibility_delta(&replacements, false).is_none());
    }

    #[test]
    fn direct_placement_publication_keeps_old_paint_and_draw_allocations_live() {
        let coverage = Bounds::new(point(px(0.), px(-64.)), size(px(40.), px(192.))).scale(1.);
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))));
        let quad = Quad {
            bounds: Bounds::new(point(px(0.), px(16.)), size(px(40.), px(16.))).scale(1.),
            content_mask: ContentMask::new(coverage), ..Default::default()
        };
        let mut ops = Operations::default();
        ops.push(PaintOperation::Primitive(Primitive::Quad(quad)));
        let mut sibling_ops = Operations::default();
        sibling_ops.push(PaintOperation::Primitive(Primitive::Quad(quad)));
        let sibling_source = Arc::new(ScrollPaint::new(Point::default(), coverage, sibling_ops));
        let sibling = sibling_source.native_placement(viewport, 1., Point::default()).unwrap();
        let sibling_moved = sibling_source.native_placement(viewport, 1., point(ScaledPixels(0.), ScaledPixels(-8.))).unwrap();
        let source = Arc::new(ScrollPaint::new(Point::default(), coverage, ops));
        let first = source.native_placement(viewport, 1., Point::default()).unwrap();
        let moved = source.native_placement(viewport, 1., point(ScaledPixels(0.), ScaledPixels(-16.))).unwrap();
        let mut painter = Painter::for_frame();
        painter.insert_primitive(quad);
        assert!(painter.push_deferred_scroll(first.clone()));
        painter.insert_primitive(quad);
        assert!(painter.push_deferred_scroll(sibling.clone()));
        painter.finish();
        let mut old = painter.into_scene();
        let mut retention = crate::window::retained::RetainedPaint::default();
        retention.finish(&mut old);
        let mut publication = crate::window::Paint::new(old, Arc::new(retention), Default::default());
        publication.layers = crate::window::SceneLayers::capture(&publication.scene, None, true);
        let publication = Arc::new(publication);
        let batch = publication.with_scroll_placements(&[(1, moved.clone()), (3, sibling_moved.clone())], true)
            .expect("two certified markers");
        assert_eq!(batch.scene.paint_operations.deferred_scroll_indices().collect::<Vec<_>>(), vec![1, 3]);
        assert!(batch.scene.paint_operations.shares_boundary_storage(&publication.scene.paint_operations));
        assert!(matches!(batch.scene.paint_operations.view(1), Some(PaintOperationRef::DeferredScroll(new)) if new.same_paint(&moved)));
        assert!(matches!(batch.scene.paint_operations.view(3), Some(PaintOperationRef::DeferredScroll(new)) if new.same_paint(&sibling_moved)));
        assert!(matches!(publication.scene.paint_operations.view(3), Some(PaintOperationRef::DeferredScroll(old)) if old.same_paint(&sibling)));
        assert!(std::ptr::eq(&batch.scene.quads[0], &publication.scene.quads[0]));
        assert!(publication.with_scroll_placements(&[(1, moved.clone()), (1, moved.clone())], true).is_none());
        assert!(publication.with_scroll_placements(&[(1, moved.clone()), (2, sibling_moved.clone())], true).is_none());
        let next = publication.with_scroll_placement(1, moved.clone(), true).unwrap();
        let direct = publication.layers.with_scroll_placement(1, &moved, true)
            .expect("certified marker inherits its proof even with siblings");
        let rebuilt = crate::window::SceneLayers::capture(&next.scene, Some(&publication.layers), true);
        assert_eq!(direct.frames.len(), 2);
        for (direct, rebuilt) in direct.frames.iter().zip(&rebuilt.frames) {
            assert_eq!(direct.root_static_raster(), rebuilt.root_static_raster());
            assert_eq!(direct.id, rebuilt.id);
            assert_eq!(direct.generation, rebuilt.generation);
            assert!(Arc::ptr_eq(&direct.content, &rebuilt.content));
        }
        assert!(publication.layers.with_scroll_placement(0, &moved, true).is_none());
        assert!(publication.layers.with_scroll_placement(1, &moved, false).is_none());
        assert_ne!(next.scene.revision, publication.scene.revision);
        assert_eq!(next.scene.len(), publication.scene.len());
        assert!(next.scene.paint_operations.shares_boundary_storage(&publication.scene.paint_operations));
        assert_eq!(next.scene.paint_operations.placement_storage_matches(&publication.scene.paint_operations), Some(false));
        assert_eq!(next.scene.paint_operations.deferred_scroll_indices().collect::<Vec<_>>(), vec![1, 3]);
        assert!(std::ptr::eq(&next.scene.quads[0], &publication.scene.quads[0]));
        assert!(std::ptr::eq(&next.scene.quads[1], &publication.scene.quads[1]));
        assert!(matches!(publication.scene.paint_operations.view(1), Some(PaintOperationRef::DeferredScroll(old)) if old.same_paint(&first)));
        assert!(matches!(next.scene.paint_operations.view(1), Some(PaintOperationRef::DeferredScroll(new)) if new.same_paint(&moved)));
        assert!(matches!(next.scene.paint_operations.view(3), Some(PaintOperationRef::DeferredScroll(unchanged)) if unchanged.same_paint(&sibling)));
        assert!(Arc::ptr_eq(&next.layers.frames[0].content, &publication.layers.frames[0].content));
        assert!(next.with_scroll_placement(0, moved.clone(), true).is_none());
        assert!(next.with_scroll_placement(1, moved, true).is_none());
        drop(publication);
        assert_eq!(next.scene.quads.len(), 2);
    }

    #[test]
    fn placement_only_finish_carries_retained_gpu_chunk_qualification() {
        use crate::window::retained::RetainedPaint;
        let coverage = Bounds::new(point(px(0.), px(-64.)), size(px(40.), px(192.))).scale(1.);
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))));
        let quad = Quad {
            bounds: Bounds::new(point(px(0.), px(16.)), size(px(40.), px(16.))).scale(1.),
            content_mask: ContentMask::new(coverage), ..Default::default()
        };
        let mut operations = Operations::default();
        operations.push(PaintOperation::Primitive(Primitive::Quad(quad)));
        let source = Arc::new(ScrollPaint::new(Point::default(), coverage, operations));
        let first = source.native_placement(viewport, 1., Point::default()).unwrap();
        let moved = source.native_placement(viewport, 1., point(ScaledPixels(0.), ScaledPixels(-16.))).unwrap();
        let mut old = Painter::for_frame();
        for _ in 0..32 { old.insert_primitive(quad); }
        assert!(old.push_deferred_scroll(first));
        old.finish();
        let mut old_retention = RetainedPaint::default();
        old_retention.record(crate::GlobalElementId::from_path(&["placement".into()]), 1, 0..32);
        old_retention.finish(&mut old.scene);
        assert_eq!(old_retention.chunks_for(&old.scene).len(), 1);
        let mut current = Painter::for_frame();
        assert!(current.replay_with_ordering(0..32, &old.scene,
            old_retention.replay_quads(&old.scene, 0..32), Some(&old)));
        assert!(current.push_scroll_placement_against(&old.scene, 32..33, moved));
        let proof = current.finish_against_with_storage(&old.scene);
        assert_eq!(proof, (false, true));
        let mut retention = RetainedPaint::default();
        retention.replay(0..32, &old_retention, 0);
        retention.finish_against(&mut current.scene, &old.scene, &old_retention, proof.0, proof.1);
        let old_chunk = &old_retention.chunks_for(&old.scene)[0];
        let new_chunk = &retention.chunks_for(&current.scene)[0];
        assert_eq!(new_chunk.id, old_chunk.id);
        assert_eq!(new_chunk.quad_range, old_chunk.quad_range);
        assert!(new_chunk.replayed);
        assert!(!old_chunk.replayed);
        assert!(retention.chunks_for(&old.scene).is_empty());
    }

    #[test]
    fn native_marker_run_replay_shares_slots_and_replaces_only_changed_marker() {
        let coverage = Bounds::new(point(px(0.), px(-64.)), size(px(40.), px(192.))).scale(1.);
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))));
        let mut operations = Operations::default();
        operations.push(PaintOperation::Primitive(Primitive::Quad(Quad {
            bounds: Bounds::new(point(px(0.), px(16.)), size(px(40.), px(16.))).scale(1.),
            content_mask: ContentMask::new(coverage), ..Default::default()
        })));
        let source = Arc::new(ScrollPaint::new(Point::default(), coverage, operations));
        let placement = source.native_placement(viewport, 1., Point::default()).unwrap();
        let moved = source.native_placement(viewport, 1., point(ScaledPixels(0.), ScaledPixels(-16.))).unwrap();
        let mut old = Painter::for_frame();
        for _ in 0..64 { assert!(old.push_deferred_scroll(placement.clone())); }
        old.finish();
        let mut current = Painter::for_frame();
        assert!(current.replay_with_ordering(0..32, &old.scene, [], Some(&old)));
        assert!(current.push_scroll_placement_against(&old.scene, 32..33, moved));
        assert!(current.replay_with_ordering(33..64, &old.scene, [], Some(&old)));
        current.finish_against(&old.scene);
        assert_eq!(current.len(), 64);
        for index in (0..32).chain(33..64) {
            assert_eq!(current.paint_operations.storage_identity(index), old.paint_operations.storage_identity(index));
        }
        assert_ne!(current.paint_operations.storage_identity(32), old.paint_operations.storage_identity(32));
        assert_eq!(current.paint_operations.placement_storage_matches(&old.paint_operations), Some(false));
        let mut unchanged = Painter::for_frame();
        assert!(unchanged.push_scroll_placement_against(&old.scene, 0..1, placement.clone()));
        assert_eq!(unchanged.paint_operations.storage_identity(0), old.paint_operations.storage_identity(0));
        // Eager owner reconstruction must not use the marker-page shortcut.
        let mut eager = Painter::default();
        eager.replay(0..64, &old.scene, []);
        assert!(!eager.paint_operations.has_deferred_scroll());
    }

    #[test]
    fn changed_native_marker_keeps_both_ordinary_replay_ranges_and_batches() {
        let coverage = Bounds::new(point(px(0.), px(-64.)), size(px(40.), px(192.))).scale(1.);
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))));
        let mut operations = Operations::default();
        operations.push(PaintOperation::Primitive(Primitive::Quad(Quad {
            bounds: Bounds::new(point(px(0.), px(16.)), size(px(40.), px(16.))).scale(1.),
            content_mask: ContentMask::new(coverage), ..Default::default()
        })));
        let source = Arc::new(ScrollPaint::new(Point::default(), coverage, operations));
        let first = source.native_placement(viewport, 1., Point::default()).unwrap();
        let moved = source.native_placement(viewport, 1., point(ScaledPixels(0.), ScaledPixels(-16.))).unwrap();
        let quad = |y| Quad {
            bounds: Bounds::new(point(px(0.), px(y)), size(px(40.), px(16.))).scale(1.),
            content_mask: viewport.scale(1.), ..Default::default()
        };
        let mut old = Painter::for_frame();
        old.insert_primitive(quad(0.));
        assert!(old.push_deferred_scroll(first));
        old.insert_primitive(quad(32.));
        old.finish();
        let mut current = Painter::for_frame();
        assert!(current.replay_with_ordering(0..1, &old.scene, [], Some(&old)));
        assert!(current.push_deferred_scroll(moved));
        assert!(current.replay_with_ordering(2..3, &old.scene, [], Some(&old)));
        assert_eq!(current.paint_operations.storage_identity(0), old.paint_operations.storage_identity(0));
        assert_eq!(current.paint_operations.storage_identity(2), old.paint_operations.storage_identity(2));
        assert!(!current.finish_against(&old.scene));
        assert_ne!(current.revision, old.revision);
        assert!(std::ptr::eq(&current.quads[0], &old.quads[0]));
        assert!(std::ptr::eq(&current.quads[1], &old.quads[1]));
        assert_eq!(current.paint_operations.placement_storage_matches(&old.paint_operations), Some(false));
    }

    #[test]
    fn placement_only_variant_preserves_ordinary_pages_without_requalifying_payload() {
        let coverage = Bounds::new(point(crate::px(0.), crate::px(-64.)),
            crate::size(crate::px(40.), crate::px(192.))).scale(1.);
        let viewport = ContentMask::new(Bounds::new(point(crate::px(0.), crate::px(0.)),
            crate::size(crate::px(40.), crate::px(64.))));
        let mut source_ops = Operations::default();
        source_ops.push(crate::scene::PaintOperation::Primitive(Primitive::Quad(crate::Quad {
            bounds: coverage, content_mask: ContentMask::new(coverage), ..Default::default()
        })));
        let source = Arc::new(ScrollPaint::new(Point::default(), coverage, source_ops));
        let first = source.native_placement(viewport, 1., Point::default()).unwrap();
        let next = source.native_placement(viewport, 1., point(ScaledPixels(0.), ScaledPixels(-16.))).unwrap();
        let mut static_ops = Operations::default();
        for _ in 0..128 {
            static_ops.push(crate::scene::PaintOperation::Primitive(Primitive::Quad(crate::Quad::default())));
        }
        static_ops.finish();
        let build = |placement: ScrollPlacement| {
            let mut ops = Operations::default();
            ops.share(&static_ops, 0..64);
            ops.push(crate::scene::PaintOperation::DeferredScroll(placement));
            ops.share(&static_ops, 64..128);
            ops.finish();
            ops
        };
        let previous = build(first.clone());
        let changed = build(next.clone());
        assert_eq!(changed.placement_storage_matches(&previous), Some(false));
        assert_eq!(build(first).placement_storage_matches(&previous), Some(true));
        assert_eq!(previous.storage_identity(0), changed.storage_identity(0));
        assert_eq!(previous.storage_identity(128), changed.storage_identity(128));
        let mut unrelated = Operations::default();
        unrelated.share(&static_ops, 0..64);
        unrelated.push(crate::scene::PaintOperation::DeferredScroll(next));
        // A rebuilt ordinary payload is deliberately not a storage proof.
        unrelated.push(crate::scene::PaintOperation::Primitive(Primitive::Quad(crate::Quad::default())));
        unrelated.share(&static_ops, 65..128);
        unrelated.finish();
        assert_eq!(unrelated.placement_storage_matches(&previous), None);
    }

    #[test]
    fn transferable_placement_projects_identical_pixels_on_another_thread() {
        let bounds = |y| Bounds::new(point(px(0.), px(y)), size(px(40.), px(16.))).scale(1.);
        let coverage = Bounds::new(point(px(0.), px(-64.)), size(px(40.), px(192.))).scale(1.);
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))).scale(1.));
        let mut operations = Operations::default();
        for y in [-48., -16., 16., 48., 80.] {
            operations.push(PaintOperation::Primitive(Primitive::Quad(Quad {
                bounds: bounds(y), content_mask: ContentMask::new(coverage), ..Default::default()
            })));
        }
        let source = Arc::new(ScrollPaint::new(Point::default(), coverage, operations));
        let delta = point(ScaledPixels(0.), ScaledPixels(-32.));
        let logical_viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))));
        let placement = source.native_placement(logical_viewport, 1., delta).expect("covered integral translation");
        assert!(Arc::ptr_eq(placement.raster_source().0, &source.operations));
        let mut eager = Painter::default();
        source.replay(&mut eager, viewport, delta);
        eager.finish();
        let recovered = std::thread::spawn(move || {
            let mut painter = Painter::default();
            placement.replay(&mut painter);
            painter.finish();
            painter.into_scene()
        }).join().unwrap();
        assert!(eager.paint_operations.matches_views(&recovered.paint_operations, |left, right| left.same_paint(right)));
        assert!(source.native_placement(logical_viewport, 1., point(ScaledPixels(0.), ScaledPixels(-0.5))).is_none());
        assert!(source.native_placement(logical_viewport, 1., point(ScaledPixels(0.), ScaledPixels(-256.))).is_none());
    }

    #[test]
    fn owner_materializes_native_marker_between_ordinary_spans() {
        let bounds = |y| Bounds::new(point(px(0.), px(y)), size(px(40.), px(16.))).scale(1.);
        let coverage = Bounds::new(point(px(0.), px(-64.)), size(px(40.), px(192.))).scale(1.);
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))));
        let mut operations = Operations::default();
        operations.push(PaintOperation::Primitive(Primitive::Quad(Quad {
            bounds: bounds(48.), content_mask: ContentMask::new(coverage), ..Default::default()
        })));
        let source = Arc::new(ScrollPaint::new(Point::default(), coverage, operations));
        let moved_placement = source.native_placement(viewport, 1., point(ScaledPixels(0.), ScaledPixels(-32.))).unwrap();
        let outside = |y| Quad { bounds: bounds(y), content_mask: viewport.scale(1.), ..Default::default() };
        let mut painter = Painter::default();
        painter.insert_primitive(outside(0.));
        assert!(painter.push_deferred_scroll(moved_placement.clone()));
        painter.insert_primitive(outside(48.));
        painter.finish();
        let scene = painter.into_scene();
        let paint = crate::window::Paint::new(scene, Arc::default(), Default::default());
        let recovered = paint.owner_fallback().unwrap().expect("deferred scroll root");
        let mut eager = Painter::default();
        eager.insert_primitive(outside(0.));
        moved_placement.replay(&mut eager);
        eager.insert_primitive(outside(48.));
        eager.finish();
        assert!(recovered.scene.paint_operations.matches_views(&eager.paint_operations, |left, right| left.same_paint(right)));
        assert_eq!(recovered.scene.len(), 3);
        assert!(recovered.layers.frames.is_empty());
    }

    #[test]
    fn native_recapture_adopts_only_undamaged_tiles_in_same_coordinates() {
        let rect = |y, height| Bounds::new(point(px(0.), px(y)), size(px(40.), px(height))).scale(1.);
        let coverage = rect(-64., 192.);
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))));
        let capture = |color| {
            let mut ops = Operations::default();
            for (y, rgb) in [(8., 0x123456), (48., color)] {
                ops.push(PaintOperation::Primitive(Primitive::Quad(Quad {
                    bounds: rect(y, 8.), content_mask: ContentMask::new(coverage),
                    background: crate::rgb(rgb).into(), ..Default::default()
                })));
            }
            ScrollPaint::new(Point::default(), coverage, ops)
        };
        let publish = |source: &Arc<ScrollPaint>| {
            let mut painter = Painter::default();
            assert!(painter.push_deferred_scroll(source.native_placement(viewport, 1., Point::default()).unwrap()));
            painter.finish();
            painter.into_scene()
        };
        let original = Arc::new(capture(0x0000ff));
        let first = publish(&original);
        let old = crate::window::SceneLayers::capture(&first, None, true);
        let mut refreshed = capture(0x00ff00);
        assert!(refreshed.inherit_native_identity(&original, 1.));
        let new = publish(&Arc::new(refreshed));
        let layers = crate::window::SceneLayers::capture(&new, Some(&old), true);
        let layer = &layers.frames[0];
        assert_eq!(layer.id, old.frames[0].id);
        assert_eq!(layer.generation, 2);
        assert_eq!(layer.adopt_tile(1, [0, 0, 40, 32], 1.), Some(2));
        assert_eq!(layer.adopt_tile(1, [0, 32, 40, 64], 1.), None);
        assert_eq!(layer.adopt_tile(0, [0, 0, 40, 32], 1.), None);
        let mut relocated = capture(0x0000ff);
        relocated.origin = point(px(0.), px(0.5));
        assert!(!relocated.inherit_native_identity(&original, 1.));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn lcd_tiles_require_and_track_the_opaque_underlay() {
        let viewport = ContentMask::new(Bounds::new(Point::default(), size(px(40.), px(64.))));
        let mask = viewport.scale(1.);
        let mut operations = Operations::default();
        operations.push(PaintOperation::Primitive(Primitive::MonochromeSprite(crate::MonochromeSprite {
            order: 0,
            pad: 0,
            animation_id: None,
            bounds: mask.bounds,
            content_mask: mask,
            color: crate::rgb(0xffffff),
            tile: crate::AtlasTile {
                texture_id: crate::AtlasTextureId { index: 0, kind: crate::AtlasTextureKind::Subpixel },
                tile_id: crate::TileId(1),
                padding: 0,
                bounds: Default::default(),
            },
            transformation: crate::TransformationMatrix::unit(),
        })));
        let frozen = Arc::new(ScrollPaint::new(Point::default(), mask.bounds, operations));
        let placement = frozen.native_placement(viewport, 1., Point::default()).unwrap();
        let scene = |background: crate::Rgba, obstructed: bool| {
            let mut painter = Painter::default();
            painter.insert_primitive(Quad {
                bounds: mask.bounds, content_mask: mask, background: background.into(),
                ..Default::default()
            });
            if obstructed {
                painter.insert_primitive(Quad {
                    bounds: Bounds::new(Point::default(), size(ScaledPixels(12.), ScaledPixels(12.))),
                    content_mask: mask, background: crate::rgb(0x009900).into(),
                    ..Default::default()
                });
            }
            assert!(painter.push_deferred_scroll(placement.clone()));
            painter.finish();
            painter.into_scene()
        };
        let red = crate::rgb(0x990000);
        let blue = crate::rgb(0x000099);
        let red_scene = scene(red, false);
        let previous = crate::window::SceneLayers::capture(&red_scene, None, true);
        assert_eq!(previous.frames[0].raster_background(), Some(red));
        let stable = crate::window::SceneLayers::capture(&red_scene, Some(&previous), true);
        assert_eq!(stable.frames[0].id, previous.frames[0].id);
        let changed = crate::window::SceneLayers::capture(&scene(blue, false), Some(&previous), true);
        assert_eq!(changed.frames[0].raster_background(), Some(blue));
        assert_ne!(changed.frames[0].id, previous.frames[0].id);
        for (scene, opaque) in [(&red_scene, false), (&scene(red, true), true)] {
            let layers = crate::window::SceneLayers::capture(scene, Some(&previous), opaque);
            assert!(layers.frames[0].root_static_raster().is_none());
        }
    }

    #[test]
    fn native_recapture_inherits_translated_content_in_common_coverage() {
        let rect = |y, height| Bounds::new(point(px(0.), px(y)), size(px(40.), px(height))).scale(1.);
        let coverage = rect(-64., 192.);
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))));
        let capture = |offset, color| {
            let mut ops = Operations::default();
            for (y, rgb) in [(8., 0x123456), (48., color)] {
                ops.push(PaintOperation::Primitive(Primitive::Quad(Quad {
                    bounds: rect(y + offset, 8.), content_mask: ContentMask::new(coverage),
                    background: crate::rgb(rgb).into(), ..Default::default()
                })));
            }
            ScrollPaint::new(point(px(0.), px(offset)), coverage, ops)
        };
        let publish = |source: &Arc<ScrollPaint>| {
            let mut painter = Painter::default();
            assert!(painter.push_deferred_scroll(source.native_placement(viewport, 1., Point::default()).unwrap()));
            painter.finish();
            painter.into_scene()
        };
        let original = Arc::new(capture(0., 0x0000ff));
        let old = crate::window::SceneLayers::capture(&publish(&original), None, true);
        let mut next = capture(16., 0x00ff00);
        assert!(next.inherit_native_identity(&original, 1.));
        let layers = crate::window::SceneLayers::capture(&publish(&Arc::new(next)), Some(&old), true);
        let layer = &layers.frames[0];
        assert_eq!(layer.id, old.frames[0].id);
        assert_eq!(layer.root_static_raster(), Some((viewport.scale(1.).bounds, [0, 16])));
        assert_eq!(layer.raster_source_for_tile([0, 0, 40, 32]).unwrap().1, [0, 16]);
        assert_eq!(layer.adopt_tile(1, [0, 0, 40, 32], 1.), Some(2));
        assert_eq!(layer.adopt_tile(1, [0, 32, 40, 64], 1.), None);
        assert_eq!(layer.adopt_tile(1, [0, 96, 40, 128], 1.), None);
    }

    #[test]
    fn native_publication_reuses_content_by_identity_after_row_reorder() {
        let viewport = ContentMask::new(Bounds::new(Point::default(), size(px(40.), px(64.))));
        let mask = viewport.scale(1.);
        let make_source = || {
            let mut operations = Operations::default();
            operations.push(PaintOperation::Primitive(Primitive::Quad(Quad {
                bounds: mask.bounds, content_mask: mask, ..Default::default()
            })));
            Arc::new(ScrollPaint::new(Point::default(), mask.bounds, operations))
                .native_placement(viewport, 1., Point::default()).unwrap()
        };
        let a = make_source();
        let b = make_source();
        let publish = |placements: &[ScrollPlacement]| {
            let mut painter = Painter::default();
            for placement in placements {
                assert!(painter.push_deferred_scroll(placement.clone()));
            }
            painter.finish();
            painter.into_scene()
        };
        let old = crate::window::SceneLayers::capture(&publish(&[a.clone(), b.clone()]), None, true);
        let stable = crate::window::SceneLayers::capture(&publish(&[a.clone(), b.clone()]), Some(&old), true);
        let reordered = crate::window::SceneLayers::capture(&publish(&[b.clone(), a]), Some(&old), true);
        let removed = crate::window::SceneLayers::capture(&publish(&[b]), Some(&old), true);
        assert_eq!(old.frames.len(), 2);
        assert!(Arc::ptr_eq(&stable.frames[0].content, &old.frames[0].content));
        assert!(Arc::ptr_eq(&stable.frames[1].content, &old.frames[1].content));
        assert!(Arc::ptr_eq(&reordered.frames[0].content, &old.frames[1].content));
        assert!(Arc::ptr_eq(&reordered.frames[1].content, &old.frames[0].content));
        assert!(Arc::ptr_eq(&removed.frames[0].content, &old.frames[1].content));
    }

    #[test]
    fn native_deferred_scroll_is_one_operation_and_rebuilds_ordinary_paint() {
        let bounds = |y| Bounds::new(point(px(0.), px(y)), size(px(40.), px(16.))).scale(1.);
        let coverage = Bounds::new(point(px(0.), px(-64.)), size(px(40.), px(192.))).scale(1.);
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))).scale(1.));
        let mut operations = Operations::default();
        for y in [16., 32., 48., 64.] {
            operations.push(PaintOperation::Primitive(Primitive::Quad(Quad {
                bounds: bounds(y), content_mask: ContentMask::new(coverage), ..Default::default()
            })));
        }
        let source = Arc::new(ScrollPaint::new(Point::default(), coverage, operations));
        let logical_viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))));
        let placement = source.native_placement(logical_viewport, 1., point(ScaledPixels(0.), ScaledPixels(-16.))).unwrap();
        let outside = |y| Quad { bounds: bounds(y), content_mask: viewport, ..Default::default() };
        let mut deferred = Painter::default();
        deferred.insert_primitive(outside(0.));
        assert!(placement.has_static_order());
        assert!(deferred.push_deferred_scroll(placement.clone()));
        deferred.insert_primitive(outside(32.));
        deferred.finish();
        let mut ui_replay = Painter::default();
        ui_replay.replay_with_ordering(0..deferred.len(), &deferred.scene, std::iter::empty(), Some(&deferred));
        ui_replay.finish();
        assert_eq!(ui_replay.len(), 3);
        assert_eq!(ui_replay.quads.len(), 2);
        assert!(ui_replay.paint_operations.has_deferred_scroll());
        let scene = deferred.into_scene();
        assert_eq!(scene.len(), 3);
        assert!(scene.paint_operations.has_deferred_scroll());
        assert_eq!(scene.paint_operations.deferred_scrolls().count(), 1);
        let layers = crate::window::SceneLayers::capture(&scene, None, true);
        assert_eq!(layers.frames.len(), 1);
        assert_eq!(layers.frames[0].root_static_span(), Some(&(1..2)));
        assert_eq!(layers.frames[0].root_static_raster(), Some((viewport.bounds, [0, -16])));
        let (tile_ops, origin) = layers.frames[0].raster_source_for_tile([0, 16, 40, 80]).unwrap();
        assert!(std::ptr::eq(tile_ops, source.operations.as_ref()));
        assert_eq!(origin, [0, 16]);
        let mut moved = Painter::default();
        moved.insert_primitive(outside(0.));
        let later = source.native_placement(logical_viewport, 1., point(ScaledPixels(0.), ScaledPixels(-32.))).unwrap();
        assert!(moved.push_deferred_scroll(later));
        moved.insert_primitive(outside(32.));
        moved.finish();
        let next = crate::window::SceneLayers::capture_against(&moved.scene, Some((&scene, &layers)), true);
        assert_eq!(next.frames[0].id, layers.frames[0].id);
        assert_eq!(next.frames[0].generation, layers.frames[0].generation);
        assert!(Arc::ptr_eq(&next.frames[0].content, &layers.frames[0].content));
        // Repeated instances must not share one cache identity in the same
        // frame. Both remain recoverable by ordinary Painter expansion.
        let mut duplicate = Painter::default();
        assert!(duplicate.push_deferred_scroll(placement.clone()));
        assert!(duplicate.push_deferred_scroll(placement.clone()));
        duplicate.finish();
        assert!(crate::window::SceneLayers::capture(&duplicate.scene, None, true).frames.is_empty());
        let mut nested = Painter::default();
        nested.push_layer(viewport.bounds);
        nested.replay_with_ordering(1..2, &scene, std::iter::empty(), Some(&moved));
        nested.pop_layer();
        nested.finish();
        assert_eq!(nested.len(), 3);
        assert!(nested.paint_operations.has_deferred_scroll());
        let nested_layers = crate::window::SceneLayers::capture(&nested.scene, Some(&layers), true);
        assert_eq!(nested_layers.frames.len(), 1);
        assert_eq!(nested_layers.frames[0].root_static_span(), Some(&(1..2)));
        assert_eq!(nested_layers.frames[0].id, layers.frames[0].id);
        assert!(Arc::ptr_eq(&nested_layers.frames[0].content, &layers.frames[0].content));
        assert_eq!(nested_layers.frames[0].adopt_tile(1, [0, 16, 40, 80], 1.), Some(1));
        // An overlapping ordinary sibling shares the parent order and
        // might be reordered across the placement on eager expansion.
        let mut conflict = Painter::default();
        conflict.push_layer(viewport.bounds);
        conflict.insert_primitive(outside(0.));
        assert!(conflict.push_deferred_scroll(placement.clone()));
        conflict.pop_layer();
        conflict.finish();
        let conflict_layers = crate::window::SceneLayers::capture_against(&conflict.scene, Some((&scene, &layers)), true);
        assert_eq!(conflict_layers.frames.len(), 1);
        assert!(conflict_layers.frames[0].root_static_span().is_none());
        let nested_paint = crate::window::Paint::new(nested.into_scene(), Arc::default(), Default::default());
        assert!(!nested_paint.owner_fallback().unwrap().unwrap().scene.paint_operations.has_deferred_scroll());
        assert_eq!(scene.bounds_for_range(1..2), Some(viewport.bounds));
        let mut shared = Operations::default();
        shared.share(&scene.paint_operations, 1..2);
        assert!(shared.has_deferred_scroll());
        assert!(shared.range_is_replayable(0..1));
        shared.edit(0..1, |operation| *operation = PaintOperation::Primitive(Primitive::Quad(outside(0.))));
        assert!(!shared.has_deferred_scroll());
        shared.clear();
        assert!(!shared.has_deferred_scroll());
        let paint = crate::window::Paint::new(scene, Arc::default(), Default::default());
        let recovered = paint.owner_fallback().unwrap().expect("native deferred fallback");
        let retry = paint.owner_fallback().unwrap().expect("same generation");
        assert!(Arc::ptr_eq(&recovered, &retry));
        assert!(paint.scene.paint_operations.has_deferred_scroll());
        assert!(recovered.owner_fallback().unwrap().is_none());
        let mut eager = Painter::default();
        eager.insert_primitive(outside(0.));
        placement.replay(&mut eager);
        eager.insert_primitive(outside(32.));
        eager.finish();
        assert!(!recovered.scene.paint_operations.has_deferred_scroll());
        assert!(recovered.scene.paint_operations.matches_views(&eager.paint_operations, |left, right| left.same_paint(right)));
    }

    #[test]
    fn scroll_blocks_cull_disjoint_primitives_and_keep_paint_order() {
        let bounds = |y| Bounds::new(point(px(0.), px(y)), size(px(40.), px(1.))).scale(1.);
        let mut operations = Operations::default();
        operations.push(PaintOperation::StartLayer(bounds(0.).union(&bounds(255.))));
        for y in 0..256 {
            let bounds = bounds(y as f32);
            operations.push(PaintOperation::Primitive(Primitive::Quad(Quad {
                bounds,
                content_mask: ContentMask::new(bounds),
                ..Default::default()
            })));
        }
        operations.push(PaintOperation::EndLayer);
        let coverage = bounds(0.).union(&bounds(255.));
        let mut recorded = ScrollPaint::new(Point::default(), coverage, operations);
        assert!(recorded.block_bounds.iter().any(Option::is_some));
        let viewport = ContentMask::new(bounds(0.).union(&bounds(47.)));
        let delta = point(ScaledPixels(0.), ScaledPixels(-128.));
        let mut indexed = Painter::default();
        recorded.replay(&mut indexed, viewport, delta);
        indexed.finish();

        recorded.block_bounds = Box::default();
        let mut reference = Painter::default();
        recorded.replay(&mut reference, viewport, delta);
        reference.finish();

        assert_eq!(indexed.quads.len(), 48);
        assert!(
            indexed
                .paint_operations
                .matches_views(&reference.paint_operations, |left, right| left.visually_eq(right))
        );
    }

    #[test]
    fn native_row_accepts_rectangular_cells_inside_a_rounded_viewport() {
        let viewport = Bounds::new(point(px(0.), px(0.)), size(px(128.), px(128.)));
        let rounded = ContentMask {
            bounds: viewport,
            corner_bounds: viewport,
            corner_radii: crate::Corners::all(px(12.)),
        };
        let coverage = Bounds::new(point(px(0.), px(-128.)), size(px(128.), px(384.))).scale(1.);
        let cell = Bounds::new(point(px(5.), px(40.)), size(px(75.), px(16.))).scale(1.);
        let mut operations = Operations::default();
        operations.push(PaintOperation::Primitive(Primitive::Quad(crate::Quad {
            bounds: cell,
            content_mask: ContentMask::new(cell),
            ..Default::default()
        })));
        let source = Arc::new(ScrollPaint::new(Point::default(), coverage, operations));
        let placement = source.native_row_placement(rounded, 1., Point::default())
            .expect("the row only touches fully covered pixels");
        assert_eq!(placement.viewport.corner_radii, Default::default());
        assert_eq!(placement.raster_source().0.len(), 1);
        assert!(source.native_row_placement(
            rounded, 1., point(ScaledPixels(0.), ScaledPixels(-38.)),
        ).is_none());
    }
}
