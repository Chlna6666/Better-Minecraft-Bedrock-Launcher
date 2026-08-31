use super::{PaintOperation, Scene};
use crate::ScaledPixels;

impl Scene {
    /// Captures the root-scene indices needed to turn the next element-filter capture into a
    /// retained compositor layer after the child subtree has finished painting.
    pub(crate) fn composite_capture_checkpoint(&self) -> (usize, usize) {
        (self.paint_operations.len(), self.blurs.len())
    }

    /// Converts one just-completed top-level element-filter capture into a zero-filter compositor
    /// layer and shrinks its damage geometry to the subtree's real pixel-producing operations.
    ///
    /// `StartLayer` is batching/layout metadata, not pixels. Keeping a full-window layer bound here
    /// would make an otherwise local page translation look like full-window source damage to every
    /// later backdrop filter. The retained operation stream is tightened as well, so replaying a
    /// cached subtree on the next frame preserves the same pixel bounds.
    pub(crate) fn finalize_composite_capture(&mut self, checkpoint: (usize, usize)) {
        let (operation_start, blur_index) = checkpoint;
        let Some(operation_end) = self.paint_operations.len().checked_sub(1) else {
            return;
        };
        if operation_start >= operation_end || self.blurs.len() != blur_index.saturating_add(1) {
            // Nested element filters keep their PaintBlur inside the parent capture scene. The
            // public compositor remains visually correct through the epsilon identity fallback;
            // only top-level captures can currently be promoted without reaching into private
            // capture-stack state.
            return;
        }
        if !matches!(
            self.paint_operations.get(operation_start),
            Some(PaintOperation::StartBlur(_))
        ) || !matches!(
            self.paint_operations.get(operation_end),
            Some(PaintOperation::EndBlur)
        ) {
            return;
        }

        let Some(content_mask_bounds) = self
            .paint_operations
            .get(operation_start)
            .and_then(|operation| match operation {
                PaintOperation::StartBlur(capture) => Some(capture.content_mask.bounds),
                _ => None,
            })
        else {
            return;
        };

        let Some(pixel_bounds) = self.paint_operations[operation_start + 1..operation_end]
            .iter()
            .filter_map(PaintOperation::visual_bounds)
            .reduce(|left, right| left.union(&right))
        else {
            return;
        };
        let tight_bounds = pixel_bounds.intersect(&content_mask_bounds);
        if tight_bounds.is_empty() {
            return;
        }

        if let Some(PaintOperation::StartBlur(capture)) =
            self.paint_operations.get_mut(operation_start)
        {
            capture.bounds = tight_bounds;
            capture.radius = ScaledPixels(0.0);
        }

        // Retained replay feeds these operations back through Scene::begin_blur/end_blur. Clamp
        // structural layer bounds now so the replayed capture cannot grow back to its layout box.
        for operation in &mut self.paint_operations[operation_start + 1..operation_end] {
            if let PaintOperation::StartLayer(bounds) = operation {
                *bounds = bounds.intersect(&tight_bounds);
            }
        }

        let blur = &mut self.blurs[blur_index];
        blur.bounds = tight_bounds;
        blur.radius = ScaledPixels(0.0);
    }
}
