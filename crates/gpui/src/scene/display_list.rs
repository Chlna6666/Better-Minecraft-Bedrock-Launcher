use crate::{Bounds, ScaledPixels, SceneFrameMetrics, TransitionProperty};
use collections::FxHashSet;

use super::BoundsTree;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

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
    /// Identity of the last completed static display list. Animation values are independent.
    pub(crate) revision: u64,
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
const ENGINE_ANIMATION_ID_START: u32 = 1 << 31;

#[derive(Clone, Debug, Default)]
pub(crate) struct BackdropBlurDamagePlan {
    entries: Vec<BackdropBlurSourceDamage>,
}

#[derive(Clone, Debug)]
struct BackdropBlurSourceDamage {
    order: DrawOrder,
    source_damage: Vec<Bounds<ScaledPixels>>,
    full_refresh: bool,
}

impl BackdropBlurDamagePlan {
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn refresh_required(&self) -> bool {
        !self.is_empty()
    }

    pub(crate) fn source_damage_for_orders(
        &self,
        first: DrawOrder,
        last: DrawOrder,
    ) -> (bool, impl Iterator<Item = Bounds<ScaledPixels>> + '_) {
        let full_refresh = self
            .entries
            .iter()
            .any(|entry| entry.order >= first && entry.order <= last && entry.full_refresh);
        let damage = self
            .entries
            .iter()
            .filter(move |entry| entry.order >= first && entry.order <= last)
            .flat_map(|entry| entry.source_damage.iter().copied());
        (full_refresh, damage)
    }

    fn mark_full(&mut self, order: DrawOrder) {
        let entry = self.entry_mut(order);
        entry.full_refresh = true;
        entry.source_damage.clear();
    }

    fn push(&mut self, order: DrawOrder, damage: Bounds<ScaledPixels>) {
        if damage.is_empty() {
            return;
        }
        let entry = self.entry_mut(order);
        if !entry.full_refresh {
            entry.source_damage.push(damage);
        }
    }

