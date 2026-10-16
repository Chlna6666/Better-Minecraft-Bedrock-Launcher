use super::blur_influence_radius;
use crate::{BlurCapture, Bounds, PaintOperation, ScaledPixels, Scene};
use smallvec::SmallVec;
use std::ops::Range;

use super::Quad;
use crate::IsZero;

pub(super) fn trim_vec_capacity<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    if vec.capacity() > floor.saturating_mul(multiplier) {
        vec.shrink_to(floor);
    }
}

pub(super) fn slice_range<T>(whole: &[T], part: &[T]) -> Range<usize> {
    let start =
        part.as_ptr().addr().saturating_sub(whole.as_ptr().addr()) / std::mem::size_of::<T>();
    start..start.saturating_add(part.len())
}

pub(crate) fn is_solid_quad(quad: &Quad) -> bool {
    quad.background.tag == crate::BackgroundTag::Solid
        && !quad.border_widths.any(|width| !width.is_zero())
        && quad.corner_radii.is_zero()
}

pub(crate) fn for_each_capture_group(
    scene: &Scene,
    mut visit: impl FnMut(Range<usize>, Bounds<ScaledPixels>),
) {
    // Most ordinary GUI scenes contain no isolated element filter. In that
    // case this query is called for every retained fragment during damage
    // computation, but a full scan of the painter list cannot yield a
    // filter output. Every completed, visible capture owns a PaintBlur.
    if scene.blurs.is_empty() {
        return;
    }
    let mut stack = SmallVec::<[(usize, &BlurCapture, Bounds<ScaledPixels>); 4]>::new();

    for (index, operation) in scene.paint_operations.iter().enumerate() {
        match operation {
            PaintOperation::StartBlur(config) => {
                stack.push((index, config, config.bounds));
            }
            PaintOperation::Primitive(primitive) => {
                let primitive_bounds = primitive.visual_bounds();
                for (_, _, bounds) in &mut stack {
                    *bounds = bounds.union(&primitive_bounds);
                }
            }
            // The layer only groups primitives for draw ordering. Its own bounds
            // must not inflate an isolated filter's pixel-producing footprint.
            PaintOperation::StartLayer(_) => {}
            PaintOperation::EndBlur => {
                let Some((start, config, bounds)) = stack.pop() else {
                    continue;
                };
                let bounds = bounds
                    .dilate(blur_influence_radius(config.radius))
                    .intersect(&config.content_mask.bounds);
                for (_, _, parent_bounds) in &mut stack {
                    *parent_bounds = parent_bounds.union(&bounds);
                }
                visit(start..index + 1, bounds);
            }
            PaintOperation::EndLayer => {}
        }
    }
}
