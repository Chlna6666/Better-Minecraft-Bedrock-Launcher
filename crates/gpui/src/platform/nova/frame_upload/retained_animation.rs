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
        let expected_count = self.sampled_animation_values.len();
        let mut refreshed_count = 0usize;
        let mut topology_matches = self.animation_values.len()
            == expected_count.saturating_mul(PACKED_ANIMATION_VALUE_BYTES);
        self.refresh_retained_animation_values_in_place(
            scene,
            &mut refreshed_count,
            &mut topology_matches,
        );

        if !topology_matches || refreshed_count != expected_count {
            self.animation_values.clear();
            self.sampled_animation_values.clear();
            summary.animation_value_count = 0;
            self.append_retained_animation_values(scene, summary);
        } else {
            summary.animation_value_count = refreshed_count as u32;
        }

        // Custom-mesh animation is a separate renderer path. Avoid touching its sidecar on the
        // normal 2D retained-animation path; a full encode already rebuilt it when mesh topology
        // changed.
        if !self.custom_mesh_3d_animation_ids.is_empty() {
            self.rebuild_custom_mesh_3d_animations();
        }
    }

    fn refresh_retained_animation_values_in_place(
        &mut self,
        scene: &crate::Scene,
        refreshed_count: &mut usize,
        topology_matches: &mut bool,
    ) {
        for value in &scene.animation_values {
            let Some(property) = AnimationProperty::from_transition_property(value.property) else {
                *topology_matches = false;
                continue;
            };
            let index = *refreshed_count;
            *refreshed_count = (*refreshed_count).saturating_add(1);
            let Some(previous) = self.sampled_animation_values.get_mut(index) else {
                *topology_matches = false;
                continue;
            };
            if previous.animation_id != value.animation_id
                || previous.property != value.property
                || previous.from != value.from
                || previous.to != value.to
            {
                *topology_matches = false;
                continue;
            }
            let offset = index.saturating_mul(PACKED_ANIMATION_VALUE_BYTES);
            let Some(record) = self
                .animation_values
                .get_mut(offset..offset + PACKED_ANIMATION_VALUE_BYTES)
            else {
                *topology_matches = false;
                continue;
            };
            write_animation_value_at(
                record,
                value.animation_id,
                property,
                value.progress,
                value.from,
                value.to,
            );
            *previous = *value;
        }

        for batch in scene.prepared_batches() {
            let PreparedSceneBatch::Blurs(range) = batch else {
                continue;
            };
            for blur in &scene.blurs[range.clone()] {
                self.refresh_retained_animation_values_in_place(
                    &blur.content,
                    refreshed_count,
                    topology_matches,
                );
            }
        }
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
