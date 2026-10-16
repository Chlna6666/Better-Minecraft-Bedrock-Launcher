use crate::{Bounds, ScaledPixels, SceneFrameMetrics};
use collections::FxHashSet;

use super::BoundsTree;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use super::geometry::{is_solid_quad, slice_range, trim_vec_capacity};
use super::{
    BatchIterator, BlurCapture, DrawOrder, MonochromeSprite, PaintBackdropBlur, PaintBlur,
    PaintOperation, PaintRendererExtension, PaintSurface, Path, PathId, PolychromeSprite,
    PreparedBackdropBlurGroup, PreparedQuadRun, PreparedSceneBatch, PreparedSceneBatches,
    Primitive, PrimitiveBatch, SceneAnimationId, SceneAnimationValue, Shadow, Underline,
    blur_influence_radius,
};

#[derive(Default)]
pub(crate) struct Scene {
    /// Identity of the last completed static display list. Animation values are independent.
    pub(crate) revision: u64,
    pub(crate) paint_operations: Vec<PaintOperation>,
    primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<DrawOrder>,
    pub(crate) shadows: Vec<Shadow>,
    pub(crate) quads: super::QuadStream,
    pub(crate) paths: Vec<Path<ScaledPixels>>,
    pub(crate) underlines: Vec<Underline>,
    pub(crate) monochrome_sprites: Vec<MonochromeSprite>,
    pub(crate) polychrome_sprites: Vec<PolychromeSprite>,
    pub(crate) surfaces: Vec<PaintSurface>,
    pub(crate) backdrop_blurs: Vec<PaintBackdropBlur>,
    pub(crate) blurs: Vec<PaintBlur>,
    pub(crate) renderer_extensions: Vec<PaintRendererExtension>,
    /// Scene-local animation samples emitted while building this display list.
    ///
    /// Engine-owned presentation samples intentionally live outside Scene so retained display-list
    /// commits stay immutable while the compositor advances between UI commits.
    pub(crate) animation_values: Vec<SceneAnimationValue>,
    next_scene_animation_id: u32,
    prepared_batches: PreparedSceneBatches,
    replayed_primitives: usize,
    pub(super) retained_prefix_invalid: bool,
    pub(super) retained_prefix_verified_len: usize,
    idle_clear_frames: u16,
    recent_peak_paint_operations: usize,
    recent_peak_primitives: usize,
    blur_captures: Vec<BlurCaptureState>,
}

