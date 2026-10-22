use crate::{Bounds, ScaledPixels, SceneFrameMetrics};
use collections::FxHashSet;

use std::ops::Range;

use super::{
    BatchIterator, MonochromeSprite, PaintBackdropBlur, PaintBlur, PaintOperation,
    PaintRendererExtension, PaintSurface, Path, PolychromeSprite, PreparedSceneBatch,
    PreparedSceneBatches, Primitive, PrimitiveBatch, SceneAnimationId, SceneAnimationValue, Shadow,
    Underline, blur_influence_radius,
};

/// Completed Paint data shared with rendering; mutable construction state stays in Painter.
#[derive(Default)]
pub(crate) struct Scene {
    pub(crate) revision: u64,
    pub(crate) paint_operations: super::Operations,
    pub(crate) shadows: super::PrimitiveStream<Shadow>,
    pub(crate) quads: super::PrimitiveStream<super::Quad>,
    pub(crate) paths: Vec<Path<ScaledPixels>>,
    pub(crate) underlines: Vec<Underline>,
    pub(crate) monochrome_sprites: Vec<MonochromeSprite>,
    pub(crate) polychrome_sprites: Vec<PolychromeSprite>,
    pub(crate) surfaces: Vec<PaintSurface>,
    pub(crate) backdrop_blurs: Vec<PaintBackdropBlur>,
    pub(crate) blurs: Vec<PaintBlur>,
    pub(crate) renderer_extensions: Vec<PaintRendererExtension>,
    pub(crate) animation_values: Vec<SceneAnimationValue>,
    pub(crate) prepared_batches: PreparedSceneBatches,
    pub(crate) replayed_primitives: usize,
}

impl Scene {
    pub fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub(crate) fn bounds_for_range(&self, range: Range<usize>) -> Option<Bounds<ScaledPixels>> {
        let mut bounds = None::<Bounds<ScaledPixels>>;
        for operation in self.paint_operations.get_range(range.clone())?.iter() {
            let operation_bounds = match operation {
                PaintOperation::Primitive(primitive) => Some(primitive.visual_bounds()),
                // Paint layers are ordering metadata, not pixel producers. Including their
                // often fullscreen bounds makes a small retained subtree appear to damage
                // the entire window when a popup or nested element changes.
                PaintOperation::StartLayer(_) => None,
                PaintOperation::StartBlur(blur) => Some(
                    blur.bounds
                        .dilate(blur_influence_radius(blur.radius))
                        .intersect(&blur.content_mask.bounds),
                ),
                PaintOperation::EndLayer | PaintOperation::EndBlur => None,
            };
            if let Some(operation_bounds) = operation_bounds {
                bounds = Some(match bounds {
                    Some(bounds) => bounds.union(&operation_bounds),
                    None => operation_bounds,
                });
            }
        }
        crate::scene::geometry::for_each_capture_group(&self, |group_range, group_bounds| {
            if group_range.start < range.end && range.start < group_range.end {
                bounds = Some(match bounds {
                    Some(bounds) => bounds.union(&group_bounds),
                    None => group_bounds,
                });
            }
        });
        bounds
    }

    pub(crate) fn has_backdrop_blurs(&self) -> bool {
        !self.backdrop_blurs.is_empty()
            || self
                .blurs
                .iter()
                .any(|blur| blur.content.has_backdrop_blurs())
    }

    /// Returns the prior captured source for a uniquely identified compositor layer.
    ///
    /// Duplicate retained identities are ambiguous and deliberately do not provide a reuse
    /// candidate. The current frame can still capture and render the layer normally.
    pub(crate) fn blur_content_for(
        &self,
        layer_id: &crate::GlobalElementId,
    ) -> Option<std::sync::Arc<Scene>> {
        fn visit(
            scene: &Scene,
            layer_id: &crate::GlobalElementId,
            found: &mut Option<std::sync::Arc<Scene>>,
            ambiguous: &mut bool,
        ) {
            for blur in &scene.blurs {
                if blur.layer_id.as_ref() == Some(layer_id) {
                    if found.is_some() {
                        *ambiguous = true;
                        return;
                    }
                    *found = Some(std::sync::Arc::clone(&blur.content));
                }
                visit(&blur.content, layer_id, found, ambiguous);
                if *ambiguous {
                    return;
                }
            }
        }

        let mut found = None;
        let mut ambiguous = false;
        visit(self, layer_id, &mut found, &mut ambiguous);
        (!ambiguous).then_some(found).flatten()
    }

    pub(crate) fn animation_value(
        &self,
        animation_id: SceneAnimationId,
    ) -> Option<&SceneAnimationValue> {
        self.animation_values
            .iter()
            .find(|value| value.animation_id == animation_id)
    }

    /// Collects every atlas allocation referenced by this retained scene, including nested
    /// element-blur/compositor scenes.
    ///
    /// Render backends use this immediately before submission to guarantee that an atlas
    /// retirement can never invalidate a tile still sampled by the Scene being encoded.
    pub(crate) fn collect_atlas_tile_ids_into(
        &self,
        ids: &mut FxHashSet<(crate::AtlasTextureId, u32)>,
    ) {
        ids.extend(
            self.monochrome_sprites
                .iter()
                .map(|sprite| (sprite.tile.texture_id, sprite.tile.tile_id.0)),
        );
        ids.extend(
            self.polychrome_sprites
                .iter()
                .map(|sprite| (sprite.tile.texture_id, sprite.tile.tile_id.0)),
        );
        for blur in &self.blurs {
            blur.content.collect_atlas_tile_ids_into(ids);
        }
    }

