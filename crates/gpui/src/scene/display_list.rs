use crate::{Bounds, ScaledPixels, SceneFrameMetrics};
use collections::FxHashSet;

use super::BoundsTree;
use std::ops::Range;

use super::geometry::{is_solid_quad, slice_range, trim_vec_capacity};
use super::{
    BatchIterator, BlurCapture, DrawOrder, MonochromeSprite, PaintBackdropBlur, PaintBlur,
    PaintGpuMesh3d, PaintOperation, PaintSurface, Path, PathId, PolychromeSprite,
    PreparedBackdropBlurGroup, PreparedGpuMesh3dPass, PreparedQuadRun, PreparedSceneBatch,
    PreparedSceneBatches, Primitive, PrimitiveBatch, Quad, SceneAnimationId, SceneAnimationValue,
    Shadow, Underline, blur_influence_radius,
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
    pub(crate) blurs: Vec<PaintBlur>,
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
        self.blurs.clear();
        self.gpu_meshes_3d.clear();
        self.animation_values.clear();
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
                PaintOperation::StartLayer(layer_bounds) => Some(*layer_bounds),
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
        for (group_range, group_bounds) in self.element_blur_groups() {
            if group_range.start < range.end && range.start < group_range.end {
                bounds = Some(match bounds {
                    Some(bounds) => bounds.union(&group_bounds),
                    None => group_bounds,
                });
            }
        }
        bounds
    }

    pub(crate) fn for_each_changed_bounds(
        &self,
        current_range: Range<usize>,
        previous: &Self,
        previous_range: Range<usize>,
        mut visit: impl FnMut(Bounds<ScaledPixels>),
    ) -> bool {
        let Some(current) = self.paint_operations.get(current_range.clone()) else {
            return false;
        };
        let Some(previous_operations) = previous.paint_operations.get(previous_range.clone())
        else {
            return false;
        };

        let prefix_len = current
            .iter()
            .zip(previous_operations)
            .take_while(|(current, previous)| current.visually_eq(previous))
            .count();
        let max_suffix_len = current
            .len()
            .min(previous_operations.len())
            .saturating_sub(prefix_len);
        let suffix_len = current
            .iter()
            .rev()
            .zip(previous_operations.iter().rev())
            .take(max_suffix_len)
            .take_while(|(current, previous)| current.visually_eq(previous))
            .count();

        for operation in
            &previous_operations[prefix_len..previous_operations.len().saturating_sub(suffix_len)]
        {
            if let Some(bounds) = operation.visual_bounds() {
                visit(bounds);
            }
        }
        for operation in &current[prefix_len..current.len().saturating_sub(suffix_len)] {
            if let Some(bounds) = operation.visual_bounds() {
                visit(bounds);
            }
        }

        let current_changed_range =
            current_range.start + prefix_len..current_range.end.saturating_sub(suffix_len);
        self.for_each_element_blur_damage(current_changed_range, &mut visit);

        let previous_changed_range =
            previous_range.start + prefix_len..previous_range.end.saturating_sub(suffix_len);
        previous.for_each_element_blur_damage(previous_changed_range, &mut visit);
        true
    }

    fn for_each_element_blur_damage(
        &self,
        changed_range: Range<usize>,
        visit: &mut impl FnMut(Bounds<ScaledPixels>),
    ) {
        if changed_range.start >= changed_range.end {
            return;
        }
        for (group_range, bounds) in self.element_blur_groups() {
            if group_range.start < changed_range.end && changed_range.start < group_range.end {
                visit(bounds);
            }
        }
    }

    fn element_blur_groups(&self) -> Vec<(Range<usize>, Bounds<ScaledPixels>)> {
        let mut groups = Vec::new();
        let mut stack = Vec::<(usize, BlurCapture, Bounds<ScaledPixels>)>::new();

        for (index, operation) in self.paint_operations.iter().enumerate() {
            match operation {
                PaintOperation::StartBlur(config) => {
                    stack.push((index, config.clone(), config.bounds));
                }
                PaintOperation::Primitive(primitive) => {
                    let primitive_bounds = primitive.visual_bounds();
                    for (_, _, bounds) in &mut stack {
                        *bounds = bounds.union(&primitive_bounds);
                    }
                }
                PaintOperation::StartLayer(bounds) => {
                    for (_, _, group_bounds) in &mut stack {
                        *group_bounds = group_bounds.union(bounds);
                    }
                }
                PaintOperation::EndBlur => {
                    let Some((start, config, bounds)) = stack.pop() else {
                        continue;
                    };
                    let bounds = bounds
                        .dilate(blur_influence_radius(config.radius))
                        .intersect(&config.content_mask.bounds);
                    for (_, _, parent_bounds) in &mut stack {
                        *parent_bounds = parent_bounds.union(&bounds);
                    }
                    groups.push((start..index + 1, bounds));
                }
                PaintOperation::EndLayer => {}
            }
        }
        groups
    }

    pub(crate) fn requires_full_redraw_fallback(&self) -> bool {
        !self.surfaces.is_empty()
            || !self.gpu_meshes_3d.is_empty()
            || self
                .blurs
                .iter()
                .any(|blur| blur.content.requires_full_redraw_fallback())
    }

    pub(crate) fn has_backdrop_blurs(&self) -> bool {
        !self.backdrop_blurs.is_empty()
            || self
                .blurs
                .iter()
                .any(|blur| blur.content.has_backdrop_blurs())
    }

    /// Returns whether any pixels sampled by any backdrop group changed.
    ///
    /// Backdrop filters are draw-order barriers: a background filter must not become dirty merely
    /// because a later tab animates, and a titlebar filter only cares about source pixels inside
    /// its own sampling footprint. The old implementation compared one global prefix before the
    /// first blur and forced every filter target to rebuild together.
    pub(crate) fn backdrop_blur_refresh_required(&self, previous: &Self) -> bool {
        if !self.has_backdrop_blurs() && !previous.has_backdrop_blurs() {
            return false;
        }

        let current_blurs = backdrop_blur_operations(&self.paint_operations);
        let previous_blurs = backdrop_blur_operations(&previous.paint_operations);
        if current_blurs.len() != previous_blurs.len() {
            return true;
        }

        for ((current_index, current_blur), (previous_index, previous_blur)) in
            current_blurs.into_iter().zip(previous_blurs)
        {
            if current_blur != previous_blur {
                return true;
            }
            let source_region = backdrop_blur_source_region(current_blur);
            if source_region.is_empty() {
                continue;
            }
            if paint_operations_changed_in_region(
                &self.paint_operations[..current_index],
                &previous.paint_operations[..previous_index],
                source_region,
            ) {
                return true;
            }
        }

        self.backdrop_blur_source_animation_values_changed(&previous.animation_values)
    }

    /// Checks GPU-side animation values that can change pixels sampled by a backdrop group.
    ///
    /// This is used by animation-only frames where the CPU scene itself is retained. It applies
    /// the same draw-order and spatial isolation as [`Self::backdrop_blur_refresh_required`].
    pub(crate) fn backdrop_blur_source_animation_values_changed(
        &self,
        next_values: &[SceneAnimationValue],
    ) -> bool {
        if !self.has_backdrop_blurs() {
            return false;
        }

        for (blur_index, blur) in backdrop_blur_operations(&self.paint_operations) {
            if animation_value_changed(self, blur.animation_id, next_values) {
                return true;
            }
            let source_region = backdrop_blur_source_region(blur);
            if source_region.is_empty() {
                continue;
            }
            for operation in &self.paint_operations[..blur_index] {
                let PaintOperation::Primitive(primitive) = operation else {
                    continue;
                };
                if !primitive.visual_bounds().intersects(&source_region) {
                    continue;
                }
                if animation_value_changed(self, primitive.animation_id(), next_values) {
                    return true;
                }
            }
        }
        false
    }

    fn animation_value(&self, animation_id: SceneAnimationId) -> Option<&SceneAnimationValue> {
        self.animation_values
            .iter()
            .find(|value| value.animation_id == animation_id)
    }

    pub(crate) fn backdrop_blur_damage(
        &self,
        damage: Bounds<ScaledPixels>,
    ) -> impl Iterator<Item = Bounds<ScaledPixels>> + '_ {
        let mut damage_regions = Vec::new();
        self.collect_backdrop_blur_damage(damage, &mut damage_regions);
        damage_regions.into_iter()
    }

    fn collect_backdrop_blur_damage(
        &self,
        damage: Bounds<ScaledPixels>,
        damage_regions: &mut Vec<Bounds<ScaledPixels>>,
    ) {
        for blur in &self.backdrop_blurs {
            // Each blur carries its own kernel support. A 0.1px background filter must never inherit
            // the 18px titlebar's damage expansion merely because both exist in the same scene.
            let influence_radius = backdrop_blur_influence_radius(blur);
            let affected = damage
                .dilate(influence_radius)
                .intersect(&blur.bounds)
                .intersect(&blur.content_mask.bounds);
            if !affected.is_empty() {
                damage_regions.push(affected);
            }
        }
        for blur in &self.blurs {
            blur.content
                .collect_backdrop_blur_damage(damage, damage_regions);
        }
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
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
        self.paint_operations
            .push(PaintOperation::StartBlur(config.clone()));
        self.blur_captures.push(BlurCaptureState {
            config,
            scene: Box::default(),
        });
    }

    pub(crate) fn end_blur(&mut self) {
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

    pub(crate) fn animation_ids(&self) -> FxHashSet<SceneAnimationId> {
        self.paint_operations
            .iter()
            .filter_map(|operation| match operation {
                PaintOperation::Primitive(primitive) => primitive.animation_id(),
                PaintOperation::StartLayer(_)
                | PaintOperation::EndLayer
                | PaintOperation::StartBlur(_)
                | PaintOperation::EndBlur => None,
            })
            .collect()
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

    fn push_ordered_primitive(
        &mut self,
        mut primitive: Primitive,
        order: DrawOrder,
        record_operation: bool,
    ) {
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
            Primitive::Blur(blur) => {
                blur.order = order;
                self.blurs.push(blur.clone());
            }
            Primitive::GpuMesh3d(mesh) => {
                mesh.order = order;
                self.gpu_meshes_3d.push(mesh.clone());
            }
        }
        if record_operation {
            self.paint_operations
                .push(PaintOperation::Primitive(primitive));
        }
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
            Primitive::Blur(blur) => {
                let mut blur = blur.clone();
                blur.order = order;
                self.blurs.push(blur);
                ScenePrimitiveKind::Blur
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
                    if let Some(capture) = self.blur_captures.last_mut() {
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
        debug_assert!(
            self.blur_captures.is_empty(),
            "element blur capture must be closed before finishing a scene"
        );
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
        self.blurs.sort_unstable_by_key(|blur| blur.order);
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
            blurs: &self.blurs,
            blurs_start: 0,
            blurs_iter: self.blurs.iter().peekable(),
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
            + self.blurs.len()
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
            + self.blurs.capacity()
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
            &mut self.blurs,
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
                PrimitiveBatch::Blurs(blurs) => {
                    PreparedSceneBatch::Blurs(slice_range(&self.blurs, blurs))
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

fn backdrop_blur_operations(operations: &[PaintOperation]) -> Vec<(usize, &PaintBackdropBlur)> {
    operations
        .iter()
        .enumerate()
        .filter_map(|(index, operation)| match operation {
            PaintOperation::Primitive(Primitive::BackdropBlur(blur)) => Some((index, blur)),
            PaintOperation::Primitive(_)
            | PaintOperation::StartLayer(_)
            | PaintOperation::EndLayer
            | PaintOperation::StartBlur(_)
            | PaintOperation::EndBlur => None,
        })
        .collect()
}

fn backdrop_blur_source_region(blur: &PaintBackdropBlur) -> Bounds<ScaledPixels> {
    blur.bounds
        .intersect(&blur.content_mask.bounds)
        .dilate(backdrop_blur_influence_radius(blur))
}

fn paint_operations_changed_in_region(
    current: &[PaintOperation],
    previous: &[PaintOperation],
    region: Bounds<ScaledPixels>,
) -> bool {
    let prefix_len = current
        .iter()
        .zip(previous)
        .take_while(|(current, previous)| current.visually_eq(previous))
        .count();
    let max_suffix_len = current.len().min(previous.len()).saturating_sub(prefix_len);
    let suffix_len = current
        .iter()
        .rev()
        .zip(previous.iter().rev())
        .take(max_suffix_len)
        .take_while(|(current, previous)| current.visually_eq(previous))
        .count();

    current[prefix_len..current.len().saturating_sub(suffix_len)]
        .iter()
        .chain(&previous[prefix_len..previous.len().saturating_sub(suffix_len)])
        .filter_map(PaintOperation::visual_bounds)
        .any(|bounds| bounds.intersects(&region))
}

fn animation_value_changed(
    scene: &Scene,
    animation_id: Option<SceneAnimationId>,
    next_values: &[SceneAnimationValue],
) -> bool {
    let Some(animation_id) = animation_id else {
        return false;
    };
    scene.animation_value(animation_id)
        != next_values
            .iter()
            .find(|value| value.animation_id == animation_id)
}

fn backdrop_blur_influence_radius(blur: &PaintBackdropBlur) -> ScaledPixels {
    let radius = blur.radius.0.abs();
    if !radius.is_finite() || radius <= 0.0 {
        return ScaledPixels(0.0);
    }

    // CSS blur radius is the Gaussian standard deviation. Three sigma contains the practical
    // filter support; add half of the source texel footprint for linear filtering and downsampling.
    let linear_footprint = 0.5 * f32::from(blur.downsample.max(1));
    ScaledPixels(radius * 3.0 + linear_footprint)
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
