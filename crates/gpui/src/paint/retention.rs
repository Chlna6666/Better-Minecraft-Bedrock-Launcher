//! Content validation for retained filter sources, independent of window damage.
use crate::scene::geometry::is_solid_quad;
use crate::{DrawOrder, PaintOperation, Primitive, Scene, SceneAnimationValue};
use std::ops::Range;

/// A static, exclusive Quad run proven against one sorted Paint generation.
pub(crate) struct QuadChunkProof {
    pub(crate) range: Range<usize>,
    pub(crate) is_solid: bool,
}

/// Temporary qualification work for one Window Chunk planning pass.
/// Borrows the only Paint payload and lazily indexes order numbers, never retaining a render tree.
pub(crate) struct QuadChunkPlanner<'a> {
    scene: &'a Scene,
    use_order_index: bool,
    sorted_orders: Option<Vec<DrawOrder>>,
}

impl<'a> QuadChunkPlanner<'a> {
    pub(crate) fn new(scene: &'a Scene, candidates: usize) -> Self {
        Self {
            scene,
            use_order_index: candidates >= 8 && scene.len() >= 128,
            sorted_orders: None,
        }
    }

    /// Rejects animated, mixed-state, captured, incomplete or nonexclusive ranges.
    pub(crate) fn prove(&mut self, range: Range<usize>) -> Option<QuadChunkProof> {
        let operations = self.scene.paint_operations.get(range.clone())?;
        if operations.len() < 32 {
            return None;
        }
        let (first, last, is_solid) = static_quad_orders(operations)?;
        let start = self.scene.quads.partition_point(|quad| quad.order < first);
        let end = self.scene.quads.partition_point(|quad| quad.order <= last);
        if end.saturating_sub(start) != operations.len()
            || !self.order_is_exclusive(range, first, last, operations.len())
        {
            return None;
        }
        Some(QuadChunkProof {
            range: start..end,
            is_solid,
        })
    }

    fn order_is_exclusive(
        &mut self,
        range: Range<usize>,
        first: DrawOrder,
        last: DrawOrder,
        count: usize,
    ) -> bool {
        if self.use_order_index {
            let orders = self.sorted_orders.get_or_insert_with(|| {
                let mut orders: Vec<_> = self
                    .scene
                    .paint_operations
                    .iter()
                    .filter_map(|operation| match operation {
                        PaintOperation::Primitive(primitive) => Some(primitive.order()),
                        _ => None,
                    })
                    .collect();
                orders.sort_unstable();
                orders
            });
            return retained_order_range_is_exclusive(orders, first, last, count);
        }
        self.scene
            .paint_operations
            .iter()
            .enumerate()
            .all(|(index, operation)| {
                range.contains(&index)
                    || match operation {
                        PaintOperation::Primitive(primitive) => {
                            primitive.order() < first || primitive.order() > last
                        }
                        _ => true,
                    }
            })
    }
}

fn static_quad_orders(operations: &[PaintOperation]) -> Option<(DrawOrder, DrawOrder, bool)> {
    let mut first = DrawOrder::MAX;
    let mut last = DrawOrder::MIN;
    let mut is_solid = None;
    for operation in operations {
        let PaintOperation::Primitive(Primitive::Quad(quad)) = operation else {
            return None;
        };
        let solid = is_solid_quad(quad);
        if quad.animation_id.is_some() || is_solid.is_some_and(|previous| previous != solid) {
            return None;
        }
        first = first.min(quad.order);
        last = last.max(quad.order);
        is_solid = Some(solid);
    }
    Some((first, last, is_solid?))
}

/// Counts all Primitive orders, including captured children and duplicates absent from flat Quads.
pub(crate) fn retained_order_range_is_exclusive(
    sorted: &[DrawOrder],
    first: DrawOrder,
    last: DrawOrder,
    count: usize,
) -> bool {
    let start = sorted.partition_point(|&order| order < first);
    let end = sorted.partition_point(|&order| order <= last);
    end - start == count
}

