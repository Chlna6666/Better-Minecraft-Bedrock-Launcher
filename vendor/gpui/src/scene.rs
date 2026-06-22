// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTextureId, AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, IsZero,
    Pixels, Point, Radians, ScaledPixels, SceneFrameMetrics, Size, bounds_tree::BoundsTree, point,
};
use std::{
    fmt::Debug,
    iter::Peekable,
    ops::{Add, Range, Sub},
    slice,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::SeqCst},
    },
};

#[allow(non_camel_case_types, unused)]
pub(crate) type PathVertex_ScaledPixels = PathVertex<ScaledPixels>;

pub(crate) type DrawOrder = u32;

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
    pub(crate) subpixel_sprites: Vec<SubpixelSprite>,
    pub(crate) polychrome_sprites: Vec<PolychromeSprite>,
    pub(crate) surfaces: Vec<PaintSurface>,
    pub(crate) backdrop_blurs: Vec<PaintBackdropBlur>,
    pub(crate) gpu_meshes_3d: Vec<PaintGpuMesh3d>,
    prepared_batches: PreparedSceneBatches,
    replayed_primitives: usize,
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
    SubpixelSprite,
    PolychromeSprite,
    Surface,
    BackdropBlur,
    GpuMesh3d,
}

const SCENE_IDLE_TRIM_FRAMES: u16 = 45;
const SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER: usize = 2;
const SCENE_MIN_RETAINED_CAPACITY: usize = 24;

#[derive(Default, Clone, Debug)]
pub(crate) struct PreparedSceneBatches {
    batches: Vec<PreparedSceneBatch>,
    pub batch_count: usize,
    pub primitive_count: usize,
    pub retained_capacity: usize,
}

impl PreparedSceneBatches {
    pub fn as_slice(&self) -> &[PreparedSceneBatch] {
        &self.batches
    }

