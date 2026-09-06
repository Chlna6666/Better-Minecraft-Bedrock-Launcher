use super::*;

impl FrameUpload {
    /// Register the final composite record of every animated element blur after the static scene has
    /// been encoded. `BeginBlur` indices already describe the exact shared blur-buffer slots, so
    /// ownership can be attached directly to AnimatedUpload without a parallel binding stream.
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
            if blur.animation_id.is_none() {
                continue;
            }
            if self.animated_primitives.len() >= MAX_ANIMATION_VALUES {
                break;
            }
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
    for blur in &scene.blurs {
        output.push(blur);
        collect_element_blurs(&blur.content, output);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_blur(animation_id: crate::SceneAnimationId, content: crate::Scene) -> crate::PaintBlur {
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

    fn opacity_value(animation_id: crate::SceneAnimationId) -> crate::SceneAnimationValue {
        crate::SceneAnimationValue {
            animation_id,
            property: crate::TransitionProperty::Opacity,
            progress: 0.5,
            from: [0.2, 0.0, 0.0, 0.0],
            to: [1.0, 0.0, 0.0, 0.0],
        }
    }

    fn test_tile(kind: crate::AtlasTextureKind) -> crate::AtlasTile {
        crate::AtlasTile {
            texture_id: crate::AtlasTextureId { index: 0, kind },
            tile_id: crate::TileId(0),
            padding: 1,
            bounds: crate::bounds(
                crate::point(crate::DevicePixels(1), crate::DevicePixels(1)),
                crate::size(crate::DevicePixels(1), crate::DevicePixels(1)),
            ),
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

    #[test]
    fn nested_parent_translation_keeps_child_opacity_on_quad_glyph_and_image() {
        let parent_animation_id = crate::SceneAnimationId(7);
        let child_animation_id = crate::SceneAnimationId(8);
        let child_bounds = crate::bounds(
            crate::point(crate::ScaledPixels(4.0), crate::ScaledPixels(6.0)),
            crate::size(crate::ScaledPixels(20.0), crate::ScaledPixels(16.0)),
        );
        let content_mask = crate::ContentMask::new(child_bounds);
        let mut child = crate::Scene::default();

        child.insert_animated_primitive(
            crate::Quad {
                bounds: child_bounds,
                content_mask: content_mask.clone(),
                ..Default::default()
            },
            child_animation_id,
        );
        child.insert_animated_primitive(
            crate::MonochromeSprite {
                order: 1,
                pad: crate::MonochromeSpriteSampling::Glyph as u32,
                animation_id: None,
                bounds: child_bounds,
                content_mask: content_mask.clone(),
                color: crate::Hsla::default().into(),
                tile: test_tile(crate::AtlasTextureKind::Monochrome),
                transformation: crate::TransformationMatrix::unit(),
            },
            child_animation_id,
        );
        child.insert_animated_primitive(
            crate::PolychromeSprite {
                order: 2,
                pad: 0,
                grayscale: false,
                opacity: 1.0,
                animation_id: None,
                bounds: child_bounds,
                content_mask,
                corner_radii: Default::default(),
                tile: test_tile(crate::AtlasTextureKind::Rgba),
            },
            child_animation_id,
        );
        child.push_animation_value(opacity_value(child_animation_id));

        let mut scene = crate::Scene::default();
        scene.insert_primitive(test_blur(parent_animation_id, child));
        scene.push_animation_value(active_value(parent_animation_id));

        assert_eq!(scene.blurs.len(), 1);
        let parent = &scene.blurs[0];
        assert_eq!(parent.animation_id, Some(parent_animation_id));
        assert_eq!(scene.animation_values.len(), 1);
        assert_eq!(
            scene.animation_values[0].property,
            crate::TransitionProperty::Translation
        );
        assert_eq!(scene.animation_values[0].animation_id, parent_animation_id);

        let child = parent.content.as_ref();
        assert_eq!(child.quads.len(), 1);
        assert_eq!(child.monochrome_sprites.len(), 1);
        assert_eq!(child.polychrome_sprites.len(), 1);
        assert_eq!(child.quads[0].animation_id, Some(child_animation_id));
        assert_eq!(
            child.monochrome_sprites[0].animation_id,
            Some(child_animation_id)
        );
        assert_eq!(
            child.polychrome_sprites[0].animation_id,
            Some(child_animation_id)
        );
        assert_eq!(child.animation_values.len(), 1);
        assert_eq!(
            child.animation_values[0].property,
            crate::TransitionProperty::Opacity
        );
        assert_eq!(child.animation_values[0].animation_id, child_animation_id);

        let child_animation_ids = child.animation_ids();
        assert_eq!(child_animation_ids.len(), 1);
        assert!(child_animation_ids.contains(&child_animation_id));
        assert!(!child_animation_ids.contains(&parent_animation_id));
    }
}
