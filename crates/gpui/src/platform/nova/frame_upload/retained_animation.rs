use super::*;

impl FrameUpload {
    /// Refreshes animation values for a retained static upload without rebuilding the flattened
    /// primitive/batch stream.
    ///
    /// `encode_scene` recursively flattens element-blur child scenes into the same `FrameUpload`.
    /// A retained frame therefore has to refresh animation values in that exact recursive order as
    /// well; refreshing only the root scene leaves animated primitives inside `PaintBlur::content`
    /// sampling stale values and was the reason retained uploads were previously disabled whenever
    /// any element blur existed.
    pub(in crate::platform::nova) fn refresh_retained_animation_values(
        &mut self,
        scene: &crate::Scene,
        summary: &mut FrameUploadSummary,
    ) {
        self.animation_values.clear();
        self.sampled_animation_values.clear();
        summary.animation_value_count = 0;
        self.append_retained_animation_values(scene, summary);
    }

    fn append_retained_animation_values(
        &mut self,
        scene: &crate::Scene,
        summary: &mut FrameUploadSummary,
    ) {
        for value in &scene.animation_values {
            let Some(property) = AnimationProperty::from_transition_property(value.property) else {
                continue;
            };
            if self.animation_values.len() / PACKED_ANIMATION_VALUE_BYTES >= MAX_ANIMATION_VALUES {
                return;
            }
            write_animation_value(
                &mut self.animation_values,
                value.animation_id,
                property,
                value.progress,
                value.from,
                value.to,
            );
            summary.animation_value_count = summary.animation_value_count.saturating_add(1);
            self.sampled_animation_values.push(*value);
        }

        // Keep the traversal identical to `encode_scene`: each child blur scene is encoded when its
        // `PreparedSceneBatch::Blurs` entry is encountered. This matters when callers construct
        // nested scenes with independent animation-value arrays.
        for batch in scene.prepared_batches() {
            let PreparedSceneBatch::Blurs(range) = batch else {
                continue;
            };
            for blur in &scene.blurs[range.clone()] {
                if self.animation_values.len() / PACKED_ANIMATION_VALUE_BYTES
                    >= MAX_ANIMATION_VALUES
                {
                    return;
                }
                self.append_retained_animation_values(&blur.content, summary);
            }
        }
    }
}
