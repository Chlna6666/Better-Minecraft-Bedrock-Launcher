//! Content-space paint shared by retained views and list rows.

use crate::{
    AnyElement, App, Bounds, ContentMask, Pixels, Point, ScaledPixels, Window, point,
    scene::{Operations, PaintOperation, Painter, Primitive},
};
use smallvec::SmallVec;
use std::sync::Arc;

const OPERATIONS_PER_BLOCK: usize = 32;
const MIN_INDEXED_OPERATIONS: usize = 128;

pub(in crate::window) struct ScrollPaint {
    pub(super) origin: Point<Pixels>,
    coverage: Bounds<ScaledPixels>,
    operations: Arc<Operations>,
    raster_eligible: bool,
    atlas_tiles: Arc<[(crate::AtlasTextureId, u32)]>,
    block_bounds: Box<[Option<PaintBlockBounds>]>,
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
        item: &mut AnyElement,
        suppress_hover: bool,
        cx: &mut App,
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
        item.paint(self, cx);
        self.scroll_hover.painting = false;
        self.visual_content_mask_stack.pop();
        self.content_mask_stack.pop();
        let operations = self.next_frame.painter.end_scroll_capture();
        let scale = self.scale_factor();
        let content = Arc::new(ScrollPaint::new(
            origin,
            capture.scale(scale).bounds,
            operations,
        ));
        content.replay(
            &mut self.next_frame.painter,
            viewport.scale(scale),
            Point::default(),
        );
        content
    }
}

impl ScrollPaint {
    fn new(origin: Point<Pixels>, coverage: Bounds<ScaledPixels>, mut operations: Operations) -> Self {
        operations.finish();
        // Qualify once, not on each scrolled frame or GPU owner request.
        let mut atlas_tiles = collections::FxHashSet::default();
        let mut raster_eligible = operations.len() != 0
            && operations.range_is_replayable(0..operations.len());
        // Residency belongs to the frozen source even when GPU raster
        // admission fails. Do not stop gathering dependencies at the first
        // unsupported operation: later offscreen images can scroll into view.
        for operation in operations.iter() {
            match operation {
                PaintOperation::Primitive(Primitive::MonochromeSprite(sprite)) => {
                    atlas_tiles.insert((sprite.tile.texture_id, sprite.tile.tile_id.0));
                }
                PaintOperation::Primitive(Primitive::PolychromeSprite(sprite)) => {
                    atlas_tiles.insert((sprite.tile.texture_id, sprite.tile.tile_id.0));
                }
                _ => {}
            }
            raster_eligible = raster_eligible
                && super::frame::static_scroll_raster_operation(operation, coverage);
        }
        let block_bounds = if operations.len() >= MIN_INDEXED_OPERATIONS {
            Self::blocks(&operations).map(Self::block_bounds).collect()
        } else {
            Box::default()
        };
        Self {
            origin,
            coverage,
            operations: Arc::new(operations),
            raster_eligible,
            atlas_tiles: atlas_tiles.into_iter().collect::<Vec<_>>().into(),
            block_bounds,
        }
    }

    pub(super) fn collect_atlas_tile_ids_into(&self, live: &mut collections::FxHashSet<(crate::AtlasTextureId, u32)>) {
        live.extend(self.atlas_tiles.iter().copied());
    }

    fn blocks(operations: &Operations) -> impl Iterator<Item = &[PaintOperation]> {
        operations
            .get_range(0..operations.len())
            .expect("full scroll operations range")
            .slices()
            .flat_map(|slice| slice.chunks(OPERATIONS_PER_BLOCK))
    }

    fn block_bounds(operations: &[PaintOperation]) -> Option<PaintBlockBounds> {
        let mut union: Option<PaintBlockBounds> = None;
        for operation in operations {
            // Always replay scope markers in painter order, even when the
            // block's visible primitives are all clipped.
            let PaintOperation::Primitive(primitive) = operation else {
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

    /// A transferable placement is independent of the UI element, input
    /// records and the mutable Painter. It uses the same original indexed
    /// operation pages for eager and deferred projection.
    pub(in crate::window) fn placement(self: &Arc<Self>, viewport: ContentMask<ScaledPixels>,
        delta: Point<ScaledPixels>) -> Option<ScrollPlacement> {
        (self.raster_eligible && self.covers(viewport.bounds, delta)
            && viewport.corner_radii == Default::default()
            && [delta.x.0, delta.y.0].into_iter().all(|v| v.is_finite() && v.fract() == 0.))
            .then(|| ScrollPlacement { source: Arc::clone(self), viewport, delta })
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
            for operation in block {
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
                        if fully_visible || primitive.visual_bounds().intersects(&source_viewport) {
                            painter.insert_primitive(project_primitive(primitive, delta, viewport));
                        }
                    }
                    PaintOperation::DeferredScroll(_) | PaintOperation::StartBlur(_) | PaintOperation::EndBlur => {
                        unreachable!("scroll capture admits only built-in unfiltered paint")
                    }
                }
            }
        }
    }
}

/// Immutable placement, suitable for a published Paint generation.
#[derive(Clone)]
pub(crate) struct ScrollPlacement {
    source: Arc<ScrollPaint>,
    viewport: ContentMask<ScaledPixels>,
    delta: Point<ScaledPixels>,
}

