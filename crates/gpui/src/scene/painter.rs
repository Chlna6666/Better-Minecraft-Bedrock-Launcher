use crate::{Bounds, ScaledPixels};
use collections::FxHashSet;

use super::BoundsTree;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use super::geometry::{is_solid_quad, slice_range, trim_vec_capacity};
use super::{
    BlurCapture, DrawOrder, PaintBlur, PaintOperation, PathId, PreparedBackdropBlurGroup,
    PreparedQuadRun, PreparedSceneBatch, Primitive, PrimitiveBatch, SceneAnimationId,
    SceneAnimationValue, blur_influence_radius,
};

use super::{QuadReplay, Scene};
mod lifecycle;
mod replay;
/// UI-owned drawing builder. Published Scene payload is moved out at commit.
#[derive(Default)]
pub(crate) struct Painter {
    pub(crate) scene: Scene,
    primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<DrawOrder>,
    next_scene_animation_id: u32,
    pub(super) retained_prefix_invalid: bool,
    pub(super) retained_prefix_verified_len: usize,
    idle_clear_frames: u16,
    recent_peak_paint_operations: usize,
    recent_peak_primitives: usize,
    blur_captures: Vec<BlurCaptureState>,
}
struct BlurCaptureState {
    config: BlurCapture,
    scene: Box<Painter>,
    previous_content: Option<std::sync::Arc<Scene>>,
}
impl std::ops::Deref for Painter {
    type Target = Scene;
    fn deref(&self) -> &Scene {
        &self.scene
    }
}
impl std::ops::DerefMut for Painter {
    fn deref_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }
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
    Blur,
    RendererExtension,
}

const SCENE_IDLE_TRIM_FRAMES: u16 = 45;
const SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER: usize = 2;
const SCENE_MIN_RETAINED_CAPACITY: usize = 24;
const ENGINE_ANIMATION_ID_START: u32 = 1 << 31;

impl Painter {
    pub(crate) fn construction_capacity(&self) -> usize {
        self.primitive_bounds.retained_capacity()
            + self.layer_stack.capacity()
            + self.blur_captures.capacity()
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        self.scene.revision = 0;
        if let Some(capture) = self.blur_captures.last_mut() {
            capture.scene.push_layer(bounds);
            self.scene
                .paint_operations
                .push(PaintOperation::StartLayer(bounds));
        } else {
            self.push_replayed_layer(bounds);
        }
    }

    fn push_replayed_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
        self.scene
            .paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    pub fn pop_layer(&mut self) {
        self.scene.revision = 0;
        if let Some(capture) = self.blur_captures.last_mut() {
            capture.scene.pop_layer();
            self.scene.paint_operations.push(PaintOperation::EndLayer);
        } else {
            self.pop_replayed_layer();
        }
    }

    fn pop_replayed_layer(&mut self) {
        self.layer_stack.pop();
        self.scene.paint_operations.push(PaintOperation::EndLayer);
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        self.scene.revision = 0;
        let primitive = primitive.into();
        if let Some(capture) = self.blur_captures.last_mut() {
            let operation_start = capture.scene.paint_operations.len();
            capture.scene.insert_primitive(primitive);
            let captured_primitive = capture
                .scene
                .paint_operations
                .get(operation_start)
                .and_then(|operation| match operation {
                    PaintOperation::Primitive(primitive) => Some(primitive.clone()),
                    PaintOperation::StartLayer(_)
                    | PaintOperation::EndLayer
                    | PaintOperation::StartBlur(_)
                    | PaintOperation::EndBlur => None,
                });
            if let Some(primitive) = captured_primitive {
                self.scene
                    .paint_operations
                    .push(PaintOperation::Primitive(primitive));
            }
            return;
        }

        self.insert_primitive_direct(primitive);
    }

    fn insert_primitive_direct(&mut self, primitive: Primitive) {
        let Some(order) = self.order_for_primitive(&primitive) else {
            return;
        };
        self.push_ordered_primitive(primitive, order, true);
    }

    pub(crate) fn begin_blur(&mut self, config: BlurCapture) {
        self.begin_previous_blur(config, None);
    }

    pub(crate) fn begin_previous_blur(
        &mut self,
        config: BlurCapture,
        previous_content: Option<std::sync::Arc<Scene>>,
    ) {
        self.scene.revision = 0;
        self.scene
            .paint_operations
            .push(PaintOperation::StartBlur(config.clone()));
        self.blur_captures.push(BlurCaptureState {
            config,
            scene: Box::default(),
            previous_content,
        });
    }

