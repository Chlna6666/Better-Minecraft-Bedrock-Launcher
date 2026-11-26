use super::blur_influence_radius;
use crate::{BlurCapture, Bounds, PaintOperation, ScaledPixels, Scene};
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

/// A completed isolated layer's source range and conservative output extent.
/// Recorded once when its capture closes, in increasing end order.
pub(super) struct CaptureGroup {
    pub(super) range: Range<usize>,
    pub(super) bounds: Bounds<ScaledPixels>,
    /// The published root output, if this capture actually produced one.
    /// Revisions identify immutable source content; order narrows the lookup
    /// after the typed blur stream is sorted. Ambiguous matches fall back.
    pub(super) output: Option<(u64, super::DrawOrder)>,
}

impl CaptureGroup {
    pub(super) fn close(scene: &Scene, start: usize, config: &BlurCapture) -> Self {
        let end = scene.len();
        let mut bounds = config.bounds;
        // The old traversal also included all raw source primitive bounds
        // and the expanded bounds of nested captures. Keep that conservative
        // behavior, but only visit this capture's source, once at close.
        for operation in scene.paint_operations.range(start + 1..end).iter() {
            if let PaintOperation::Primitive(primitive) = operation {
                bounds = bounds.union(&primitive.visual_bounds());
            }
        }
        let first = scene.capture_groups.partition_point(|group| group.range.end <= start);
        for group in &scene.capture_groups[first..] {
            bounds = bounds.union(&group.bounds);
        }
        Self::from_source_bounds(start..end + 1, config, bounds)
    }

    pub(super) fn from_source_bounds(
        range: Range<usize>,
        config: &BlurCapture,
        bounds: Bounds<ScaledPixels>,
    ) -> Self {
        Self {
            range,
            output: None,
            bounds: bounds.dilate(blur_influence_radius(config.radius))
                .intersect(&config.content_mask.bounds),
        }
    }
}

pub(crate) fn for_each_capture_group(
    scene: &Scene,
    mut visit: impl FnMut(Range<usize>, Bounds<ScaledPixels>),
) {
    if scene.blurs.is_empty() {
        return;
    }
    for group in &scene.capture_groups {
        visit(group.range.clone(), group.bounds);
    }
}

#[cfg(test)]
pub(super) fn scan_capture_groups(
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
    let mut stack = smallvec::SmallVec::<[(usize, &BlurCapture, Bounds<ScaledPixels>); 4]>::new();

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