struct BlurCaptureState {
    config: BlurCapture,
    scene: Box<Scene>,
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

impl Scene {
    pub fn clear(&mut self) {
        self.revision = 0;
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
        self.blurs.clear();
        self.renderer_extensions.clear();
        self.animation_values.clear();
        self.next_scene_animation_id = 0;
        self.prepared_batches.clear();
        self.replayed_primitives = 0;
        self.retained_prefix_invalid = false;
        self.retained_prefix_verified_len = 0;
        self.blur_captures.clear();

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
        for operation in self.paint_operations.get(range.clone())? {
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

    pub(crate) fn animation_value(
        &self,
        animation_id: SceneAnimationId,
    ) -> Option<&SceneAnimationValue> {
        self.animation_values
            .iter()
            .find(|value| value.animation_id == animation_id)
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        self.revision = 0;
        if let Some(capture) = self.blur_captures.last_mut() {
            capture.scene.push_layer(bounds);
            self.paint_operations
                .push(PaintOperation::StartLayer(bounds));
        } else {
            self.push_replayed_layer(bounds);
        }
    }

    fn push_replayed_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
        self.paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    pub fn pop_layer(&mut self) {
        self.revision = 0;
        if let Some(capture) = self.blur_captures.last_mut() {
            capture.scene.pop_layer();
            self.paint_operations.push(PaintOperation::EndLayer);
        } else {
            self.pop_replayed_layer();
        }
    }

    fn pop_replayed_layer(&mut self) {
        self.layer_stack.pop();
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        self.revision = 0;
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
                self.paint_operations
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
        self.revision = 0;
        self.paint_operations
            .push(PaintOperation::StartBlur(config.clone()));
        self.blur_captures.push(BlurCaptureState {
            config,
            scene: Box::default(),
        });
    }

    pub(crate) fn is_capturing_blur(&self) -> bool {
        !self.blur_captures.is_empty()
    }

    pub(crate) fn end_blur(&mut self) {
        self.revision = 0;
        let Some(capture) = self.blur_captures.pop() else {
            debug_assert!(false, "ending an element blur without a matching begin");
            return;
        };

        let mut content = *capture.scene;
        let Some(content_bounds) = content
            .bounds_for_range(0..content.len())
            .map(|bounds| bounds.union(&capture.config.bounds))
        else {
            self.paint_operations.push(PaintOperation::EndBlur);
            return;
        };
        let effect_bounds = content_bounds.dilate(blur_influence_radius(capture.config.radius));
        let config = capture.config;
        content.finish();
        let blur = PaintBlur {
            order: 0,
            animation_id: config.animation_id,
            layer_id: config.layer_id,
            bounds: effect_bounds,
            content_mask: config.content_mask,
            radius: config.radius,
            opacity: config.opacity,
            content: std::sync::Arc::new(content),
        };

        if let Some(parent) = self.blur_captures.last_mut() {
            parent.scene.insert_primitive(blur);
        } else {
            let primitive = Primitive::from(blur);
            let Some(order) = self.order_for_primitive(&primitive) else {
                self.paint_operations.push(PaintOperation::EndBlur);
                return;
            };
            self.push_ordered_primitive(primitive, order, false);
        }
        self.paint_operations.push(PaintOperation::EndBlur);
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

    pub(crate) fn replace_animation_values(
        &mut self,
        values: impl IntoIterator<Item = SceneAnimationValue>,
    ) {
        self.animation_values.clear();
        self.animation_values.extend(values);
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
        for operation in &self.paint_operations {
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
                self.shadows.push(*shadow);
            }
            Primitive::Quad(quad) => {
                quad.order = order;
                self.quads.push(*quad);
            }
            Primitive::Path(path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path.clone());
            }
            Primitive::Underline(underline) => {
                underline.order = order;
                self.underlines.push(*underline);
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.order = order;
                self.monochrome_sprites.push(*sprite);
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(*sprite);
            }
            Primitive::Surface(surface) => {
                surface.order = order;
                self.surfaces.push(surface.clone());
            }
            Primitive::BackdropBlur(blur) => {
                blur.order = order;
                self.backdrop_blurs.push(blur.clone());
            }
            Primitive::Blur(blur) => {
                blur.order = order;
                self.blurs.push(blur.clone());
            }
            Primitive::RendererExtension(extension) => {
                extension.order = order;
                self.renderer_extensions.push(extension.clone());
            }
        }
        if record_operation {
            self.paint_operations
                .push(PaintOperation::Primitive(primitive));
        }
    }

    fn replay_primitive(&mut self, primitive: &Primitive, retain_order: bool) {
        // A retained-order proof belongs to the exact ordering prefix that produced it.
        // If an enclosing layer was rebuilt with a different DrawOrder, keeping the
        // primitive's old order would mix two frame-local ordering domains. Rebind to
        // the current layer and invalidate the remainder of this retained prefix.
        let retain_order = retain_order && !self.retained_prefix_invalid;
        let order = if retain_order {
            let clipped_bounds = Self::ordering_bounds(primitive);
            if clipped_bounds.is_empty() {
                return;
            }
            if let Some(layer_order) = self.layer_stack.last().copied() {
                if layer_order != primitive.order() {
                    self.retained_prefix_invalid = true;
                }
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
                let mut shadow = *shadow;
                shadow.order = order;
                self.shadows.push(shadow);
                ScenePrimitiveKind::Shadow
            }
            Primitive::Quad(quad) => {
                let mut quad = *quad;
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
                let mut underline = *underline;
                underline.order = order;
                self.underlines.push(underline);
                ScenePrimitiveKind::Underline
            }
            Primitive::MonochromeSprite(sprite) => {
                let mut sprite = *sprite;
                sprite.order = order;
                self.monochrome_sprites.push(sprite);
                ScenePrimitiveKind::MonochromeSprite
            }
            Primitive::PolychromeSprite(sprite) => {
                let mut sprite = *sprite;
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
            Primitive::Blur(blur) => {
                let mut blur = blur.clone();
                blur.order = order;
                self.blurs.push(blur);
                ScenePrimitiveKind::Blur
            }
            Primitive::RendererExtension(extension) => {
                let mut extension = extension.clone();
                extension.order = order;
                self.renderer_extensions.push(extension);
                ScenePrimitiveKind::RendererExtension
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

    fn replay_animation_values(&mut self, range: Range<usize>, previous: &Scene) {
        if previous.animation_values.is_empty() {
            return;
        }
        let referenced: FxHashSet<_> = previous.paint_operations[range]
            .iter()
            .filter_map(|operation| match operation {
                PaintOperation::Primitive(primitive) => primitive.animation_id(),
                _ => None,
            })
            .collect();
        let mut emitted: FxHashSet<_> = self
            .animation_values
            .iter()
            .map(|value| (value.animation_id, value.property))
            .collect();
        // Samples are part of the replayed content's binding proof. Keep a freshly
        // emitted sample when an executing wrapper retargets the same stable id.
        self.animation_values.extend(
            previous
                .animation_values
                .iter()
                .filter(|value| {
                    referenced.contains(&value.animation_id)
                        && emitted.insert((value.animation_id, value.property))
                })
                .copied(),
        );
    }

    /// Replays Paint operations and reports whether their draw-order mapping was preserved.
    /// Retention owners use this result to decide whether separately owned Chunk metadata is reusable.
    /// Quad spans must be qualified against this exact source generation by the caller;
    /// their payload is shared only while the current Layer and ordering prefix still match.
    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene, quad_spans: &[super::QuadReplay]) -> bool {
        self.replay_animation_values(range.clone(), prev_scene);
        self.revision = 0;
        let range_end = range.end;
        let retain_order = !self.retained_prefix_invalid
            && self.paint_operations.len() == range.start
            && self.ordering_prefix_matches_previous(prev_scene, range.start);
        if !retain_order {
            self.retained_prefix_invalid = true;
        }
        let mut index = range.start;
        let mut spans = quad_spans.iter().peekable();
        while index < range.end {
            while spans.peek().is_some_and(|span| span.paint_range.start < index) { spans.next(); }
            if let Some(span) = spans.peek()
                && span.paint_range.start == index && span.paint_range.end <= range.end
                && retain_order && !self.retained_prefix_invalid
                && self.replay_quad_span(prev_scene, span)
            {
                index = span.paint_range.end;
                spans.next();
                continue;
            }
            let operation = &prev_scene.paint_operations[index];
            index += 1;
            match operation {
                PaintOperation::Primitive(primitive) => {
                    if self.blur_captures.last().is_some() {
                        let (captured_primitive, capture_prefix_invalid) = {
                            let capture = self.blur_captures.last_mut().unwrap();
                            let operation_start = capture.scene.paint_operations.len();
                            capture.scene.replay_primitive(primitive, retain_order);
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
                            (captured_primitive, capture.scene.retained_prefix_invalid)
                        };
                        if capture_prefix_invalid {
                            self.retained_prefix_invalid = true;
                        }
                        if let Some(primitive) = captured_primitive {
                            self.paint_operations
                                .push(PaintOperation::Primitive(primitive));
                        }
                    } else {
                        self.replay_primitive(primitive, retain_order);
                    }
                }
                PaintOperation::StartLayer(bounds) => {
                    if let Some(capture) = self.blur_captures.last_mut() {
                        capture.scene.push_replayed_layer(*bounds);
                        self.paint_operations
                            .push(PaintOperation::StartLayer(*bounds));
                    } else {
                        self.push_replayed_layer(*bounds);
                    }
                }
                PaintOperation::EndLayer => {
                    if let Some(capture) = self.blur_captures.last_mut() {
                        capture.scene.pop_replayed_layer();
                        self.paint_operations.push(PaintOperation::EndLayer);
                    } else {
                        self.pop_replayed_layer();
                    }
                }
                PaintOperation::StartBlur(config) => self.begin_blur(config.clone()),
                PaintOperation::EndBlur => self.end_blur(),
            }
        }
        if retain_order && !self.retained_prefix_invalid && self.paint_operations.len() == range_end
        {
            self.retained_prefix_verified_len = range_end;
            true
        } else {
            if retain_order { self.retained_prefix_invalid = true; }
            false
        }
    }

    fn replay_quad_span(&mut self, previous: &Scene, span: &super::QuadReplay) -> bool {
        if !self.blur_captures.is_empty() || span.paint_range.is_empty()
            || span.paint_range.len() != span.quad_range.len()
        { return false; }
        let Some(operations) = previous.paint_operations.get(span.paint_range.clone()) else { return false; };
        if !operations.iter().all(|operation| matches!(operation,
            PaintOperation::Primitive(Primitive::Quad(quad))
                if quad.animation_id.is_none() && self.layer_stack.last().is_none_or(|order| *order == quad.order)
        )) { return false; }
        for operation in operations {
            let PaintOperation::Primitive(primitive) = operation else { unreachable!() };
            if self.layer_stack.is_empty() {
                self.primitive_bounds.insert_with_order(Self::ordering_bounds(primitive), primitive.order());
            }
            self.paint_operations.push(PaintOperation::Primitive(primitive.clone()));
        }
        self.quads.share(&previous.quads, span.quad_range.clone());
        self.replayed_primitives += operations.len();
        true
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
        debug_assert!(
            self.blur_captures.is_empty(),
            "element blur capture must be closed before finishing a scene"
        );
        self.shadows.sort_unstable_by_key(|shadow| shadow.order);
        self.quads.finish();
        self.paths.sort_unstable_by_key(|path| path.order);
        self.underlines
            .sort_unstable_by_key(|underline| underline.order);
        self.monochrome_sprites
            .sort_unstable_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.polychrome_sprites
            .sort_unstable_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.surfaces.sort_unstable_by_key(|surface| surface.order);
        self.backdrop_blurs.sort_unstable_by_key(|blur| blur.order);
        self.blurs.sort_unstable_by_key(|blur| blur.order);
        self.renderer_extensions
            .sort_unstable_by_key(|extension| extension.order);
        self.prepare_batches();
        static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);
        self.revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
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

    fn primitive_count(&self) -> usize {
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
            + self.blurs.capacity()
            + self.renderer_extensions.capacity()
            + self.animation_values.capacity()
            + self.prepared_batches.batches.capacity()
            + self.primitive_bounds.retained_capacity()
            + self.layer_stack.capacity()
            + self.blur_captures.capacity()
    }

    pub(crate) fn trim_for_reuse_against(&mut self, current: &Self) {
        macro_rules! trim_vec_against {
            ($field:ident) => {{
                let target = SCENE_MIN_RETAINED_CAPACITY.max(current.$field.len());
                if self.$field.capacity()
                    > target.saturating_mul(SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER)
                {
                    self.$field.shrink_to(target);
                }
            }};
        }

        trim_vec_against!(paint_operations);
        trim_vec_against!(layer_stack);
        trim_vec_against!(shadows);
        trim_vec_against!(quads);
        trim_vec_against!(paths);
        trim_vec_against!(underlines);
        trim_vec_against!(monochrome_sprites);
        trim_vec_against!(polychrome_sprites);
        trim_vec_against!(surfaces);
        trim_vec_against!(backdrop_blurs);
        trim_vec_against!(blurs);
        trim_vec_against!(renderer_extensions);
        trim_vec_against!(animation_values);
        trim_vec_against!(blur_captures);

        let batch_target = SCENE_MIN_RETAINED_CAPACITY.max(current.prepared_batches.batches.len());
        if self.prepared_batches.batches.capacity()
            > batch_target.saturating_mul(SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.prepared_batches.batches.shrink_to(batch_target);
            self.prepared_batches.retained_capacity = self.prepared_batches.batches.capacity();
        }

        self.primitive_bounds.trim_for_reuse_against(
            &current.primitive_bounds,
            SCENE_MIN_RETAINED_CAPACITY,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );

        // The old frame just became scratch. Seed its high-water history from the frame that is
        // actually visible now, not from a page that may have disappeared during this transition.
        self.recent_peak_paint_operations = current.paint_operations.len();
        self.recent_peak_primitives = current.primitive_count();
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
        if self.quads.capacity() > primitive_floor.saturating_mul(SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER) {
            self.quads.shrink_to(primitive_floor);
        }
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
            &mut self.blurs,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.renderer_extensions,
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
            if let PrimitiveBatch::Quads { quads, range } = batch {
                let mut append_run = |range: Range<usize>, is_solid| {
                    if let Some(PreparedSceneBatch::Quads(previous)) = prepared.last_mut()
                        && previous.is_solid == is_solid && previous.range.end == range.start
                    {
                        previous.range.end = range.end;
                    } else {
                        prepared.push(PreparedSceneBatch::Quads(PreparedQuadRun { range, is_solid }));
                    }
                };
                let mut run_start = range.start;
                let mut run_is_solid = quads.first().is_some_and(is_solid_quad);
                for (offset, quad) in quads.iter().enumerate().skip(1) {
                    let is_solid = is_solid_quad(quad);
                    if is_solid == run_is_solid {
                        continue;
                    }
                    let run_end = range.start + offset;
                    append_run(run_start..run_end, run_is_solid);
                    run_start = run_end;
                    run_is_solid = is_solid;
                }
                append_run(run_start..range.end, run_is_solid);
                continue;
            }

            prepared.push(match batch {
                PrimitiveBatch::Shadows(shadows) => {
                    PreparedSceneBatch::Shadows(slice_range(&self.shadows, shadows))
                }
                PrimitiveBatch::Quads { .. } => {
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
                PrimitiveBatch::Blurs(blurs) => {
                    PreparedSceneBatch::Blurs(slice_range(&self.blurs, blurs))
                }
                PrimitiveBatch::RendererExtensions(extensions) => {
                    PreparedSceneBatch::RendererExtensions(slice_range(
                        &self.renderer_extensions,
                        extensions,
                    ))
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
        (PaintOperation::EndLayer, PaintOperation::EndLayer)
        | (PaintOperation::EndBlur, PaintOperation::EndBlur) => true,
        (PaintOperation::StartBlur(current), PaintOperation::StartBlur(previous)) => {
            current == previous
        }
        _ => false,
    }
}
