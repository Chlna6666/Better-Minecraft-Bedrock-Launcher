use crate::{Bounds, ScaledPixels, SceneFrameMetrics};

use super::BoundsTree;
use std::ops::Range;

use super::util::{is_solid_quad, slice_range, trim_vec_capacity};
use super::{
    BatchIterator, DrawOrder, MonochromeSprite, PaintBackdropBlur, PaintGpuMesh3d, PaintOperation,
    PaintSurface, Path, PathId, PolychromeSprite, PreparedBackdropBlurGroup, PreparedGpuMesh3dPass,
    PreparedQuadRun, PreparedSceneBatch, PreparedSceneBatches, Primitive, PrimitiveBatch, Quad,
    SceneAnimationId, SceneAnimationValue, Shadow, Underline,
};

#[derive(Default)]
pub(crate) struct Scene {
    pub(crate) paint_operations: Vec<PaintOperation>,
    primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<DrawOrder>,
    pub(crate) shadows: Vec<Shadow>,
    pub(crate) quads: Vec<Quad>,
    pub(crate) paths: Vec<Path<ScaledPixels>>,
    pub(crate) underlines: Vec<Underline>,
    pub(crate) monochrome_sprites: Vec<MonochromeSprite>,
    pub(crate) polychrome_sprites: Vec<PolychromeSprite>,
    pub(crate) surfaces: Vec<PaintSurface>,
    pub(crate) backdrop_blurs: Vec<PaintBackdropBlur>,
    pub(crate) gpu_meshes_3d: Vec<PaintGpuMesh3d>,
    pub(crate) animation_values: Vec<SceneAnimationValue>,
    next_scene_animation_id: u32,
    prepared_batches: PreparedSceneBatches,
    replayed_primitives: usize,
    pub(super) retained_prefix_invalid: bool,
    pub(super) retained_prefix_verified_len: usize,
    idle_clear_frames: u16,
    recent_peak_paint_operations: usize,
    recent_peak_primitives: usize,
}

#[derive(Clone, Copy)]
enum ScenePrimitiveKind {
    Shadow,
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    PolychromeSprite,
    Surface,
    BackdropBlur,
    GpuMesh3d,
}

const SCENE_IDLE_TRIM_FRAMES: u16 = 45;
const SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER: usize = 2;
const SCENE_MIN_RETAINED_CAPACITY: usize = 24;

impl Scene {
    pub fn clear(&mut self) {
        let primitive_count_before_clear = self.primitive_count();
        self.recent_peak_paint_operations = self
            .recent_peak_paint_operations
            .max(self.paint_operations.len());
        self.recent_peak_primitives = self
            .recent_peak_primitives
            .max(primitive_count_before_clear);

        self.paint_operations.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.paths.clear();
        self.shadows.clear();
        self.quads.clear();
        self.underlines.clear();
        self.monochrome_sprites.clear();
        self.polychrome_sprites.clear();
        self.surfaces.clear();
        self.backdrop_blurs.clear();
        self.gpu_meshes_3d.clear();
        self.animation_values.clear();
        self.prepared_batches.clear();
        self.replayed_primitives = 0;
        self.retained_prefix_invalid = false;
        self.retained_prefix_verified_len = 0;

        if primitive_count_before_clear == 0 {
            self.idle_clear_frames = self.idle_clear_frames.saturating_add(1);
        } else {
            self.idle_clear_frames = 0;
        }

        if self.idle_clear_frames >= SCENE_IDLE_TRIM_FRAMES {
            self.trim_retained_capacity();
            self.idle_clear_frames = 0;
        }
    }

    pub fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub(crate) fn bounds_for_range(&self, range: Range<usize>) -> Option<Bounds<ScaledPixels>> {
        let mut bounds = None::<Bounds<ScaledPixels>>;
        for operation in self.paint_operations.get(range)? {
            let operation_bounds = match operation {
                PaintOperation::Primitive(primitive) => Some(*primitive.bounds()),
                PaintOperation::StartLayer(layer_bounds) => Some(*layer_bounds),
                PaintOperation::EndLayer => None,
            };
            if let Some(operation_bounds) = operation_bounds {
                bounds = Some(match bounds {
                    Some(bounds) => bounds.union(&operation_bounds),
                    None => operation_bounds,
                });
            }
        }
        bounds
    }

    pub(crate) fn requires_full_redraw_fallback(&self) -> bool {
        !self.surfaces.is_empty() || !self.gpu_meshes_3d.is_empty()
    }

    pub(crate) fn has_backdrop_blurs(&self) -> bool {
        !self.backdrop_blurs.is_empty()
    }