    fn clear(&mut self) {
        self.batches.clear();
        self.batch_count = 0;
        self.primitive_count = 0;
        self.retained_capacity = self.batches.capacity();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PreparedSceneBatch {
    Shadows(Range<usize>),
    Quads(PreparedQuadRun),
    Paths(Range<usize>),
    Underlines(Range<usize>),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        sampling: MonochromeSpriteSampling,
        range: Range<usize>,
    },
    SubpixelSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    Surfaces(Range<usize>),
    BackdropBlurs(PreparedBackdropBlurGroup),
    GpuMeshes3d(PreparedGpuMesh3dPass),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedQuadRun {
    pub range: Range<usize>,
    pub is_solid: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedBackdropBlurGroup {
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedGpuMesh3dPass {
    pub range: Range<usize>,
}

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
        self.subpixel_sprites.clear();
        self.polychrome_sprites.clear();
        self.surfaces.clear();
        self.backdrop_blurs.clear();
        self.gpu_meshes_3d.clear();
        self.prepared_batches.clear();
        self.replayed_primitives = 0;

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
        !self.surfaces.is_empty()
            || !self.backdrop_blurs.is_empty()
            || !self.gpu_meshes_3d.is_empty()
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
        self.paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    pub fn pop_layer(&mut self) {
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
            Primitive::SubpixelSprite(sprite) => {
                sprite.order = order;
                self.subpixel_sprites.push(sprite.clone());
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

    fn replay_primitive(&mut self, primitive: &Primitive) {
        let Some(order) = self.order_for_primitive(primitive) else {
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
            Primitive::SubpixelSprite(sprite) => {
                let mut sprite = sprite.clone();
                sprite.order = order;
                self.subpixel_sprites.push(sprite);
                ScenePrimitiveKind::SubpixelSprite
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
        for operation in &prev_scene.paint_operations[range] {
            match operation {
                PaintOperation::Primitive(primitive) => self.replay_primitive(primitive),
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::EndLayer => self.pop_layer(),
            }
        }
    }

    pub fn finish(&mut self) {
        self.shadows.sort_by_key(|shadow| shadow.order);
        self.quads.sort_by_key(|quad| quad.order);
        self.paths.sort_by_key(|path| path.order);
        self.underlines.sort_by_key(|underline| underline.order);
        self.monochrome_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.subpixel_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.polychrome_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.surfaces.sort_by_key(|surface| surface.order);
        self.backdrop_blurs.sort_by_key(|blur| blur.order);
        self.gpu_meshes_3d.sort_by_key(|mesh| mesh.order);
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
            subpixel_sprites: &self.subpixel_sprites,
            subpixel_sprites_start: 0,
            subpixel_sprites_iter: self.subpixel_sprites.iter().peekable(),
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
            + self.subpixel_sprites.len()
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
            &mut self.subpixel_sprites,
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
            &mut self.prepared_batches.batches,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );

        self.prepared_batches.retained_capacity = self.prepared_batches.batches.capacity();
        self.recent_peak_paint_operations = self.paint_operations.len();
        self.recent_peak_primitives = self.primitive_count();
    }

    pub(crate) fn trim_retained_capacity_for_level(&mut self, level: crate::GpuiMemoryTrimLevel) {
        match level {
            crate::GpuiMemoryTrimLevel::Light => self.trim_retained_capacity(),
            crate::GpuiMemoryTrimLevel::Moderate | crate::GpuiMemoryTrimLevel::Aggressive => {
                self.paint_operations.shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.layer_stack.shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.shadows.shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.quads.shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.paths.shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.underlines.shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.monochrome_sprites
                    .shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.subpixel_sprites.shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.polychrome_sprites
                    .shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.surfaces.shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.backdrop_blurs.shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.gpu_meshes_3d.shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.prepared_batches
                    .batches
                    .shrink_to(SCENE_MIN_RETAINED_CAPACITY);
                self.prepared_batches.retained_capacity = self.prepared_batches.batches.capacity();
                self.recent_peak_paint_operations = self.paint_operations.len();
                self.recent_peak_primitives = self.primitive_count();
            }
        }
    }

    fn prepare_batches(&mut self) {
        let mut prepared = Vec::with_capacity(self.prepared_batches.batches.capacity());
        let batches = self.batches().collect::<Vec<_>>();
        for batch in batches {
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
                PrimitiveBatch::SubpixelSprites {
                    texture_id,
                    sprites,
                } => PreparedSceneBatch::SubpixelSprites {
                    texture_id,
                    range: slice_range(&self.subpixel_sprites, sprites),
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

fn trim_vec_capacity<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    if vec.capacity() > floor.saturating_mul(multiplier) {
        vec.shrink_to(floor);
    }
}

fn slice_range<T>(whole: &[T], part: &[T]) -> Range<usize> {
    let start =
        part.as_ptr().addr().saturating_sub(whole.as_ptr().addr()) / std::mem::size_of::<T>();
    start..start.saturating_add(part.len())
}

fn is_solid_quad(quad: &Quad) -> bool {
    quad.background.tag == crate::BackgroundTag::Solid
        && !quad.border_widths.any(|width| !width.is_zero())
        && quad.corner_radii.is_zero()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveKind {
    Shadow,
    #[default]
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    SubpixelSprite,
    PolychromeSprite,
    Surface,
    BackdropBlur,
    GpuMesh3d,
}

pub(crate) enum PaintOperation {
    Primitive(Primitive),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
}

#[derive(Clone)]
pub(crate) enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(Path<ScaledPixels>),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    SubpixelSprite(SubpixelSprite),
    PolychromeSprite(PolychromeSprite),
    Surface(PaintSurface),
    BackdropBlur(PaintBackdropBlur),
    GpuMesh3d(PaintGpuMesh3d),
}

impl Primitive {
    fn set_order(&mut self, order: DrawOrder) {
        match self {
            Primitive::Shadow(shadow) => shadow.order = order,
            Primitive::Quad(quad) => quad.order = order,
            Primitive::Path(path) => path.order = order,
            Primitive::Underline(underline) => underline.order = order,
            Primitive::MonochromeSprite(sprite) => sprite.order = order,
            Primitive::SubpixelSprite(sprite) => sprite.order = order,
            Primitive::PolychromeSprite(sprite) => sprite.order = order,
            Primitive::Surface(surface) => surface.order = order,
            Primitive::BackdropBlur(blur) => blur.order = order,
            Primitive::GpuMesh3d(mesh) => mesh.order = order,
        }
    }

    pub fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::SubpixelSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
            Primitive::Surface(surface) => &surface.bounds,
            Primitive::BackdropBlur(blur) => &blur.bounds,
            Primitive::GpuMesh3d(mesh) => &mesh.bounds,
        }
    }

    pub fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::SubpixelSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
            Primitive::Surface(surface) => &surface.content_mask,
            Primitive::BackdropBlur(blur) => &blur.content_mask,
            Primitive::GpuMesh3d(mesh) => &mesh.content_mask,
        }
    }
}

#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
struct BatchIterator<'a> {
    shadows: &'a [Shadow],
    shadows_start: usize,
    shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    quads: &'a [Quad],
    quads_start: usize,
    quads_iter: Peekable<slice::Iter<'a, Quad>>,
    paths: &'a [Path<ScaledPixels>],
    paths_start: usize,
    paths_iter: Peekable<slice::Iter<'a, Path<ScaledPixels>>>,
    underlines: &'a [Underline],
    underlines_start: usize,
    underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    monochrome_sprites: &'a [MonochromeSprite],
    monochrome_sprites_start: usize,
    monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    subpixel_sprites: &'a [SubpixelSprite],
    subpixel_sprites_start: usize,
    subpixel_sprites_iter: Peekable<slice::Iter<'a, SubpixelSprite>>,
    polychrome_sprites: &'a [PolychromeSprite],
    polychrome_sprites_start: usize,
    polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
    surfaces: &'a [PaintSurface],
    surfaces_start: usize,
    surfaces_iter: Peekable<slice::Iter<'a, PaintSurface>>,
    backdrop_blurs: &'a [PaintBackdropBlur],
    backdrop_blurs_start: usize,
    backdrop_blurs_iter: Peekable<slice::Iter<'a, PaintBackdropBlur>>,
    gpu_meshes_3d: &'a [PaintGpuMesh3d],
    gpu_meshes_3d_start: usize,
    gpu_meshes_3d_iter: Peekable<slice::Iter<'a, PaintGpuMesh3d>>,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = PrimitiveBatch<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (
                self.shadows_iter.peek().map(|s| s.order),
                PrimitiveKind::Shadow,
            ),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (
                self.underlines_iter.peek().map(|u| u.order),
                PrimitiveKind::Underline,
            ),
            (
                self.monochrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::MonochromeSprite,
            ),
            (
                self.subpixel_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::SubpixelSprite,
            ),
            (
                self.polychrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::PolychromeSprite,
            ),
            (
                self.surfaces_iter.peek().map(|s| s.order),
                PrimitiveKind::Surface,
            ),
            (
                self.backdrop_blurs_iter.peek().map(|blur| blur.order),
                PrimitiveKind::BackdropBlur,
            ),
            (
                self.gpu_meshes_3d_iter.peek().map(|mesh| mesh.order),
                PrimitiveKind::GpuMesh3d,
            ),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, (second.0.unwrap_or(u32::MAX), second.1))
        } else {
            return None;
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self
                    .shadows_iter
                    .next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(
                    &self.shadows[shadows_start..shadows_end],
                ))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self
                    .quads_iter
                    .next_if(|quad| (quad.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(&self.quads[quads_start..quads_end]))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self
                    .paths_iter
                    .next_if(|path| (path.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(&self.paths[paths_start..paths_end]))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(
                    &self.underlines[underlines_start..underlines_end],
                ))
            }
            PrimitiveKind::MonochromeSprite => {
                let first_sprite = self.monochrome_sprites_iter.peek().unwrap();
                let texture_id = first_sprite.tile.texture_id;
                let sampling = first_sprite.sampling();
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                            && sprite.sampling() == sampling
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    sampling,
                    sprites: &self.monochrome_sprites[sprites_start..sprites_end],
                })
            }
            PrimitiveKind::SubpixelSprite => {
                let texture_id = self.subpixel_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.subpixel_sprites_start;
                let mut sprites_end = self.subpixel_sprites_start + 1;
                self.subpixel_sprites_iter.next();
                while self
                    .subpixel_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.subpixel_sprites_start = sprites_end;
                Some(PrimitiveBatch::SubpixelSprites {
                    texture_id,
                    sprites: &self.subpixel_sprites[sprites_start..sprites_end],
                })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = self.polychrome_sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    sprites: &self.polychrome_sprites[sprites_start..sprites_end],
                })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self
                    .surfaces_iter
                    .next_if(|surface| (surface.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(
                    &self.surfaces[surfaces_start..surfaces_end],
                ))
            }
            PrimitiveKind::BackdropBlur => {
                let blurs_start = self.backdrop_blurs_start;
                let mut blurs_end = blurs_start + 1;
                self.backdrop_blurs_iter.next();
                while self
                    .backdrop_blurs_iter
                    .next_if(|blur| (blur.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    blurs_end += 1;
                }
                self.backdrop_blurs_start = blurs_end;
                Some(PrimitiveBatch::BackdropBlurs(
                    &self.backdrop_blurs[blurs_start..blurs_end],
                ))
            }
            PrimitiveKind::GpuMesh3d => {
                let meshes_start = self.gpu_meshes_3d_start;
                let mut meshes_end = meshes_start + 1;
                self.gpu_meshes_3d_iter.next();
                while self
                    .gpu_meshes_3d_iter
                    .next_if(|mesh| (mesh.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    meshes_end += 1;
                }
                self.gpu_meshes_3d_start = meshes_end;
                Some(PrimitiveBatch::GpuMeshes3d(
                    &self.gpu_meshes_3d[meshes_start..meshes_end],
                ))
            }
        }
    }
}

