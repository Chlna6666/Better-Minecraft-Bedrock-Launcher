use super::*;

impl FrameUpload {
    /// Register the final composite record of every animated element blur after the static scene has
    /// been encoded. `BeginBlur` indices already describe the exact shared blur-buffer slots, so we
    /// can add animation bindings without perturbing the recursive encoder or the captured child
    /// batches.
    pub(in crate::platform::nova) fn register_element_blur_animations(
        &mut self,
        scene: &crate::Scene,
        summary: &mut FrameUploadSummary,
    ) {
        let mut blurs = Vec::new();
        collect_element_blurs(scene, &mut blurs);
        if blurs.is_empty() {
            return;
        }

        let blur_indices: Vec<u32> = self
            .batches
            .iter()
            .filter_map(|batch| match *batch {
                UploadedBatch::BeginBlur { index } => Some(index),
                UploadedBatch::SolidQuads { .. }
                | UploadedBatch::Quads { .. }
                | UploadedBatch::Shadows { .. }
                | UploadedBatch::PathRasterization { .. }
                | UploadedBatch::Paths { .. }
                | UploadedBatch::MonoSprites { .. }
                | UploadedBatch::PolySprites { .. }
                | UploadedBatch::Underlines { .. }
                | UploadedBatch::BackdropBlurs { .. }
                | UploadedBatch::EndBlur { .. }
                | UploadedBatch::CompositeBlur { .. }
                | UploadedBatch::CustomMesh3d { .. } => None,
            })
            .collect();
        debug_assert_eq!(
            blurs.len(),
            blur_indices.len(),
            "encoded element-blur markers must match scene blur composites"
        );

        for (blur, index) in blurs.into_iter().zip(blur_indices) {
            let Some(animation_id) = blur.animation_id else {
                continue;
            };
            if self.animation_bindings.len() / PACKED_ANIMATION_BINDING_BYTES
                >= MAX_ANIMATION_BINDINGS
            {
                break;
            }
            // Root backdrop and element blur composites share the same GPU buffer/record kind.
            // AnimatedUpload's Primitive variant keeps their filter semantics distinct on the CPU.
            write_animation_binding(
                &mut self.animation_bindings,
                animation_id,
                AnimatedPrimitiveKind::BackdropBlur,
                index,
            );
            summary.animation_binding_count = summary.animation_binding_count.saturating_add(1);
            self.animated_primitives.push(AnimatedUpload::new(
                crate::Primitive::Blur(blur.clone()),
                AnimatedPrimitiveKind::BackdropBlur,
                index,
            ));
        }
    }

    /// Element blur source/filter work can be skipped while the retained static upload is reused
    /// and the only active animation on the layer is its promoted final composite. Captured child
    /// animations remain source mutations and therefore deliberately block this fast path.
    pub(in crate::platform::nova) fn composite_only_element_blur_indices(&self) -> FxHashSet<u32> {
        if !self.retained_static_reused {
            return FxHashSet::default();
        }
        let active_animation_ids: FxHashSet<_> = self
            .sampled_animation_values
            .iter()
            .map(|value| value.animation_id)
            .collect();
        self.animated_primitives
            .iter()
            .filter_map(|primitive| {
                let blur = primitive.base_paint_blur()?;
                let animation_id = blur.animation_id?;
                if !active_animation_ids.contains(&animation_id)
                    || !blur.content.animation_ids().is_empty()
                {
                    return None;
                }
                Some(primitive.index)
            })
            .collect()
    }
}

fn collect_element_blurs<'a>(scene: &'a crate::Scene, output: &mut Vec<&'a crate::PaintBlur>) {
    // Scene::blurs is sorted by draw order during finish(), which is also the order in which the
    // prepared blur batches are recursively encoded. Descend immediately after every parent to
    // mirror `encode_scene()`'s BeginBlur -> child -> EndBlur sequence.
    for blur in &scene.blurs {
        output.push(blur);
        collect_element_blurs(&blur.content, output);
    }
}