/// Proves a Paint range does not inherit a Layer or Capture from outside the range.
/// Invalid ranges and unmatched closing scopes reject reuse; an empty valid range is independent.
pub(crate) fn range_is_replayable(scene: &Scene, range: Range<usize>) -> bool {
    if range.start > range.end || range.end > scene.len() {
        return false;
    }
    if range.is_empty() {
        return true;
    }
    let mut blur_depth = 0usize;
    let mut layer_depth = 0usize;
    for (index, operation) in scene.paint_operations[..range.end].iter().enumerate() {
        if index == range.start && (blur_depth != 0 || layer_depth != 0) {
            return false;
        }
        match operation {
            PaintOperation::StartBlur(_) => blur_depth = blur_depth.saturating_add(1),
            PaintOperation::StartLayer(_) => layer_depth = layer_depth.saturating_add(1),
            PaintOperation::EndBlur => {
                let Some(depth) = blur_depth.checked_sub(1) else {
                    return false;
                };
                blur_depth = depth;
            }
            PaintOperation::EndLayer => {
                let Some(depth) = layer_depth.checked_sub(1) else {
                    return false;
                };
                layer_depth = depth;
            }
            PaintOperation::Primitive(_) => {}
        }
    }
    blur_depth == 0 && layer_depth == 0
}

/// Compares complete Paint structure, including ordering-layer geometry.
/// This proof can preserve a Paint revision; damage comparison alone may ignore layer bounds.
pub(crate) fn paints_match(current: &Scene, previous: &Scene) -> bool {
    current.paint_operations.len() == previous.paint_operations.len()
        && current
            .paint_operations
            .iter()
            .zip(&previous.paint_operations)
            .all(|(current, previous)| match (current, previous) {
                (PaintOperation::Primitive(current), PaintOperation::Primitive(previous)) => {
                    current.visually_eq(previous)
                }
                (PaintOperation::StartLayer(current), PaintOperation::StartLayer(previous)) => {
                    current == previous
                }
                (PaintOperation::EndLayer, PaintOperation::EndLayer)
                | (PaintOperation::EndBlur, PaintOperation::EndBlur) => true,
                (PaintOperation::StartBlur(current), PaintOperation::StartBlur(previous)) => {
                    current == previous
                }
                _ => false,
            })
}

/// Exact source-cache proof for an isolated element filter.
///
/// Window damage is NOT element-filter input damage: an unrelated overlay
/// above the filter can intersect its output rectangle without changing any
/// of its captured pixels. Compare the captured painter operations instead.
/// Samples must be the complete resolved set consumed by the GPU, including
/// caller-sampled values registered in ancestor scenes. Missing samples
/// execute at identity; every property track participates in this proof.
pub(crate) fn filter_source_matches(
    scene: &Scene,
    previous: &Scene,
    animation_values: &[SceneAnimationValue],
    previous_animation_values: &[SceneAnimationValue],
) -> bool {
    if scene.paint_operations.len() != previous.paint_operations.len() {
        return false;
    }

    for (current, old) in scene
        .paint_operations
        .iter()
        .zip(&previous.paint_operations)
    {
        if !current.visually_eq(old) {
            return false;
        }
        // A nested filtered child can have a dynamic GPU-sampled source
        // even when its own composite primitive and the enclosing scene
        // retain identical static bytes. Traverse it before deciding to
        // reuse the parent filter result.
        if let (
            PaintOperation::Primitive(Primitive::Blur(current_blur)),
            PaintOperation::Primitive(Primitive::Blur(previous_blur)),
        ) = (current, old)
            && !crate::paint::retention::filter_source_matches(
                &current_blur.content,
                &previous_blur.content,
                animation_values,
                previous_animation_values,
            )
        {
            return false;
        }
        let animation_id = match current {
            PaintOperation::Primitive(primitive) => primitive.animation_id(),
            PaintOperation::StartBlur(blur) => blur.animation_id,
            PaintOperation::StartLayer(_) | PaintOperation::EndLayer | PaintOperation::EndBlur => {
                None
            }
        };
        let Some(animation_id) = animation_id else {
            continue;
        };
        let mut current_values = animation_values
            .iter()
            .filter(|value| value.animation_id == animation_id);
        let mut previous_values = previous_animation_values
            .iter()
            .filter(|value| value.animation_id == animation_id);
        if !current_values.clone().eq(previous_values.clone())
            && !(current_values.all(crate::paint::animation_sample_is_identity)
                && previous_values.all(crate::paint::animation_sample_is_identity))
        {
            return false;
        }
    }
    true
}
