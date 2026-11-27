use crate::{Bounds, ScaledPixels};
use collections::FxHashSet;

use super::BoundsTree;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use super::geometry::{is_solid_quad, slice_range, trim_vec_capacity};
use super::{
    BlurCapture, DrawOrder, PaintBlur, PaintOperation, PathId, PreparedBackdropBlurGroup,
    PreparedQuadRun, PreparedSceneBatch, Primitive, PrimitiveBatch, PrimitiveRef, PaintOperationRef, SceneAnimationId,
    SceneAnimationValue, blur_influence_radius,
};

use super::{Operations, QuadReplay, Scene};
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
    scroll_capture: Option<Operations>,
    // Window Frame construction exposes logical operations immediately,
    // but only exposes sorted draw payload at finish. Standalone painters
    // preserve the older immediate draw-read contract.
    frame_builder: bool,
    draw_payload_pending: bool,
    draw_permutation: super::primitive_stream::DrawPermutation,
    draw_scratch_low_frames: u16,
}
struct BlurCaptureState {
    config: BlurCapture,
    operation_start: usize,
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

const SCENE_IDLE_TRIM_FRAMES: u16 = 45;
const SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER: usize = 2;
const SCENE_MIN_RETAINED_CAPACITY: usize = 24;
const ENGINE_ANIMATION_ID_START: u32 = 1 << 31;

impl Painter {
    pub(crate) fn for_frame() -> Self {
        Self { frame_builder: true, ..Self::default() }
    }

    /// Freeze a new drawable operation page before appending retained
    /// ranges or entering a record-only capture. Nothing is visited when
    /// the current frame has no new typed draw payload.
    fn flush_draw_payloads(&mut self) {
        if !self.draw_payload_pending { return; }
        self.scene.paint_operations.publish_draw_payloads(
            &mut self.scene.shadows,
            &mut self.scene.quads,
            &mut self.scene.monochrome_sprites,
            &mut self.scene.polychrome_sprites,
            &mut self.draw_permutation,
            true,
        );
        self.scene.paint_operations.finish();
        self.draw_payload_pending = false;
    }

    pub(crate) fn begin_scroll_capture(&mut self) {
        debug_assert!(self.scroll_capture.is_none() && self.blur_captures.is_empty());
        self.scroll_capture = Some(Operations::default());
    }

    pub(crate) fn end_scroll_capture(&mut self) -> Operations {
        self.scroll_capture.take().expect("open scroll paint capture")
    }

    /// An image can be evicted while its element is still being captured.
    /// Pending capture operations have not reached the ordinary Scene yet.
    pub(crate) fn collect_polychrome_tile_ids_into(&self, live: &mut FxHashSet<(crate::AtlasTextureId, u32)>) {
        self.scene.collect_polychrome_tile_ids_into(live);
        // Image eviction may run during Paint, before the new draw page is
        // published. The typed pending owner is the authoritative source.
        if self.frame_builder {
            self.scene.paint_operations.collect_pending_poly_tiles(live);
        }
    }

    pub(crate) fn collect_scroll_capture_tile_ids_into(
        &self,
        live: &mut FxHashSet<(crate::AtlasTextureId, u32)>,
    ) {
        if let Some(capture) = &self.scroll_capture {
            for operation in capture.views() {
                if let PaintOperationRef::Primitive(PrimitiveRef::PolychromeSprite(sprite)) = operation {
                    live.insert((sprite.tile.texture_id, sprite.tile.tile_id.0));
                }
            }
        }
    }

    /// Record a logical placement without visiting the frozen source.
    /// The root ordering tree is provisional until the owner chooses
    /// composition or reconstructs the ordinary Scene. Do not import
    /// native ordering proofs from across this boundary.
    pub(crate) fn push_deferred_scroll(&mut self, placement: crate::window::scroll_paint::ScrollPlacement) -> bool {
        if !self.layer_stack.is_empty() && !placement.has_scoped_order() {
            return false;
        }
        self.retain_deferred_scroll(placement)
    }

    /// A retained view may be replayed in an ordinary scope after a
    /// structural edit. Keep its logical indices intact; the root proof
    /// certifies disjoint scopes or the owner rebuilds their DrawOrder.
    fn retain_deferred_scroll(&mut self, placement: crate::window::scroll_paint::ScrollPlacement) -> bool {
        if self.scroll_capture.is_some() || !self.blur_captures.is_empty() {
            return false;
        }
        self.scene.revision = 0;
        self.flush_draw_payloads();
        // Native placements do not insert a leaf into the ordinary
        // BoundsTree. Later ordinary replay can still prove its exact
        // prefix against the previous logical marker domain. Composition
        // ordering is certified independently at Paint publication.
        self.scene.paint_operations.push_scroll_placement(placement);
        true
    }

