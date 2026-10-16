use super::RetainedPaint;
use crate::paint::retention::range_is_replayable;
use crate::{PaintOperation, Scene};
use std::ops::Range;

impl RetainedPaint {
    /// Reuses a sparse structure proof only for the immutable Paint generation that produced it.
    pub(crate) fn range_is_replayable(&self, scene: &Scene, range: Range<usize>) -> bool {
        if scene.revision == 0 || scene.len() < 256 {
            return range_is_replayable(scene, range);
        }
        self.replay_structure
            .get_or_init(|| ReplayStructureIndex::new(scene))
            .range_is_replayable(scene, range)
    }
}

/// A sparse structural prefix state after one Start/End Blur or Layer operation.
/// Ordinary primitives do not allocate entries; the index is derived from the
/// authoritative paint_operations, not a second display list or RenderTree.
#[derive(Clone, Copy, Default)]
struct ReplayBoundary {
    after_operation: usize,
    blur_depth: usize,
    layer_depth: usize,
    invalid_prefix: bool,
}

#[derive(Default)]
pub(crate) struct ReplayStructureIndex {
    revision: u64,
    operation_count: usize,
    boundaries: Vec<ReplayBoundary>,
}

impl ReplayStructureIndex {
    /// Allocated sparse structural entries, excluding authoritative Paint storage.
    pub(crate) fn retained_capacity(&self) -> usize {
        self.boundaries.capacity()
    }

    pub(crate) fn new(scene: &Scene) -> Self {
        let operations = &scene.paint_operations;
        let mut state = ReplayBoundary::default();
        let mut boundaries = Vec::new();
        for (index, operation) in operations.iter().enumerate() {
            match operation {
                PaintOperation::StartBlur(_) => {
                    state.blur_depth = state.blur_depth.saturating_add(1);
                }
                PaintOperation::EndBlur => match state.blur_depth.checked_sub(1) {
                    Some(depth) => state.blur_depth = depth,
                    None => state.invalid_prefix = true,
                },
                PaintOperation::StartLayer(_) => {
                    state.layer_depth = state.layer_depth.saturating_add(1);
                }
                PaintOperation::EndLayer => match state.layer_depth.checked_sub(1) {
                    Some(depth) => state.layer_depth = depth,
                    None => state.invalid_prefix = true,
                },
                PaintOperation::Primitive(_) => continue,
            }
            state.after_operation = index + 1;
            boundaries.push(state);
        }
        Self {
            boundaries,
            revision: scene.revision,
            operation_count: operations.len(),
        }
    }

    fn state_at(&self, operation_boundary: usize) -> ReplayBoundary {
        let end = self
            .boundaries
            .partition_point(|state| state.after_operation <= operation_boundary);
        if end == 0 {
            ReplayBoundary::default()
        } else {
            self.boundaries[end - 1]
        }
    }

    pub(crate) fn range_is_replayable(&self, scene: &Scene, range: Range<usize>) -> bool {
        if scene.revision == 0
            || scene.revision != self.revision
            || scene.paint_operations.len() != self.operation_count
        {
            return range_is_replayable(scene, range);
        }
        if range.start > range.end || range.end > self.operation_count {
            return false;
        }
        if range.is_empty() {
            return true;
        }
        let start = self.state_at(range.start);
        if start.invalid_prefix || start.blur_depth != 0 || start.layer_depth != 0 {
            return false;
        }
        let end = self.state_at(range.end);
        !end.invalid_prefix && end.blur_depth == 0 && end.layer_depth == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlurCapture, Bounds, Primitive, Quad, ScaledPixels};
    #[test]
    fn replay_structure_index_matches_prefix_scan_for_nested_and_invalid_ranges() {
        let mut scene = Scene::default();
        let quad = || PaintOperation::Primitive(Primitive::Quad(Quad::default()));
        scene.paint_operations.extend((0..132).map(|_| quad()));
        scene
            .paint_operations
            .push(PaintOperation::StartLayer(Bounds::default()));
        scene.paint_operations.extend((0..4).map(|_| quad()));
        scene
            .paint_operations
            .push(PaintOperation::StartBlur(BlurCapture {
                animation_id: None,
                layer_id: None,
                bounds: Bounds::default(),
                content_mask: crate::ContentMask::default(),
                radius: ScaledPixels(2.0),
                opacity: 1.0,
            }));
        scene.paint_operations.extend((0..5).map(|_| quad()));
        scene.paint_operations.push(PaintOperation::EndBlur);
        scene.paint_operations.push(PaintOperation::EndLayer);
        scene.paint_operations.extend((0..132).map(|_| quad()));
        scene.revision = 1;
        let index = ReplayStructureIndex::new(&scene);
        let len = scene.paint_operations.len();
        for start in 0..=len {
            for end in start..=len {
                assert_eq!(
                    index.range_is_replayable(&scene, start..end),
                    range_is_replayable(&scene, start..end),
                    "structural range {start}..{end}"
                );
            }
        }
        // A corrupted unmatched close must poison only ranges reaching it,
        // not an earlier completed prefix. The index retains this fact.
        scene.paint_operations.insert(134, PaintOperation::EndBlur);
        scene.revision = 2;
        for start in 0..=scene.paint_operations.len() {
            let end = scene.paint_operations.len();
            assert_eq!(
                index.range_is_replayable(&scene, start..end),
                range_is_replayable(&scene, start..end),
                "invalid structural prefix {start}..{end}"
            );
        }
        // A mutable/in-progress Scene may not reuse a completed old index.
        scene.revision = 0;
        assert_eq!(
            index.range_is_replayable(&scene, 0..135),
            range_is_replayable(&scene, 0..135)
        );
    }
}

/// Preserves Paint identity only after proving the entire sorted operation stream unchanged.
/// Called by Window retention after Paint finish, before the immutable generation is published.
pub(crate) fn retain_revision(current: &mut Scene, previous: &Scene) {
    if crate::paint::retention::paints_match(current, previous) {
        current.revision = previous.revision;
    }
}
