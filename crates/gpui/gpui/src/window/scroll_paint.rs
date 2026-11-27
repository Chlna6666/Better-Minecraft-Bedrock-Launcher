//! Content-space paint shared by retained views and list rows.

use crate::{
    AnyElement, App, Bounds, ContentMask, Pixels, Point, ScaledPixels, Window, point,
    scene::{Operations, PaintOperation, Painter, Primitive},
};
use smallvec::SmallVec;
use std::rc::Rc;

const OPERATIONS_PER_BLOCK: usize = 32;
const MIN_INDEXED_OPERATIONS: usize = 128;

pub(super) struct ScrollPaint {
    pub(super) origin: Point<Pixels>,
    coverage: Bounds<ScaledPixels>,
    operations: Operations,
    block_bounds: Box<[Option<Bounds<ScaledPixels>>]>,
}

impl Window {
    pub(super) fn capture_scroll_paint(
        &mut self,
        origin: Point<Pixels>,
        item: &mut AnyElement,
        suppress_hover: bool,
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
        self.scroll_hover.painting = suppress_hover;
        item.paint(self, cx);
        self.scroll_hover.painting = false;
        self.visual_content_mask_stack.pop();
        self.content_mask_stack.pop();
        let operations = self.next_frame.painter.end_scroll_capture();
        let scale = self.scale_factor();
        let content = Rc::new(ScrollPaint::new(
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
    fn new(origin: Point<Pixels>, coverage: Bounds<ScaledPixels>, operations: Operations) -> Self {
        let block_bounds = if operations.len() >= MIN_INDEXED_OPERATIONS {
            Self::blocks(&operations).map(Self::block_bounds).collect()
        } else {
            Box::default()
        };
        Self {
            origin,
            coverage,
            operations,
            block_bounds,
        }
    }

    fn blocks(operations: &Operations) -> impl Iterator<Item = &[PaintOperation]> {
        operations
            .get_range(0..operations.len())
            .expect("full scroll operations range")
            .slices()
            .flat_map(|slice| slice.chunks(OPERATIONS_PER_BLOCK))
    }

    fn block_bounds(operations: &[PaintOperation]) -> Option<Bounds<ScaledPixels>> {
        let mut union: Option<Bounds<ScaledPixels>> = None;
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
            if !bounds.is_empty() {
                union = Some(union.map_or(bounds, |previous| previous.union(&bounds)));
            }
        }
        Some(union.unwrap_or_default())
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

    pub(super) fn replay(
        &self,
        painter: &mut Painter,
        viewport: ContentMask<ScaledPixels>,
        delta: Point<ScaledPixels>,
    ) {
        let mut layers = SmallVec::<[bool; 8]>::new();
        let source_viewport = Bounds::new(viewport.bounds.origin - delta, viewport.bounds.size);
        for (index, block) in Self::blocks(&self.operations).enumerate() {
            if let Some(Some(bounds)) = self.block_bounds.get(index)
                && (bounds.is_empty() || !bounds.intersects(&source_viewport))
            {
                continue;
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
                        if primitive.visual_bounds().intersects(&source_viewport) {
                            painter.insert_primitive(project_primitive(primitive, delta, viewport));
                        }
                    }
                    PaintOperation::StartBlur(_) | PaintOperation::EndBlur => {
                        unreachable!("scroll capture admits only built-in unfiltered paint")
                    }
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