    fn entry_mut(&mut self, order: DrawOrder) -> &mut BackdropBlurSourceDamage {
        if let Some(index) = self.entries.iter().position(|entry| entry.order == order) {
            return &mut self.entries[index];
        }
        let index = self.entries.len();
        self.entries.push(BackdropBlurSourceDamage {
            order,
            source_damage: Vec::new(),
            full_refresh: false,
        });
        &mut self.entries[index]
    }
}

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
        self.gpu_meshes_3d.clear();
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
            .take_while(|(current, previous_operation)| {
                paint_operations_match_for_damage(self, current, previous, previous_operation)
            })
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
            .take_while(|(current, previous_operation)| {
                paint_operations_match_for_damage(self, current, previous, previous_operation)
            })
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

    /// Computes spatial source damage independently for every backdrop draw-order barrier.
    pub(crate) fn backdrop_blur_damage_plan(&self, previous: &Self) -> BackdropBlurDamagePlan {
        let mut plan = BackdropBlurDamagePlan::default();
        if !self.has_backdrop_blurs() && !previous.has_backdrop_blurs() {
            return plan;
        }

        let current_blurs = backdrop_blur_operations(&self.paint_operations);
        let previous_blurs = backdrop_blur_operations(&previous.paint_operations);
        if current_blurs.len() != previous_blurs.len() {
            for (_, blur) in current_blurs {
                plan.mark_full(blur.order);
            }
            return plan;
        }

        for ((current_index, current_blur), (previous_index, previous_blur)) in
            current_blurs.into_iter().zip(previous_blurs)
        {
            if current_blur != previous_blur {
                plan.mark_full(current_blur.order);
                continue;
            }
            let source_region = backdrop_blur_source_region(current_blur);
            if source_region.is_empty() {
                continue;
            }
            collect_paint_operation_damage(
                &self.paint_operations[..current_index],
                &previous.paint_operations[..previous_index],
                source_region,
                |damage| plan.push(current_blur.order, damage),
            );
        }

        self.collect_backdrop_blur_animation_damage(&previous.animation_values, &mut plan);
        plan
    }

    /// Computes per-backdrop source damage for an animation-only retained-scene frame.
    pub(crate) fn backdrop_blur_animation_damage_plan(
        &self,
        next_values: &[SceneAnimationValue],
    ) -> BackdropBlurDamagePlan {
        let mut plan = BackdropBlurDamagePlan::default();
        self.collect_backdrop_blur_animation_damage(next_values, &mut plan);
        plan
    }

    fn collect_backdrop_blur_animation_damage(
        &self,
        next_values: &[SceneAnimationValue],
        plan: &mut BackdropBlurDamagePlan,
    ) {
        if !self.has_backdrop_blurs() {
            return;
        }

        for (blur_index, blur) in backdrop_blur_operations(&self.paint_operations) {
            if animation_value_changed(self, blur.animation_id, next_values) {
                plan.mark_full(blur.order);
                continue;
            }
            let source_region = backdrop_blur_source_region(blur);
            if source_region.is_empty() {
                continue;
            }
            for operation in &self.paint_operations[..blur_index] {
                let damage = match operation {
                    PaintOperation::Primitive(primitive)
                        if animation_value_changed(self, primitive.animation_id(), next_values) =>
                    {
                        Some(animation_swept_bounds(self, primitive, next_values))
                    }
                    PaintOperation::StartBlur(capture)
                        if animation_value_changed(self, capture.animation_id, next_values) =>
                    {
                        Some(blur_capture_animation_swept_bounds(
                            self,
                            capture,
                            next_values,
                        ))
                    }
                    PaintOperation::Primitive(_)
                    | PaintOperation::StartLayer(_)
                    | PaintOperation::EndLayer
                    | PaintOperation::StartBlur(_)
                    | PaintOperation::EndBlur => None,
                };
                if let Some(damage) = damage
                    && damage.intersects(&source_region)
                {
                    plan.push(blur.order, damage.intersect(&source_region));
                }
            }
        }
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

    pub(crate) fn backdrop_blur_output_damage(
        &self,
        plan: &BackdropBlurDamagePlan,
    ) -> impl Iterator<Item = Bounds<ScaledPixels>> + '_ {
        let mut output_damage = Vec::new();
        for (_, blur) in backdrop_blur_operations(&self.paint_operations) {
            let (full_refresh, source_damage) =
                plan.source_damage_for_orders(blur.order, blur.order);
            if full_refresh {
                let bounds = blur.bounds.intersect(&blur.content_mask.bounds);
                if !bounds.is_empty() {
                    output_damage.push(bounds);
                }
                continue;
            }
            let influence_radius = backdrop_blur_influence_radius(blur);
            for damage in source_damage {
                let affected = damage
                    .dilate(influence_radius)
                    .intersect(&blur.bounds)
                    .intersect(&blur.content_mask.bounds);
                if !affected.is_empty() {
                    output_damage.push(affected);
                }
            }
        }
        output_damage.into_iter()
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

    pub(crate) fn replace_engine_animation_values(
        &mut self,
        values: impl IntoIterator<Item = SceneAnimationValue>,
    ) {
        self.animation_values
            .retain(|value| value.animation_id.0 < ENGINE_ANIMATION_ID_START);
        self.animation_values.extend(values);
    }

    pub(crate) fn animation_ids(&self) -> FxHashSet<SceneAnimationId> {
        self.paint_operations
            .iter()
            .filter_map(|operation| match operation {
                PaintOperation::Primitive(primitive) => primitive.animation_id(),
                PaintOperation::StartBlur(blur) => blur.animation_id,
                PaintOperation::StartLayer(_)
                | PaintOperation::EndLayer
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
        self.revision = 0;
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
        self.finish_with_previous(None);
    }

    pub(crate) fn finish_retaining_revision(&mut self, previous: &Scene) {
        self.finish_with_previous(Some(previous));
    }

    fn finish_with_previous(&mut self, previous: Option<&Scene>) {
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
        if let Some(previous) = previous
            && self.paint_operations.len() == previous.paint_operations.len()
            && self
                .paint_operations
                .iter()
                .zip(&previous.paint_operations)
                .all(|(current, previous)| paint_operations_visually_match(current, previous))
        {
            self.revision = previous.revision;
            return;
        }
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

fn collect_paint_operation_damage(
    current: &[PaintOperation],
    previous: &[PaintOperation],
    region: Bounds<ScaledPixels>,
    mut visit: impl FnMut(Bounds<ScaledPixels>),
) {
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

    let current_changed = &current[prefix_len..current.len().saturating_sub(suffix_len)];
    let previous_changed = &previous[prefix_len..previous.len().saturating_sub(suffix_len)];

    if current_changed.len() == previous_changed.len() {
        for (current_operation, previous_operation) in
            current_changed.iter().zip(previous_changed)
        {
            if visit_opaque_solid_quad_move_damage(
                current_operation,
                previous_operation,
                region,
                &mut visit,
            ) {
                continue;
            }
            visit_operation_damage(current_operation, region, &mut visit);
            visit_operation_damage(previous_operation, region, &mut visit);
        }
        return;
    }

    current_changed
        .iter()
        .chain(previous_changed)
        .for_each(|operation| visit_operation_damage(operation, region, &mut visit));
}

fn visit_operation_damage(
    operation: &PaintOperation,
    region: Bounds<ScaledPixels>,
    visit: &mut impl FnMut(Bounds<ScaledPixels>),
) {
    let Some(bounds) = operation.visual_bounds() else {
        return;
    };
    let damage = bounds.intersect(&region);
    if !damage.is_empty() {
        visit(damage);
    }
}

/// A moved opaque rectangular fill only changes pixels in the symmetric difference of its old and
/// new rectangles. The overlap is byte-for-byte identical and does not need to invalidate a
/// backdrop source. Keep the predicate deliberately strict; any uncertainty falls back to the
/// ordinary old+new bounds damage path.
fn visit_opaque_solid_quad_move_damage(
    current: &PaintOperation,
    previous: &PaintOperation,
    region: Bounds<ScaledPixels>,
    visit: &mut impl FnMut(Bounds<ScaledPixels>),
) -> bool {
    let (
        PaintOperation::Primitive(Primitive::Quad(current)),
        PaintOperation::Primitive(Primitive::Quad(previous)),
    ) = (current, previous)
    else {
        return false;
    };

    if !is_solid_quad(current)
        || !is_solid_quad(previous)
        || current.order != previous.order
        || current.animation_id != previous.animation_id
        || current.background != previous.background
        || current.background.solid.a < 1.0
        || current.border_style != previous.border_style
        || current.border_color != previous.border_color
        || current.corner_radii != previous.corner_radii
        || current.border_widths != previous.border_widths
        || current.content_mask != previous.content_mask
    {
        return false;
    }

    let current_bounds = current.bounds.intersect(&current.content_mask.bounds);
    let previous_bounds = previous.bounds.intersect(&previous.content_mask.bounds);
    if current_bounds.is_empty() || previous_bounds.is_empty() {
        return false;
    }

    let overlap = current_bounds.intersect(&previous_bounds);
    if overlap.is_empty() {
        visit_clipped_damage(current_bounds, region, visit);
        visit_clipped_damage(previous_bounds, region, visit);
        return true;
    }

    visit_rect_difference(current_bounds, overlap, region, visit);
    visit_rect_difference(previous_bounds, overlap, region, visit);
    true
}

fn visit_rect_difference(
    bounds: Bounds<ScaledPixels>,
    overlap: Bounds<ScaledPixels>,
    region: Bounds<ScaledPixels>,
    visit: &mut impl FnMut(Bounds<ScaledPixels>),
) {
    let top = Bounds::new(
        crate::point(bounds.left(), bounds.top()),
        crate::size(bounds.size.width, overlap.top() - bounds.top()),
    );
    let bottom = Bounds::new(
        crate::point(bounds.left(), overlap.bottom()),
        crate::size(bounds.size.width, bounds.bottom() - overlap.bottom()),
    );
    let left = Bounds::new(
        crate::point(bounds.left(), overlap.top()),
        crate::size(overlap.left() - bounds.left(), overlap.size.height),
    );
    let right = Bounds::new(
        crate::point(overlap.right(), overlap.top()),
        crate::size(bounds.right() - overlap.right(), overlap.size.height),
    );

    for damage in [top, bottom, left, right] {
        visit_clipped_damage(damage, region, visit);
    }
}

fn visit_clipped_damage(
    bounds: Bounds<ScaledPixels>,
    region: Bounds<ScaledPixels>,
    visit: &mut impl FnMut(Bounds<ScaledPixels>),
) {
    if bounds.is_empty() {
        return;
    }
    let damage = bounds.intersect(&region);
    if !damage.is_empty() {
        visit(damage);
    }
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

fn animation_swept_bounds(
    scene: &Scene,
    primitive: &Primitive,
    next_values: &[SceneAnimationValue],
) -> Bounds<ScaledPixels> {
    let Some(animation_id) = primitive.animation_id() else {
        return primitive.visual_bounds();
    };
    let previous = scene.animation_value(animation_id);
    let next = next_values
        .iter()
        .find(|value| value.animation_id == animation_id);
    animation_sampled_bounds(primitive, previous).union(&animation_sampled_bounds(primitive, next))
}

fn blur_capture_animation_swept_bounds(
    scene: &Scene,
    blur: &BlurCapture,
    next_values: &[SceneAnimationValue],
) -> Bounds<ScaledPixels> {
    let Some(animation_id) = blur.animation_id else {
        return blur_capture_visual_bounds(blur);
    };
    let previous = scene.animation_value(animation_id);
    let next = next_values
        .iter()
        .find(|value| value.animation_id == animation_id);
    animation_sampled_blur_capture_bounds(blur, previous)
        .union(&animation_sampled_blur_capture_bounds(blur, next))
}

fn blur_capture_visual_bounds(blur: &BlurCapture) -> Bounds<ScaledPixels> {
    blur.bounds
        .dilate(blur_influence_radius(blur.radius))
        .intersect(&blur.content_mask.bounds)
}

fn animation_sampled_blur_capture_bounds(
    blur: &BlurCapture,
    value: Option<&SceneAnimationValue>,
) -> Bounds<ScaledPixels> {
    let bounds = blur_capture_visual_bounds(blur);
    let Some(value) = value else {
        return bounds;
    };
    let sampled = sampled_animation_components(value);
    match value.property {
        TransitionProperty::Translation => Bounds {
            origin: bounds.origin
                + crate::point(ScaledPixels(sampled[0]), ScaledPixels(sampled[1])),
            size: bounds.size,
        },
        TransitionProperty::Scale => scaled_animation_bounds(bounds, sampled[0], bounds.center()),
        TransitionProperty::Transform => scaled_animation_bounds(
            bounds,
            sampled[0],
            crate::point(ScaledPixels(sampled[2]), ScaledPixels(sampled[3])),
        ),
        // Opacity changes pixels but not geometry, so the whole composite output is source damage
        // for a later backdrop barrier.
        TransitionProperty::Opacity => bounds,
        _ => bounds,
    }
}

fn animation_sampled_bounds(
    primitive: &Primitive,
    value: Option<&SceneAnimationValue>,
) -> Bounds<ScaledPixels> {
    let Some(value) = value else {
        return primitive.visual_bounds();
    };
    let sampled = sampled_animation_components(value);
    let bounds = primitive.visual_bounds();
    match value.property {
        TransitionProperty::Translation
            if matches!(
                primitive,
                Primitive::Quad(_)
                    | Primitive::Shadow(_)
                    | Primitive::MonochromeSprite(_)
                    | Primitive::PolychromeSprite(_)
                    | Primitive::BackdropBlur(_)
                    | Primitive::Blur(_)
            ) =>
        {
            Bounds {
                origin: bounds.origin
                    + crate::point(ScaledPixels(sampled[0]), ScaledPixels(sampled[1])),
                size: bounds.size,
            }
        }
        TransitionProperty::Scale => scaled_animation_bounds(bounds, sampled[0], bounds.center()),
        TransitionProperty::Transform => scaled_animation_bounds(
            bounds,
            sampled[0],
            crate::point(ScaledPixels(sampled[2]), ScaledPixels(sampled[3])),
        ),
        TransitionProperty::Rotation => rotated_animation_bounds(primitive, sampled[0]),
        _ => bounds,
    }
}

fn sampled_animation_components(value: &SceneAnimationValue) -> [f32; 4] {
    let progress = if value.progress.is_finite() {
        value.progress
    } else {
        0.0
    };
    std::array::from_fn(|index| {
        value.from[index] + (value.to[index] - value.from[index]) * progress
    })
}

fn scaled_animation_bounds(
    bounds: Bounds<ScaledPixels>,
    scale: f32,
    origin: crate::Point<ScaledPixels>,
) -> Bounds<ScaledPixels> {
    let scale = if scale.is_finite() {
        scale.max(0.0)
    } else {
        1.0
    };
    Bounds {
        origin: origin + (bounds.origin - origin) * scale,
        size: bounds.size.map(|value| value * scale),
    }
}

fn rotated_animation_bounds(primitive: &Primitive, angle: f32) -> Bounds<ScaledPixels> {
    let Primitive::MonochromeSprite(sprite) = primitive else {
        return primitive.visual_bounds();
    };
    if !angle.is_finite() {
        return primitive.visual_bounds();
    }
    let center = sprite.bounds.center();
    let rotation = super::TransformationMatrix::unit()
        .translate(center)
        .rotate(crate::radians(angle))
        .translate(crate::point(
            ScaledPixels(-center.x.0),
            ScaledPixels(-center.y.0),
        ));
    let transform = sprite.transformation.compose(rotation);
    let left = sprite.bounds.left().0;
    let right = sprite.bounds.right().0;
    let top = sprite.bounds.top().0;
    let bottom = sprite.bounds.bottom().0;
    let corners = [[left, top], [right, top], [left, bottom], [right, bottom]];
    let transformed = corners.map(|[x, y]| {
        crate::point(
            ScaledPixels(
                transform.translation[0]
                    + transform.rotation_scale[0][0] * x
                    + transform.rotation_scale[0][1] * y,
            ),
            ScaledPixels(
                transform.translation[1]
                    + transform.rotation_scale[1][0] * x
                    + transform.rotation_scale[1][1] * y,
            ),
        )
    });
    let min_x = transformed
        .iter()
        .map(|point| point.x)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .unwrap_or_default();
    let max_x = transformed
        .iter()
        .map(|point| point.x)
        .max_by(|left, right| left.0.total_cmp(&right.0))
        .unwrap_or_default();
    let min_y = transformed
        .iter()
        .map(|point| point.y)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .unwrap_or_default();
    let max_y = transformed
        .iter()
        .map(|point| point.y)
        .max_by(|left, right| left.0.total_cmp(&right.0))
        .unwrap_or_default();
    Bounds::new(
        crate::point(min_x, min_y),
        crate::size(max_x - min_x, max_y - min_y),
    )
    .dilate(ScaledPixels(1.0))
    .intersect(&sprite.content_mask.bounds)
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

fn paint_operations_visually_match(current: &PaintOperation, previous: &PaintOperation) -> bool {
    match (current, previous) {
        (PaintOperation::Primitive(current), PaintOperation::Primitive(previous)) => {
            current.visually_eq(previous)
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

fn paint_operations_match_for_damage(
    current_scene: &Scene,
    current: &PaintOperation,
    previous_scene: &Scene,
    previous: &PaintOperation,
) -> bool {
    if !current.visually_eq(previous) {
        return false;
    }

    match (current, previous) {
        (
            PaintOperation::Primitive(current_primitive),
            PaintOperation::Primitive(previous_primitive),
        ) => {
            let Some(current_animation_id) = current_primitive.animation_id() else {
                return true;
            };
            let Some(previous_animation_id) = previous_primitive.animation_id() else {
                return false;
            };
            current_scene.animation_value(current_animation_id)
                == previous_scene.animation_value(previous_animation_id)
        }
        (PaintOperation::StartBlur(current_blur), PaintOperation::StartBlur(previous_blur)) => {
            match (current_blur.animation_id, previous_blur.animation_id) {
                (Some(current_animation_id), Some(previous_animation_id)) => {
                    current_scene.animation_value(current_animation_id)
                        == previous_scene.animation_value(previous_animation_id)
                }
                (None, None) => true,
                _ => false,
            }
        }
        _ => true,
    }
}