#[derive(Debug)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveBatch<'a> {
    Shadows(&'a [Shadow]),
    Quads(&'a [Quad]),
    Paths(&'a [Path<ScaledPixels>]),
    Underlines(&'a [Underline]),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        sampling: MonochromeSpriteSampling,
        sprites: &'a [MonochromeSprite],
    },
    SubpixelSprites {
        texture_id: AtlasTextureId,
        sprites: &'a [SubpixelSprite],
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        sprites: &'a [PolychromeSprite],
    },
    Surfaces(&'a [PaintSurface]),
    BackdropBlurs(&'a [PaintBackdropBlur]),
    GpuMeshes3d(&'a [PaintGpuMesh3d]),
}

#[derive(Default, Debug, Clone)]
#[repr(C)]
pub(crate) struct Quad {
    pub order: DrawOrder,
    pub border_style: BorderStyle,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Hsla,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct Underline {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub thickness: ScaledPixels,
    pub wavy: u32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
pub(crate) struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

/// A data type representing a 2 dimensional transformation that can be applied to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct TransformationMatrix {
    /// 2x2 matrix containing rotation and scale,
    /// stored row-major
    pub rotation_scale: [[f32; 2]; 2],
    /// translation vector
    pub translation: [f32; 2],
}

impl Eq for TransformationMatrix {}

impl TransformationMatrix {
    /// The unit matrix, has no effect.
    pub fn unit() -> Self {
        Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [0.0, 0.0],
        }
    }

    /// Move the origin by a given point
    pub fn translate(mut self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [point.x.0, point.y.0],
        })
    }

    /// Clockwise rotation in radians around the origin
    pub fn rotate(self, angle: Radians) -> Self {
        self.compose(Self {
            rotation_scale: [
                [angle.0.cos(), -angle.0.sin()],
                [angle.0.sin(), angle.0.cos()],
            ],
            translation: [0.0, 0.0],
        })
    }

    /// Scale around the origin
    pub fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self {
            rotation_scale: [[size.width, 0.0], [0.0, size.height]],
            translation: [0.0, 0.0],
        })
    }

    /// Perform matrix multiplication with another transformation
    /// to produce a new transformation that is the result of
    /// applying both transformations: first, `other`, then `self`.
    #[inline]
    pub fn compose(self, other: TransformationMatrix) -> TransformationMatrix {
        if other == Self::unit() {
            return self;
        }
        // Perform matrix multiplication
        TransformationMatrix {
            rotation_scale: [
                [
                    self.rotation_scale[0][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0]
                    + self.rotation_scale[0][0] * other.translation[0]
                    + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1]
                    + self.rotation_scale[1][0] * other.translation[0]
                    + self.rotation_scale[1][1] * other.translation[1],
            ],
        }
    }

    /// Apply transformation to a point, mainly useful for debugging
    pub fn apply(&self, point: Point<Pixels>) -> Point<Pixels> {
        let input = [point.x.0, point.y.0];
        let mut output = self.translation;
        for (i, output_cell) in output.iter_mut().enumerate() {
            for (k, input_cell) in input.iter().enumerate() {
                *output_cell += self.rotation_scale[i][k] * *input_cell;
            }
        }
        Point::new(output[0].into(), output[1].into())
    }
}

impl Default for TransformationMatrix {
    fn default() -> Self {
        Self::unit()
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct MonochromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
    pub transformation: TransformationMatrix,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(crate) enum MonochromeSpriteSampling {
    Glyph = 0,
    Linear = 1,
}

impl MonochromeSprite {
    pub(crate) fn sampling(&self) -> MonochromeSpriteSampling {
        match self.pad {
            1 => MonochromeSpriteSampling::Linear,
            _ => MonochromeSpriteSampling::Glyph,
        }
    }
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct SubpixelSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
    pub transformation: TransformationMatrix,
}

impl From<SubpixelSprite> for Primitive {
    fn from(sprite: SubpixelSprite) -> Self {
        Primitive::SubpixelSprite(sprite)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct PolychromeSprite {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub grayscale: bool,
    pub opacity: f32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub tile: AtlasTile,
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

/// The backing content for a painted surface.
#[derive(Clone, Debug)]
pub(crate) enum SurfaceContent {
    #[cfg(target_os = "macos")]
    CoreVideo(core_video::pixel_buffer::CVPixelBuffer),
    #[cfg(not(target_os = "macos"))]
    Placeholder,
}

#[derive(Clone, Debug)]
pub(crate) struct PaintSurface {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub content: SurfaceContent,
}

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
}

/// Parameters for GPU-backed backdrop blur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackdropBlurStyle {
    /// Blur radius in logical pixels.
    pub radius: Pixels,
    /// Downsample factor used by backends that implement a separable GPU blur.
    pub downsample: u8,
    /// Number of Dual Kawase downsample/upsample levels.
    pub levels: u8,
    /// Saturation multiplier applied after blur.
    pub saturation: f32,
    /// Optional tint color blended over the blurred backdrop.
    pub tint: Option<Hsla>,
}

impl BackdropBlurStyle {
    /// Creates a blur style with conservative defaults for interactive UI.
    pub fn new(radius: Pixels) -> Self {
        Self {
            radius,
            downsample: 2,
            levels: 3,
            saturation: 1.0,
            tint: None,
        }
    }

    /// Sets the downsample factor. Values lower than one are clamped to one.
    pub fn downsample(mut self, downsample: u8) -> Self {
        self.downsample = downsample.max(1);
        self
    }

    /// Sets the number of Dual Kawase blur levels. Values are clamped to `1..=6`.
    pub fn levels(mut self, levels: u8) -> Self {
        self.levels = levels.clamp(1, 6);
        self
    }

    /// Sets the saturation multiplier.
    pub fn saturation(mut self, saturation: f32) -> Self {
        self.saturation = saturation.max(0.0);
        self
    }

    /// Sets a tint color blended over the blurred backdrop.
    pub fn tint(mut self, tint: Hsla) -> Self {
        self.tint = Some(tint);
        self
    }
}

impl From<Pixels> for BackdropBlurStyle {
    fn from(radius: Pixels) -> Self {
        Self::new(radius)
    }
}

impl From<f32> for BackdropBlurStyle {
    fn from(radius: f32) -> Self {
        Self::new(radius.into())
    }
}

impl From<f64> for BackdropBlurStyle {
    fn from(radius: f64) -> Self {
        Self::new(radius.into())
    }
}

/// Backdrop blur primitive emitted into the scene.
#[derive(Clone, Debug)]
pub(crate) struct PaintBackdropBlur {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub radius: ScaledPixels,
    pub downsample: u8,
    pub levels: u8,
    pub saturation: f32,
    pub tint: Option<Hsla>,
}

impl From<PaintBackdropBlur> for Primitive {
    fn from(blur: PaintBackdropBlur) -> Self {
        Primitive::BackdropBlur(blur)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
/// Stable identity for a GPU-backed 3D mesh.
pub struct GpuMesh3dId(pub usize);

/// A vertex in a GPU-backed 3D mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct GpuMesh3dVertex {
    /// Model-space x, y, z position.
    pub position: [f32; 3],
    /// Linear RGBA color used by the mesh fragment shader.
    pub color: [f32; 4],
}

/// A contiguous vertex range inside a GPU-backed 3D mesh.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct GpuMesh3dRange {
    /// First vertex in the range.
    pub start: u32,
    /// Number of vertices in the range.
    pub count: u32,
}

/// Draw ranges for the supported 3D mesh material passes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct GpuMesh3dDrawRanges {
    /// Opaque geometry drawn with depth writes enabled.
    pub opaque: GpuMesh3dRange,
    /// Transparent glass geometry drawn after opaque geometry.
    pub glass: GpuMesh3dRange,
    /// Transparent water geometry drawn after glass geometry.
    pub water: GpuMesh3dRange,
}

/// Camera parameters for a GPU-backed 3D mesh draw.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct GpuMesh3dCamera {
    /// Horizontal rotation in radians.
    pub yaw: f32,
    /// Vertical rotation in radians.
    pub pitch: f32,
    /// Zoom multiplier.
    pub zoom: f32,
}

/// Immutable 3D mesh data that can be drawn by GPUI's renderer.
#[derive(Clone, Debug)]
pub struct GpuMesh3d {
    /// Mesh identity used by renderers to cache uploaded vertex buffers.
    pub id: GpuMesh3dId,
    /// Mesh generation used to invalidate cached GPU buffers for this id.
    pub generation: u64,
    /// Packed vertex buffer for all draw ranges.
    pub vertices: Vec<GpuMesh3dVertex>,
    /// Material draw ranges within `vertices`.
    pub ranges: GpuMesh3dDrawRanges,
    /// Model-space center used by the mesh projection shader.
    pub center: [f32; 3],
    /// Fit scale used by the mesh projection shader.
    pub fit_scale: f32,
    /// Vertical scale used by the mesh projection shader.
    pub vertical_scale: f32,
}

impl GpuMesh3d {
    /// Creates a new GPU-backed 3D mesh with a fresh renderer cache id.
    pub fn new(
        vertices: Vec<GpuMesh3dVertex>,
        ranges: GpuMesh3dDrawRanges,
        center: [f32; 3],
        fit_scale: f32,
        vertical_scale: f32,
    ) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        Self {
            id: GpuMesh3dId(NEXT_ID.fetch_add(1, SeqCst)),
            generation: 0,
            vertices,
            ranges,
            center,
            fit_scale,
            vertical_scale,
        }
    }

    /// Sets the generation used by renderers to refresh cached GPU buffers.
    pub fn with_generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PaintGpuMesh3d {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub mesh: Arc<GpuMesh3d>,
    pub camera: GpuMesh3dCamera,
}

impl From<PaintGpuMesh3d> for Primitive {
    fn from(mesh: PaintGpuMesh3d) -> Self {
        Primitive::GpuMesh3d(mesh)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PathId(pub(crate) usize);

/// A line made up of a series of vertices and control points.
#[derive(Clone, Debug)]
pub struct Path<P: Clone + Debug + Default + PartialEq> {
    pub(crate) id: PathId,
    pub(crate) order: DrawOrder,
    pub(crate) bounds: Bounds<P>,
    pub(crate) content_mask: ContentMask<P>,
    pub(crate) vertices: Vec<PathVertex<P>>,
    pub(crate) color: Background,
    start: Point<P>,
    current: Point<P>,
    contour_count: usize,
}

impl Path<Pixels> {
    /// Create a new path with the given starting point.
    pub fn new(start: Point<Pixels>) -> Self {
        Self {
            id: PathId(0),
            order: DrawOrder::default(),
            vertices: Vec::new(),
            start,
            current: start,
            bounds: Bounds {
                origin: start,
                size: Default::default(),
            },
            content_mask: Default::default(),
            color: Default::default(),
            contour_count: 0,
        }
    }

    /// Scale this path by the given factor.
    pub fn scale(&self, factor: f32) -> Path<ScaledPixels> {
        Path {
            id: self.id,
            order: self.order,
            bounds: self.bounds.scale(factor),
            content_mask: self.content_mask.scale(factor),
            vertices: self
                .vertices
                .iter()
                .map(|vertex| vertex.scale(factor))
                .collect(),
            start: self.start.map(|start| start.scale(factor)),
            current: self.current.scale(factor),
            contour_count: self.contour_count,
            color: self.color,
        }
    }

    /// Move the start, current point to the given point.
    pub fn move_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        self.start = to;
        self.current = to;
    }

    /// Draw a straight line from the current point to the given point.
    pub fn line_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }

        self.push_triangle(
            (self.current, ctrl, to),
            (point(0., 0.), point(0.5, 0.), point(1., 1.)),
        );
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub fn push_triangle(
        &mut self,
        xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>),
        st: (Point<f32>, Point<f32>, Point<f32>),
    ) {
        self.bounds = self
            .bounds
            .union(&Bounds {
                origin: xy.0,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.1,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.2,
                size: Default::default(),
            });

        self.vertices.push(PathVertex {
            xy_position: xy.0,
            st_position: st.0,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.1,
            st_position: st.1,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.2,
            st_position: st.2,
            content_mask: Default::default(),
        });
    }
}

impl<T> Path<T>
where
    T: Clone + Debug + Default + PartialEq + PartialOrd + Add<T, Output = T> + Sub<Output = T>,
{
    #[allow(unused)]
    pub(crate) fn clipped_bounds(&self) -> Bounds<T> {
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub(crate) struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub(crate) xy_position: Point<P>,
    pub(crate) st_position: Point<f32>,
    pub(crate) content_mask: ContentMask<P>,
}

impl PathVertex<Pixels> {
    pub fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: self.xy_position.scale(factor),
            st_position: self.st_position,
            content_mask: self.content_mask.scale(factor),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DevicePixels, Hsla, bounds, px, size};

    fn monochrome_sprite(order: DrawOrder, pad: u32) -> MonochromeSprite {
        MonochromeSprite {
            order,
            pad,
            bounds: bounds(
                point(ScaledPixels(0.0), ScaledPixels(0.0)),
                size(ScaledPixels(1.0), ScaledPixels(1.0)),
            ),
            content_mask: ContentMask {
                bounds: bounds(
                    point(ScaledPixels(0.0), ScaledPixels(0.0)),
                    size(ScaledPixels(10.0), ScaledPixels(10.0)),
                ),
            },
            color: Hsla::default(),
            tile: AtlasTile {
                texture_id: AtlasTextureId {
                    index: 0,
                    kind: crate::AtlasTextureKind::Monochrome,
                },
                tile_id: crate::TileId(0),
                padding: 1,
                bounds: bounds(
                    point(DevicePixels(1), DevicePixels(1)),
                    size(DevicePixels(1), DevicePixels(1)),
                ),
            },
            transformation: TransformationMatrix::unit(),
        }
    }

    #[test]
    fn monochrome_sprite_batches_split_by_sampling() {
        let mut scene = Scene::default();
        scene.insert_primitive(monochrome_sprite(0, MonochromeSpriteSampling::Glyph as u32));
        scene.insert_primitive(monochrome_sprite(
            0,
            MonochromeSpriteSampling::Linear as u32,
        ));
        scene.finish();

        let batches = scene.batches().collect::<Vec<_>>();
        let monochrome_batches = batches
            .iter()
            .filter(|batch| matches!(batch, PrimitiveBatch::MonochromeSprites { .. }))
            .count();

        assert_eq!(monochrome_batches, 2);
    }

    #[test]
    fn prepared_quad_runs_split_solid_and_bordered_quads() {
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(10.0), px(10.0))).scale(1.0);
        let content_mask = ContentMask { bounds };
        let mut scene = Scene::default();
        scene.insert_primitive(Quad {
            bounds,
            content_mask: content_mask.clone(),
            background: Hsla::white().into(),
            ..Quad::default()
        });
        scene.insert_primitive(Quad {
            bounds,
            content_mask,
            background: Hsla::white().into(),
            border_color: Hsla::black(),
            border_widths: Edges::all(ScaledPixels(1.0)),
            ..Quad::default()
        });

        scene.finish();

        let quad_runs = scene
            .prepared_batches()
            .iter()
            .filter_map(|batch| match batch {
                PreparedSceneBatch::Quads(run) => Some(run),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(quad_runs.len(), 2);
        assert!(quad_runs[0].is_solid);
        assert!(!quad_runs[1].is_solid);
        assert_eq!(quad_runs[0].range, 0..1);
        assert_eq!(quad_runs[1].range, 1..2);
    }

    #[test]
    fn scene_batches_gpu_mesh_3d_in_draw_order() {
        let mesh = Arc::new(GpuMesh3d::new(
            vec![GpuMesh3dVertex {
                position: [0.0, 0.0, 0.0],
                color: [1.0, 1.0, 1.0, 1.0],
            }],
            GpuMesh3dDrawRanges {
                opaque: GpuMesh3dRange { start: 0, count: 1 },
                glass: GpuMesh3dRange::default(),
                water: GpuMesh3dRange::default(),
            },
            [0.0, 0.0, 0.0],
            1.0,
            1.0,
        ));
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(10.0), px(10.0))).scale(1.0);
        let content_mask = ContentMask { bounds };
        let camera = GpuMesh3dCamera {
            yaw: 0.0,
            pitch: 0.0,
            zoom: 1.0,
        };
        let mut scene = Scene::default();

        scene.insert_primitive(Quad {
            bounds,
            content_mask: content_mask.clone(),
            ..Quad::default()
        });
        scene.insert_primitive(PaintGpuMesh3d {
            order: 0,
            bounds,
            content_mask,
            mesh: mesh.clone(),
            camera,
        });
        scene.finish();

        let batches = scene.batches().collect::<Vec<_>>();
        assert!(matches!(batches[0], PrimitiveBatch::Quads(_)));
        let PrimitiveBatch::GpuMeshes3d(meshes) = batches[1] else {
            panic!("expected gpu mesh batch");
        };
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].mesh.id, mesh.id);
        assert_eq!(meshes[0].camera, camera);
    }

    #[test]
    fn gpu_mesh_3d_generation_is_stable_for_camera_changes() {
        let mesh = GpuMesh3d::new(
            vec![GpuMesh3dVertex {
                position: [1.0, 2.0, 3.0],
                color: [0.25, 0.5, 0.75, 1.0],
            }],
            GpuMesh3dDrawRanges::default(),
            [0.0, 0.0, 0.0],
            1.0,
            1.0,
        )
        .with_generation(42);
        let before_id = mesh.id;
        let before_generation = mesh.generation;
        let camera_a = GpuMesh3dCamera {
            yaw: 0.0,
            pitch: 0.68,
            zoom: 1.0,
        };
        let camera_b = GpuMesh3dCamera {
            yaw: 1.0,
            pitch: 0.68,
            zoom: 1.8,
        };

        assert_ne!(camera_a, camera_b);
        assert_eq!(mesh.id, before_id);
        assert_eq!(mesh.generation, before_generation);
    }
}