    pub(crate) fn construction_capacity(&self) -> usize {
        self.primitive_bounds.retained_capacity()
            + self.layer_stack.capacity()
            + self.blur_captures.capacity()
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        if let Some(capture) = &mut self.scroll_capture {
            capture.push(PaintOperation::StartLayer(bounds));
            return;
        }
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
        if let Some(capture) = &mut self.scroll_capture {
            capture.push(PaintOperation::EndLayer);
            return;
        }
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
        let primitive = primitive.into();
        if let Some(capture) = &mut self.scroll_capture {
            capture.push(PaintOperation::Primitive(primitive));
            return;
        }
        self.scene.revision = 0;
        if let Some(capture) = self.blur_captures.last_mut() {
            let operation_start = capture.scene.paint_operations.len();
            capture.scene.insert_primitive(primitive);
            let captured_primitive = capture
                .scene
                .paint_operations
                .view(operation_start)
                .and_then(|operation| match operation {
                    PaintOperationRef::Primitive(primitive) => Some(primitive.to_primitive()),
                    PaintOperationRef::StartLayer(_)
                    | PaintOperationRef::EndLayer
                    | PaintOperationRef::StartBlur(_)
                    | PaintOperationRef::EndBlur
                    | PaintOperationRef::DeferredScroll(_) => None,
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
        let Some(order) = self.order_for_primitive(primitive.as_ref()) else {
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
        self.flush_draw_payloads();
        self.scene.revision = 0;
        let operation_start = self.scene.paint_operations.len();
        self.scene
            .paint_operations
            .push(PaintOperation::StartBlur(config.clone()));
        self.blur_captures.push(BlurCaptureState {
            config,
            operation_start,
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

        let start = capture.operation_start;
        let mut content = *capture.scene;
        // A leaf capture mirrors exactly the primitives accepted by its
        // content painter. For nonzero filters the capture seed is final, so
        // compute both extents from one traversal, in their original fold
        // order. Nested groups and identity filters need the general path.
        let (content_bounds, group_source_bounds) = if capture.config.radius.0 != 0.0
            && content.capture_groups.is_empty()
            && self.scene.capture_groups.last().is_none_or(|group| group.range.end <= start)
            && self.scene.len() - start - 1 == content.len()
        {
            let mut content_bounds: Option<Bounds<ScaledPixels>> = None;
            let mut group_bounds = capture.config.bounds;
            for bounds in content.paint_operations.views().filter_map(PaintOperationRef::visual_bounds) {
                content_bounds = Some(content_bounds.map_or(bounds, |old| old.union(&bounds)));
                group_bounds = group_bounds.union(&bounds);
            }
            (content_bounds, Some(group_bounds))
        } else {
            (content.bounds_for_range(0..content.len()), None)
        };
        let Some(content_bounds) = content_bounds else {
            self.close_capture_group(start, &capture.config, group_source_bounds);
            return;
        };
        let mut config = capture.config;
        let effect_bounds = if config.radius.0 == 0.0 {
            // A zero-filter layer has no Gaussian support. Its actual pixel
            // footprint is already known from the captured content, including
            // nested captures. Do not union the enclosing layout rectangle.
            let tight = content_bounds.intersect(&config.content_mask.bounds);
            if !tight.is_empty() {
                config.bounds = tight;
                let end = self.scene.paint_operations.len();
                let marker_changed = self.scene.paint_operations.view(capture.operation_start)
                    .is_some_and(|operation| matches!(operation, PaintOperationRef::StartBlur(marker) if marker.bounds != tight));
                if marker_changed {
                    self.scene.paint_operations.tighten_capture(capture.operation_start, end, tight);
                }
            }
            tight
        } else {
            content_bounds.union(&config.bounds).dilate(blur_influence_radius(config.radius))
        };
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

        let output = if let Some(parent) = self.blur_captures.last_mut() {
            parent.scene.insert_primitive(blur);
            None
        } else {
            let revision = blur.content.revision;
            let primitive = Primitive::from(blur);
            let Some(order) = self.order_for_primitive(primitive.as_ref()) else {
                self.close_capture_group(start, &config, group_source_bounds);
                return;
            };
            self.push_ordered_primitive(primitive, order, false);
            Some((revision, order))
        };
        self.close_capture_group(start, &config, group_source_bounds);
        self.scene.capture_groups.last_mut().expect("closed capture").output = output;
    }

    fn close_capture_group(
        &mut self,
        start: usize,
        config: &BlurCapture,
        source_bounds: Option<Bounds<ScaledPixels>>,
    ) {
        let group = match source_bounds {
            Some(bounds) => super::geometry::CaptureGroup::from_source_bounds(
                start..self.scene.len() + 1, config, bounds,
            ),
            None => super::geometry::CaptureGroup::close(&self.scene, start, config),
        };
        #[cfg(test)]
        {
            let reference = super::geometry::CaptureGroup::close(&self.scene, start, config);
            assert_eq!(group.range, reference.range);
            assert_eq!(group.bounds, reference.bounds);
        }
        self.scene.capture_groups.push(group);
        self.scene.paint_operations.push(PaintOperation::EndBlur);
        // Root capture mirrors are logical replay data, not root draws.
        // Close their page before ordinary root primitives can append.
        if self.frame_builder && self.blur_captures.is_empty() {
            self.scene.paint_operations.finish();
        }
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

    fn order_for_primitive(&mut self, primitive: PrimitiveRef<'_>) -> Option<DrawOrder> {
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

    fn ordering_bounds(primitive: PrimitiveRef<'_>) -> Bounds<ScaledPixels> {
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
        if self.frame_builder && record_operation
            && matches!(&primitive, Primitive::Shadow(_) | Primitive::Quad(_) | Primitive::MonochromeSprite(_) | Primitive::PolychromeSprite(_))
        {
            primitive.set_order(order);
            self.scene.paint_operations.push(PaintOperation::Primitive(primitive));
            self.draw_payload_pending = true;
            return;
        }
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
        self.flush_draw_payloads();
        std::mem::take(&mut self.scene)
    }
    /// Consume this builder when finishing a nested capture or a synthetic drawing.
    pub(crate) fn into_scene(mut self) -> Scene {
        self.flush_draw_payloads();
        self.scene
    }
}