impl ScrollPlacement {
    pub(in crate::window) fn raster_source(&self) -> (&Arc<Operations>, Bounds<ScaledPixels>,
        Point<ScaledPixels>, &Arc<[(crate::AtlasTextureId, u32)]>) {
        (&self.source.operations, self.source.coverage, self.delta, &self.source.atlas_tiles)
    }

    pub(crate) fn same_paint(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
            && self.viewport == other.viewport
            && self.delta == other.delta
    }

    /// The viewport is a conservative damage bound for every projected
    /// primitive, including a newly exposed part of the overscan.
    pub(crate) fn visual_bounds(&self) -> Bounds<ScaledPixels> {
        self.viewport.bounds
    }

    pub(crate) fn collect_atlas_tile_ids_into(&self, live: &mut collections::FxHashSet<(crate::AtlasTextureId, u32)>) {
        self.source.collect_atlas_tile_ids_into(live);
    }

    pub(crate) fn replay(&self, painter: &mut Painter) {
        self.source.replay(painter, self.viewport, self.delta);
    }
}

fn moved(bounds: Bounds<ScaledPixels>, delta: Point<ScaledPixels>) -> Bounds<ScaledPixels> {
    Bounds::new(bounds.origin + delta, bounds.size)
}

fn project_primitive(
    primitive: &Primitive,
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
        Primitive::Quad(quad) => project!(quad, Quad),
        Primitive::Underline(underline) => project!(underline, Underline),
        Primitive::MonochromeSprite(sprite) => project!(sprite, MonochromeSprite),
        Primitive::PolychromeSprite(sprite) => project!(sprite, PolychromeSprite),
        Primitive::Shadow(shadow) => project!(shadow, Shadow),
        _ => unreachable!("unsupported primitive in built-in scroll capture"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Quad, px, size};

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
        let placement = source.placement(viewport, delta).expect("covered integral translation");
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
        assert!(eager.paint_operations.matches_by(&recovered.paint_operations, PaintOperation::same_paint));
        assert!(source.placement(viewport, point(ScaledPixels(0.), ScaledPixels(-0.5))).is_none());
        assert!(source.placement(viewport, point(ScaledPixels(0.), ScaledPixels(-256.))).is_none());
    }

    #[test]
    fn materialized_root_replaces_only_certified_placement_span() {
        let bounds = |y| Bounds::new(point(px(0.), px(y)), size(px(40.), px(16.))).scale(1.);
        let coverage = Bounds::new(point(px(0.), px(-64.)), size(px(40.), px(192.))).scale(1.);
        let viewport = ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(40.), px(64.))));
        let mut operations = Operations::default();
        operations.push(PaintOperation::Primitive(Primitive::Quad(Quad {
            bounds: bounds(48.), content_mask: ContentMask::new(coverage), ..Default::default()
        })));
        let source = Arc::new(ScrollPaint::new(Point::default(), coverage, operations));
        let old_placement = source.placement(viewport.scale(1.), Point::default()).unwrap();
        let moved_placement = source.placement(viewport.scale(1.), point(ScaledPixels(0.), ScaledPixels(-32.))).unwrap();
        let outside = |y| Quad { bounds: bounds(y), content_mask: viewport.scale(1.), ..Default::default() };
        let mut painter = Painter::default();
        painter.insert_primitive(outside(0.));
        old_placement.replay(&mut painter);
        painter.insert_primitive(outside(48.));
        painter.finish();
        let scene = painter.into_scene();
        let mut paint = crate::window::Paint::new(scene, Arc::default(), Default::default());
        let mut fixture_source = Operations::default();
        fixture_source.share(&source.operations, 0..source.operations.len());
        paint.layers.raster_fixture(fixture_source, coverage, viewport, [0, 0]);
        paint.layers.certify_fixture_root(&paint.scene, 1..2);
        paint.layers.attach_fixture_placement(moved_placement.clone());
        let recovered = paint.materialize_static_root().expect("static root fallback");
        let mut eager = Painter::default();
        eager.insert_primitive(outside(0.));
        moved_placement.replay(&mut eager);
        eager.insert_primitive(outside(48.));
        eager.finish();
        assert!(recovered.scene.paint_operations.matches_by(&eager.paint_operations, PaintOperation::same_paint));
        assert_eq!(recovered.scene.len(), 3);
        assert!(recovered.layers.frames.is_empty());
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
        let placement = source.placement(viewport, point(ScaledPixels(0.), ScaledPixels(-16.))).unwrap();
        let outside = |y| Quad { bounds: bounds(y), content_mask: viewport, ..Default::default() };
        let mut deferred = Painter::default();
        deferred.insert_primitive(outside(0.));
        deferred.scene.paint_operations.push(PaintOperation::DeferredScroll(placement.clone()));
        deferred.insert_primitive(outside(32.));
        deferred.finish();
        let scene = deferred.into_scene();
        assert_eq!(scene.len(), 3);
        assert!(scene.paint_operations.has_deferred_scroll());
        assert_eq!(scene.paint_operations.deferred_scrolls().count(), 1);
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
        let recovered = paint.materialize_static_root().expect("native deferred fallback");
        let mut eager = Painter::default();
        eager.insert_primitive(outside(0.));
        placement.replay(&mut eager);
        eager.insert_primitive(outside(32.));
        eager.finish();
        assert!(!recovered.scene.paint_operations.has_deferred_scroll());
        assert!(recovered.scene.paint_operations.matches_by(&eager.paint_operations, PaintOperation::same_paint));
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
                .matches_by(&reference.paint_operations, PaintOperation::visually_eq)
        );
    }
}