    pub(crate) fn is_capturing_blur(&self) -> bool {
        !self.blur_captures.is_empty()
    }

    pub(crate) fn end_blur(&mut self) {
        self.scene.revision = 0;
        let Some(capture) = self.blur_captures.pop() else {
            debug_assert!(false, "ending an element blur without a matching begin");
            return;
        };

        let mut content = *capture.scene;
        let Some(content_bounds) = content
            .bounds_for_range(0..content.len())
            .map(|bounds| bounds.union(&capture.config.bounds))
        else {
            self.scene.paint_operations.push(PaintOperation::EndBlur);
            return;
        };
        let effect_bounds = content_bounds.dilate(blur_influence_radius(capture.config.radius));
        let config = capture.config;
        let content = if let Some(previous) = capture.previous_content {
            if content.finish_against(&previous)
                && content.scene.animation_values == previous.animation_values
            {
                previous
            } else {
                std::sync::Arc::new(content.into_scene())
            }
        } else {
            content.finish();
            std::sync::Arc::new(content.into_scene())
        };
        let blur = PaintBlur {
            order: 0,
            animation_id: config.animation_id,
            layer_id: config.layer_id,
            bounds: effect_bounds,
            content_mask: config.content_mask,
            radius: config.radius,
            opacity: config.opacity,
            content,
        };

        if let Some(parent) = self.blur_captures.last_mut() {
            parent.scene.insert_primitive(blur);
        } else {
            let primitive = Primitive::from(blur);
            let Some(order) = self.order_for_primitive(&primitive) else {
                self.scene.paint_operations.push(PaintOperation::EndBlur);
                return;
            };
            self.push_ordered_primitive(primitive, order, false);
        }
        self.scene.paint_operations.push(PaintOperation::EndBlur);
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
        self.scene.animation_values.push(value);
    }

    pub(crate) fn replace_animation_values(
        &mut self,
        values: impl IntoIterator<Item = SceneAnimationValue>,
    ) {
        self.scene.animation_values.clear();
        self.scene.animation_values.extend(values);
    }

    fn order_for_primitive(&mut self, primitive: &Primitive) -> Option<DrawOrder> {
        let clipped_bounds = Self::ordering_bounds(primitive);

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

    fn ordering_bounds(primitive: &Primitive) -> Bounds<ScaledPixels> {
        // A retained animation can move geometry into its fixed clip after paint.
        // Reserve ordering across that clip so replay cannot discard the primitive
        // or reorder static siblings when the presentation sample changes.
        if primitive.animation_id().is_some() {
            primitive.content_mask().bounds
        } else {
            primitive
                .bounds()
                .intersect(&primitive.content_mask().bounds)
        }
    }

    fn push_ordered_primitive(
        &mut self,
        mut primitive: Primitive,
        order: DrawOrder,
        record_operation: bool,
    ) {
        match &mut primitive {
            Primitive::Shadow(shadow) => {
                shadow.order = order;
                self.scene.shadows.push(*shadow);
            }
            Primitive::Quad(quad) => {
                quad.order = order;
                self.scene.quads.push(*quad);
            }
            Primitive::Path(path) => {
                path.order = order;
                path.id = PathId(self.scene.paths.len());
                self.scene.paths.push(path.clone());
            }
            Primitive::Underline(underline) => {
                underline.order = order;
                self.scene.underlines.push(*underline);
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.order = order;
                self.scene.monochrome_sprites.push(*sprite);
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.order = order;
                self.scene.polychrome_sprites.push(*sprite);
            }
            Primitive::Surface(surface) => {
                surface.order = order;
                self.scene.surfaces.push(surface.clone());
            }
            Primitive::BackdropBlur(blur) => {
                blur.order = order;
                self.scene.backdrop_blurs.push(blur.clone());
            }
            Primitive::Blur(blur) => {
                blur.order = order;
                self.scene.blurs.push(blur.clone());
            }
            Primitive::RendererExtension(extension) => {
                extension.order = order;
                self.scene.renderer_extensions.push(extension.clone());
            }
        }
        if record_operation {
            self.scene
                .paint_operations
                .push(PaintOperation::Primitive(primitive));
        }
    }
    /// Move the completed payload into its immutable publication without cloning it.
    pub(crate) fn take_scene(&mut self) -> Scene {
        std::mem::take(&mut self.scene)
    }
    /// Consume this builder when finishing a nested capture or a synthetic drawing.
    pub(crate) fn into_scene(self) -> Scene {
        self.scene
    }
}