    pub(crate) fn backdrop_blur_bounds(&self) -> impl Iterator<Item = Bounds<ScaledPixels>> + '_ {
        self.backdrop_blurs.iter().map(|blur| blur.bounds)
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        self.push_replayed_layer(bounds);
    }

    fn push_replayed_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
        self.paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    pub fn pop_layer(&mut self) {
        self.pop_replayed_layer();
    }

    fn pop_replayed_layer(&mut self) {
        self.layer_stack.pop();
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        let primitive = primitive.into();
        let Some(order) = self.order_for_primitive(&primitive) else {
            return;
        };
        self.push_ordered_primitive(primitive, order);
    }

    pub(crate) fn allocate_animation_id(&mut self) -> SceneAnimationId {
        let animation_id = SceneAnimationId(self.next_scene_animation_id);
        self.next_scene_animation_id = self.next_scene_animation_id.wrapping_add(1);
        animation_id
    }

    pub(crate) fn insert_animated_primitive(
        &mut self,
        primitive: impl Into<Primitive>,
        animation_id: SceneAnimationId,
    ) {
        let mut primitive = primitive.into();
        primitive.set_animation_id(animation_id);
        self.insert_primitive(primitive);
    }

    pub(crate) fn push_animation_value(&mut self, value: SceneAnimationValue) {
        self.animation_values.push(value);
    }

    fn order_for_primitive(&mut self, primitive: &Primitive) -> Option<DrawOrder> {
        let clipped_bounds = primitive
            .bounds()
            .intersect(&primitive.content_mask().bounds);

        if clipped_bounds.is_empty() {
            return None;
        }

        Some(
            self.layer_stack
                .last()
                .copied()
                .unwrap_or_else(|| self.primitive_bounds.insert(clipped_bounds)),
        )
    }

    fn push_ordered_primitive(&mut self, mut primitive: Primitive, order: DrawOrder) {
        match &mut primitive {
            Primitive::Shadow(shadow) => {
                shadow.order = order;
                self.shadows.push(shadow.clone());
            }
            Primitive::Quad(quad) => {
                quad.order = order;
                self.quads.push(quad.clone());
            }
            Primitive::Path(path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path.clone());
            }
            Primitive::Underline(underline) => {
                underline.order = order;
                self.underlines.push(underline.clone());
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.order = order;
                self.monochrome_sprites.push(sprite.clone());
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(sprite.clone());
            }
            Primitive::Surface(surface) => {
                surface.order = order;
                self.surfaces.push(surface.clone());
            }
            Primitive::BackdropBlur(blur) => {
                blur.order = order;
                self.backdrop_blurs.push(blur.clone());
            }
            Primitive::GpuMesh3d(mesh) => {
                mesh.order = order;
                self.gpu_meshes_3d.push(mesh.clone());
            }
        }
        self.paint_operations
            .push(PaintOperation::Primitive(primitive));
    }

    fn replay_primitive(&mut self, primitive: &Primitive, retain_order: bool) {
        let order = if retain_order {
            let clipped_bounds = primitive
                .bounds()
                .intersect(&primitive.content_mask().bounds);
            if clipped_bounds.is_empty() {
                return;
            }
            if let Some(layer_order) = self.layer_stack.last().copied() {
                debug_assert_eq!(layer_order, primitive.order());
                layer_order
            } else {
                self.primitive_bounds
                    .insert_with_order(clipped_bounds, primitive.order())
            }
        } else if let Some(order) = self.order_for_primitive(primitive) {
            order
        } else {
            return;
        };
        self.replayed_primitives = self.replayed_primitives.saturating_add(1);
        let primitive_kind = self.push_replayed_primitive(primitive, order);
        let mut primitive = primitive.clone();
        primitive.set_order(order);
        if let (ScenePrimitiveKind::Path, Primitive::Path(path)) = (primitive_kind, &mut primitive)
        {
            path.id = PathId(self.paths.len().saturating_sub(1));
        }
        self.paint_operations
            .push(PaintOperation::Primitive(primitive));
    }

    fn push_replayed_primitive(
        &mut self,
        primitive: &Primitive,
        order: DrawOrder,
    ) -> ScenePrimitiveKind {
        match primitive {
            Primitive::Shadow(shadow) => {
                let mut shadow = shadow.clone();
                shadow.order = order;
                self.shadows.push(shadow);
                ScenePrimitiveKind::Shadow
            }
            Primitive::Quad(quad) => {
                let mut quad = quad.clone();
                quad.order = order;
                self.quads.push(quad);
                ScenePrimitiveKind::Quad
            }
            Primitive::Path(path) => {
                let mut path = path.clone();
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path);
                ScenePrimitiveKind::Path
            }
            Primitive::Underline(underline) => {
                let mut underline = underline.clone();
                underline.order = order;
                self.underlines.push(underline);
                ScenePrimitiveKind::Underline
            }
            Primitive::MonochromeSprite(sprite) => {
                let mut sprite = sprite.clone();
                sprite.order = order;
                self.monochrome_sprites.push(sprite);
                ScenePrimitiveKind::MonochromeSprite
            }
            Primitive::PolychromeSprite(sprite) => {
                let mut sprite = sprite.clone();
                sprite.order = order;
                self.polychrome_sprites.push(sprite);
                ScenePrimitiveKind::PolychromeSprite
            }
            Primitive::Surface(surface) => {
                let mut surface = surface.clone();
                surface.order = order;
                self.surfaces.push(surface);
                ScenePrimitiveKind::Surface
            }
            Primitive::BackdropBlur(blur) => {
                let mut blur = blur.clone();
                blur.order = order;
                self.backdrop_blurs.push(blur);
                ScenePrimitiveKind::BackdropBlur
            }
            Primitive::GpuMesh3d(mesh) => {
                let mut mesh = mesh.clone();
                mesh.order = order;
                self.gpu_meshes_3d.push(mesh);
                ScenePrimitiveKind::GpuMesh3d
            }
        }
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

    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        let range_end = range.end;
        let retain_order = !self.retained_prefix_invalid
            && self.paint_operations.len() == range.start
            && self.ordering_prefix_matches_previous(prev_scene, range.start);
        if !retain_order {
            self.retained_prefix_invalid = true;
        }
        for operation in &prev_scene.paint_operations[range] {
            match operation {
                PaintOperation::Primitive(primitive) => {
                    self.replay_primitive(primitive, retain_order)
                }
                PaintOperation::StartLayer(bounds) => self.push_replayed_layer(*bounds),
                PaintOperation::EndLayer => self.pop_replayed_layer(),
            }
        }
        if retain_order && self.paint_operations.len() == range_end {
            self.retained_prefix_verified_len = range_end;
        } else if retain_order {
            self.retained_prefix_invalid = true;
        }
    }

    fn ordering_prefix_matches_previous(&self, prev_scene: &Scene, prefix_end: usize) -> bool {
        let prefix_start = self.retained_prefix_verified_len;
        let Some(current_prefix) = self.paint_operations.get(prefix_start..prefix_end) else {
            return false;
        };
        let Some(previous_prefix) = prev_scene.paint_operations.get(prefix_start..prefix_end)
        else {
            return false;
        };

        current_prefix
            .iter()
            .zip(previous_prefix)
            .all(|(current, previous)| ordering_operations_match(current, previous))
    }

    pub fn finish(&mut self) {
        self.shadows.sort_unstable_by_key(|shadow| shadow.order);
        self.quads.sort_unstable_by_key(|quad| quad.order);
        self.paths.sort_unstable_by_key(|path| path.order);
        self.underlines
            .sort_unstable_by_key(|underline| underline.order);
        self.monochrome_sprites
            .sort_unstable_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.polychrome_sprites
            .sort_unstable_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.surfaces.sort_unstable_by_key(|surface| surface.order);
        self.backdrop_blurs.sort_unstable_by_key(|blur| blur.order);
        self.gpu_meshes_3d.sort_unstable_by_key(|mesh| mesh.order);
        self.prepare_batches();
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
            shadows: &self.shadows,
            shadows_start: 0,
            shadows_iter: self.shadows.iter().peekable(),
            quads: &self.quads,
            quads_start: 0,
            quads_iter: self.quads.iter().peekable(),
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
            gpu_meshes_3d: &self.gpu_meshes_3d,
            gpu_meshes_3d_start: 0,
            gpu_meshes_3d_iter: self.gpu_meshes_3d.iter().peekable(),
        }
    }

    fn primitive_count(&self) -> usize {
        self.shadows.len()
            + self.quads.len()
            + self.paths.len()
            + self.underlines.len()
            + self.monochrome_sprites.len()
            + self.polychrome_sprites.len()
            + self.surfaces.len()
            + self.backdrop_blurs.len()
            + self.gpu_meshes_3d.len()
    }

    fn retained_capacity(&self) -> usize {
        self.paint_operations.capacity()
            + self.shadows.capacity()
            + self.quads.capacity()
            + self.paths.capacity()
            + self.underlines.capacity()
            + self.monochrome_sprites.capacity()
            + self.polychrome_sprites.capacity()
            + self.surfaces.capacity()
            + self.backdrop_blurs.capacity()
            + self.gpu_meshes_3d.capacity()
            + self.animation_values.capacity()
            + self.prepared_batches.batches.capacity()
    }

    fn trim_retained_capacity(&mut self) {
        let primitive_floor = self.recent_peak_primitives.max(SCENE_MIN_RETAINED_CAPACITY);
        let paint_floor = self
            .recent_peak_paint_operations
            .max(SCENE_MIN_RETAINED_CAPACITY);

        trim_vec_capacity(
            &mut self.paint_operations,
            paint_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.shadows,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.quads,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.paths,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.underlines,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.monochrome_sprites,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.polychrome_sprites,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.surfaces,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.backdrop_blurs,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.gpu_meshes_3d,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.animation_values,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.prepared_batches.batches,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );

        self.prepared_batches.retained_capacity = self.prepared_batches.batches.capacity();
        self.recent_peak_paint_operations = self.paint_operations.len();
        self.recent_peak_primitives = self.primitive_count();
    }

    fn prepare_batches(&mut self) {
        let mut prepared = std::mem::take(&mut self.prepared_batches.batches);
        prepared.clear();
        for batch in self.batches() {
            if let PrimitiveBatch::Quads(quads) = batch {
                let range = slice_range(&self.quads, quads);
                let mut run_start = range.start;
                let mut run_is_solid = quads.first().is_some_and(is_solid_quad);
                for (offset, quad) in quads.iter().enumerate().skip(1) {
                    let is_solid = is_solid_quad(quad);
                    if is_solid == run_is_solid {
                        continue;
                    }
                    let run_end = range.start + offset;
                    prepared.push(PreparedSceneBatch::Quads(PreparedQuadRun {
                        range: run_start..run_end,
                        is_solid: run_is_solid,
                    }));
                    run_start = run_end;
                    run_is_solid = is_solid;
                }
                prepared.push(PreparedSceneBatch::Quads(PreparedQuadRun {
                    range: run_start..range.end,
                    is_solid: run_is_solid,
                }));
                continue;
            }

            prepared.push(match batch {
                PrimitiveBatch::Shadows(shadows) => {
                    PreparedSceneBatch::Shadows(slice_range(&self.shadows, shadows))
                }
                PrimitiveBatch::Quads(_) => {
                    unreachable!("quad batches are split before this match")
                }
                PrimitiveBatch::Paths(paths) => {
                    PreparedSceneBatch::Paths(slice_range(&self.paths, paths))
                }
                PrimitiveBatch::Underlines(underlines) => {
                    PreparedSceneBatch::Underlines(slice_range(&self.underlines, underlines))
                }
                PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    sampling,
                    sprites,
                } => PreparedSceneBatch::MonochromeSprites {
                    texture_id,
                    sampling,
                    range: slice_range(&self.monochrome_sprites, sprites),
                },
                PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    sprites,
                } => PreparedSceneBatch::PolychromeSprites {
                    texture_id,
                    range: slice_range(&self.polychrome_sprites, sprites),
                },
                PrimitiveBatch::Surfaces(surfaces) => {
                    PreparedSceneBatch::Surfaces(slice_range(&self.surfaces, surfaces))
                }
                PrimitiveBatch::BackdropBlurs(blurs) => {
                    PreparedSceneBatch::BackdropBlurs(PreparedBackdropBlurGroup {
                        range: slice_range(&self.backdrop_blurs, blurs),
                    })
                }
                PrimitiveBatch::GpuMeshes3d(meshes) => {
                    PreparedSceneBatch::GpuMeshes3d(PreparedGpuMesh3dPass {
                        range: slice_range(&self.gpu_meshes_3d, meshes),
                    })
                }
            });
        }

        self.prepared_batches.batches = prepared;
        self.prepared_batches.batch_count = self.prepared_batches.batches.len();
        self.prepared_batches.primitive_count = self.primitive_count();
        self.prepared_batches.retained_capacity = self.prepared_batches.batches.capacity();
    }
}

fn ordering_operations_match(current: &PaintOperation, previous: &PaintOperation) -> bool {
    match (current, previous) {
        (PaintOperation::Primitive(current), PaintOperation::Primitive(previous)) => {
            current.order() == previous.order()
                && current.bounds().intersect(&current.content_mask().bounds)
                    == previous.bounds().intersect(&previous.content_mask().bounds)
        }
        (PaintOperation::StartLayer(current), PaintOperation::StartLayer(previous)) => {
            current == previous
        }
        (PaintOperation::EndLayer, PaintOperation::EndLayer) => true,
        _ => false,
    }
}
