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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_blur(
        animation_id: crate::SceneAnimationId,
        content: crate::Scene,
    ) -> crate::PaintBlur {
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
            crate::size(crate::ScaledPixels(120.0), crate::ScaledPixels(80.0)),
        );
        crate::PaintBlur {
            order: 3,
            animation_id: Some(animation_id),
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            radius: crate::ScaledPixels(12.0),
            opacity: 1.0,
            content: std::sync::Arc::new(content),
        }
    }

    fn active_value(animation_id: crate::SceneAnimationId) -> crate::SceneAnimationValue {
        crate::SceneAnimationValue {
            animation_id,
            property: crate::TransitionProperty::Translation,
            progress: 0.5,
            from: [0.0; 4],
            to: [32.0, 0.0, 0.0, 0.0],
        }
    }

    #[test]
    fn retained_static_element_blur_with_only_composite_animation_skips_filter_work() {
        let animation_id = crate::SceneAnimationId(7);
        let blur = test_blur(animation_id, crate::Scene::default());
        let upload = FrameUpload {
            retained_static_reused: true,
            sampled_animation_values: vec![active_value(animation_id)],
            animated_primitives: vec![AnimatedUpload::new(
                crate::Primitive::Blur(blur),
                AnimatedPrimitiveKind::BackdropBlur,
                9,
            )],
            ..Default::default()
        };

        assert!(upload.composite_only_element_blur_indices().contains(&9));
    }

    #[test]
    fn animated_child_blocks_composite_only_element_blur_fast_path() {
        let animation_id = crate::SceneAnimationId(7);
        let child_animation_id = crate::SceneAnimationId(8);
        let child_bounds = crate::bounds(
            crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
            crate::size(crate::ScaledPixels(20.0), crate::ScaledPixels(20.0)),
        );
        let mut child = crate::Scene::default();
        child.insert_animated_primitive(
            crate::Quad {
                bounds: child_bounds,
                content_mask: crate::ContentMask::new(child_bounds),
                ..Default::default()
            },
            child_animation_id,
        );
        let blur = test_blur(animation_id, child);
        let upload = FrameUpload {
            retained_static_reused: true,
            sampled_animation_values: vec![active_value(animation_id)],
            animated_primitives: vec![AnimatedUpload::new(
                crate::Primitive::Blur(blur),
                AnimatedPrimitiveKind::BackdropBlur,
                9,
            )],
            ..Default::default()
        };

        assert!(!upload.composite_only_element_blur_indices().contains(&9));
    }
}