    /// Collects atlas allocation identities referenced by image/emoji sprites in this retained
    /// scene, including nested element-blur scenes.
    ///
    /// Window image residency pruning intersects this set with the static image-tile lookup, so
    /// non-image polychrome sprites are harmless: they never match an image cache entry.
    pub(crate) fn collect_polychrome_tile_ids_into(
        &self,
        ids: &mut FxHashSet<(crate::AtlasTextureId, u32)>,
    ) {
        ids.extend(
            self.polychrome_sprites
                .iter()
                .map(|sprite| (sprite.tile.texture_id, sprite.tile.tile_id.0)),
        );
        for blur in &self.blurs {
            blur.content.collect_polychrome_tile_ids_into(ids);
        }
    }

    pub(crate) fn collect_animation_ids_into(&self, ids: &mut FxHashSet<SceneAnimationId>) {
        for operation in self.paint_operations.iter() {
            match operation {
                PaintOperation::Primitive(primitive) => {
                    if let Some(animation_id) = primitive.animation_id() {
                        ids.insert(animation_id);
                    }
                    if let Primitive::Blur(blur) = primitive {
                        blur.content.collect_animation_ids_into(ids);
                    }
                }
                PaintOperation::StartBlur(blur) => {
                    if let Some(animation_id) = blur.animation_id {
                        ids.insert(animation_id);
                    }
                }
                PaintOperation::StartLayer(_)
                | PaintOperation::EndLayer
                | PaintOperation::EndBlur => {}
            }
        }
    }

    pub(crate) fn animation_ids(&self) -> FxHashSet<SceneAnimationId> {
        let mut ids = FxHashSet::default();
        self.collect_animation_ids_into(&mut ids);
        ids
    }

    pub(crate) fn frame_metrics(&self) -> SceneFrameMetrics {
        SceneFrameMetrics {
            primitives: self.prepared_batches.primitive_count,
            batches: self.prepared_batches.batch_count,
            replayed_primitives: self.replayed_primitives,
            retained_capacity: self.retained_capacity(),
            ..SceneFrameMetrics::default()
        }
    }

    pub(crate) fn prepared_batches(&self) -> &[PreparedSceneBatch] {
        self.prepared_batches.as_slice()
    }

    #[cfg_attr(
        all(
            any(target_os = "linux", target_os = "freebsd"),
            not(any(feature = "x11", feature = "wayland"))
        ),
        allow(dead_code)
    )]
    pub(crate) fn batches(&self) -> impl Iterator<Item = PrimitiveBatch<'_>> {
        BatchIterator {
            shadows_iter: self.shadows.cursor(),
            quads_iter: self.quads.cursor(),
            paths: &self.paths,
            paths_start: 0,
            paths_iter: self.paths.iter().peekable(),
            underlines: &self.underlines,
            underlines_start: 0,
            underlines_iter: self.underlines.iter().peekable(),
            monochrome_sprites: &self.monochrome_sprites,
            monochrome_sprites_start: 0,
            monochrome_sprites_iter: self.monochrome_sprites.iter().peekable(),
            polychrome_sprites: &self.polychrome_sprites,
            polychrome_sprites_start: 0,
            polychrome_sprites_iter: self.polychrome_sprites.iter().peekable(),
            surfaces: &self.surfaces,
            surfaces_start: 0,
            surfaces_iter: self.surfaces.iter().peekable(),
            backdrop_blurs: &self.backdrop_blurs,
            backdrop_blurs_start: 0,
            backdrop_blurs_iter: self.backdrop_blurs.iter().peekable(),
            blurs: &self.blurs,
            blurs_start: 0,
            blurs_iter: self.blurs.iter().peekable(),
            renderer_extensions: &self.renderer_extensions,
            renderer_extensions_start: 0,
            renderer_extensions_iter: self.renderer_extensions.iter().peekable(),
        }
    }

    pub(super) fn primitive_count(&self) -> usize {
        self.shadows.len()
            + self.quads.len()
            + self.paths.len()
            + self.underlines.len()
            + self.monochrome_sprites.len()
            + self.polychrome_sprites.len()
            + self.surfaces.len()
            + self.backdrop_blurs.len()
            + self.blurs.len()
            + self.renderer_extensions.len()
    }

    pub(super) fn retained_capacity(&self) -> usize {
        self.paint_operations.capacity()
            + self.shadows.capacity()
            + self.quads.capacity()
            + self.paths.capacity()
            + self.underlines.capacity()
            + self.monochrome_sprites.capacity()
            + self.polychrome_sprites.capacity()
            + self.surfaces.capacity()
            + self.backdrop_blurs.capacity()
            + self.blurs.capacity()
            + self.renderer_extensions.capacity()
            + self.animation_values.capacity()
            + self.prepared_batches.capacity()
    }
}
