use super::*;
use std::cell::RefCell;

/// Automatic renderer-owned invalidation state for root backdrop filters.
///
/// Applications only declare backdrop filters. GPUI records which draw-order barriers actually
/// need a new filtered result and the renderer keeps every other target cached.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BackdropBlurRefresh {
    force_all: bool,
    dirty_orders: Vec<DrawOrder>,
}

impl BackdropBlurRefresh {
    pub(crate) fn all() -> Self {
        Self {
            force_all: true,
            dirty_orders: Vec::new(),
        }
    }

    pub(crate) fn force_all(&self) -> bool {
        self.force_all
    }

    pub(crate) fn requires_refresh(&self) -> bool {
        self.force_all || !self.dirty_orders.is_empty()
    }

    pub(crate) fn dirty_orders(&self) -> &[DrawOrder] {
        &self.dirty_orders
    }

    pub(crate) fn contains_order(&self, order: DrawOrder) -> bool {
        self.force_all || self.dirty_orders.contains(&order)
    }

    fn mark(&mut self, order: DrawOrder) {
        if !self.force_all && !self.dirty_orders.contains(&order) {
            self.dirty_orders.push(order);
        }
    }

    fn merge(&mut self, other: Self) {
        if self.force_all {
            return;
        }
        if other.force_all {
            *self = Self::all();
            return;
        }
        for order in other.dirty_orders {
            self.mark(order);
        }
    }
}

thread_local! {
    /// Scene refresh metadata is rendering scratch, not application state. Keeping it outside the
    /// retained display list avoids making every Scene clone/store carry a cache object while still
    /// letting the Window invalidation pass hand an exact plan to Nova later in the same UI thread.
    static BACKDROP_REFRESH: RefCell<std::collections::HashMap<usize, BackdropBlurRefresh>> =
        RefCell::new(std::collections::HashMap::new());
}

fn scene_key(scene: &Scene) -> usize {
    std::ptr::from_ref(scene) as usize
}

impl Scene {
    pub(crate) fn backdrop_blur_refresh_state(&self) -> BackdropBlurRefresh {
        BACKDROP_REFRESH.with(|states| {
            states
                .borrow()
                .get(&scene_key(self))
                .cloned()
                .unwrap_or_default()
        })
    }

    pub(super) fn store_backdrop_blur_refresh(&self, refresh: BackdropBlurRefresh) -> bool {
        let required = refresh.requires_refresh();
        BACKDROP_REFRESH.with(|states| {
            let mut states = states.borrow_mut();
            if refresh.requires_refresh() {
                states.insert(scene_key(self), refresh);
            } else {
                states.remove(&scene_key(self));
            }
        });
        required
    }

    pub(super) fn clear_backdrop_blur_refresh_state(&self) {
        BACKDROP_REFRESH.with(|states| {
            states.borrow_mut().remove(&scene_key(self));
        });
    }

    pub(super) fn compute_backdrop_blur_refresh(&self, previous: &Self) -> BackdropBlurRefresh {
        if !self.has_backdrop_blurs() && !previous.has_backdrop_blurs() {
            return BackdropBlurRefresh::default();
        }

        let current_blurs = root_backdrop_blur_operations(&self.paint_operations);
        let previous_blurs = root_backdrop_blur_operations(&previous.paint_operations);
        if current_blurs.len() != previous_blurs.len() {
            return BackdropBlurRefresh::all();
        }

        let mut refresh = BackdropBlurRefresh::default();
        for ((current_index, current_blur), (previous_index, previous_blur)) in
            current_blurs.into_iter().zip(previous_blurs)
        {
            // A barrier moving in draw order changes the source prefix for every following root
            // filter, so this is the one structural case where selective reuse is unsafe.
            if current_blur.order != previous_blur.order {
                return BackdropBlurRefresh::all();
            }

            if backdrop_filter_input_changed(current_blur, previous_blur) {
                refresh.mark(current_blur.order);
                continue;
            }

            let source_region = root_backdrop_source_region(current_blur);
            if source_region.is_empty() {
                continue;
            }
            if paint_operations_changed_in_source_region(
                &self.paint_operations[..current_index],
                &previous.paint_operations[..previous_index],
                source_region,
            ) {
                refresh.mark(current_blur.order);
            }
        }

        refresh.merge(self.compute_backdrop_blur_animation_refresh(&previous.animation_values));
        refresh
    }

    pub(super) fn compute_backdrop_blur_animation_refresh(
        &self,
        next_values: &[SceneAnimationValue],
    ) -> BackdropBlurRefresh {
        if !self.has_backdrop_blurs() {
            return BackdropBlurRefresh::default();
        }

        let mut refresh = BackdropBlurRefresh::default();
        for (blur_index, blur) in root_backdrop_blur_operations(&self.paint_operations) {
            if animation_value_changed_for_scene(self, blur.animation_id, next_values) {
                refresh.mark(blur.order);
            }

            let source_region = root_backdrop_source_region(blur);
            if source_region.is_empty() {
                continue;
            }
            for operation in &self.paint_operations[..blur_index] {
                let PaintOperation::Primitive(primitive) = operation else {
                    continue;
                };
                if !animation_value_changed_for_scene(self, primitive.animation_id(), next_values) {
                    continue;
                }
                if primitive_animation_swept_bounds(self, primitive, next_values)
                    .intersects(&source_region)
                {
                    refresh.mark(blur.order);
                    break;
                }
            }
        }
        refresh
    }
}

/// Root backdrop filters only. Backdrop filters captured inside `filter: blur(...)` belong to that
/// isolated element layer and must never invalidate the root backdrop cache family.
fn root_backdrop_blur_operations(
    operations: &[PaintOperation],
) -> Vec<(usize, &PaintBackdropBlur)> {
    let mut depth = 0usize;
    let mut blurs = Vec::new();
    for (index, operation) in operations.iter().enumerate() {
        match operation {
            PaintOperation::StartBlur(_) => depth = depth.saturating_add(1),
            PaintOperation::EndBlur => depth = depth.saturating_sub(1),
            PaintOperation::Primitive(Primitive::BackdropBlur(blur)) if depth == 0 => {
                blurs.push((index, blur));
            }
            PaintOperation::Primitive(_)
            | PaintOperation::StartLayer(_)
            | PaintOperation::EndLayer => {}
        }
    }
    blurs
}

fn root_backdrop_source_region(blur: &PaintBackdropBlur) -> Bounds<ScaledPixels> {
    blur.bounds
        .intersect(&blur.content_mask.bounds)
        .dilate(root_backdrop_influence_radius(blur))
}

/// Only fields that change the cached Gaussian input/output belong here. Tint, saturation,
/// opacity and corner clipping are composite-only and can update in the main pass without touching
/// the cached filter target.
fn backdrop_filter_input_changed(current: &PaintBackdropBlur, previous: &PaintBackdropBlur) -> bool {
    current.bounds != previous.bounds
        || current.content_mask != previous.content_mask
        || current.radius != previous.radius
        || current.downsample != previous.downsample
        || current.levels != previous.levels
        || current.recompute_overlap != previous.recompute_overlap
}

fn paint_operations_changed_in_source_region(
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

fn animation_value_changed_for_scene(
    scene: &Scene,
    animation_id: Option<SceneAnimationId>,
    next_values: &[SceneAnimationValue],
) -> bool {
    let Some(animation_id) = animation_id else {
        return false;
    };
    scene
        .animation_values
        .iter()
        .find(|value| value.animation_id == animation_id)
        != next_values
            .iter()
            .find(|value| value.animation_id == animation_id)
}

fn primitive_animation_swept_bounds(
    scene: &Scene,
    primitive: &Primitive,
    next_values: &[SceneAnimationValue],
) -> Bounds<ScaledPixels> {
    let base = primitive.visual_bounds();
    let Some(animation_id) = primitive.animation_id() else {
        return base;
    };
    let current = scene
        .animation_values
        .iter()
        .find(|value| value.animation_id == animation_id);
    let next = next_values
        .iter()
        .find(|value| value.animation_id == animation_id);

    let mut swept = base;
    if let Some(value) = current {
        swept = swept.union(&animated_bounds_for_value(primitive, base, value));
    }
    if let Some(value) = next {
        swept = swept.union(&animated_bounds_for_value(primitive, base, value));
    }
    swept
}

fn animated_bounds_for_value(
    primitive: &Primitive,
    base: Bounds<ScaledPixels>,
    value: &SceneAnimationValue,
) -> Bounds<ScaledPixels> {
    let sampled = sample_animation(value);
    match value.property {
        crate::TransitionProperty::Translation => {
            let mut bounds = base;
            bounds.origin += point(ScaledPixels(sampled[0]), ScaledPixels(sampled[1]));
            bounds
        }
        crate::TransitionProperty::Scale => {
            scale_bounds_about(base, primitive.bounds().center(), sampled[0])
        }
        crate::TransitionProperty::Transform => scale_bounds_about(
            base,
            point(ScaledPixels(sampled[2]), ScaledPixels(sampled[3])),
            sampled[0],
        ),
        crate::TransitionProperty::Rotation => rotate_bounds_about_center(base, sampled[0]),
        _ => base,
    }
}

fn sample_animation(value: &SceneAnimationValue) -> [f32; 4] {
    let progress = if value.progress.is_finite() {
        value.progress
    } else {
        0.0
    };
    std::array::from_fn(|index| {
        value.from[index] + (value.to[index] - value.from[index]) * progress
    })
}

fn scale_bounds_about(
    bounds: Bounds<ScaledPixels>,
    origin: Point<ScaledPixels>,
    scale: f32,
) -> Bounds<ScaledPixels> {
    let scale = if scale.is_finite() { scale.max(0.0) } else { 1.0 };
    Bounds {
        origin: origin + (bounds.origin - origin) * scale,
        size: bounds.size.map(|value| value * scale),
    }
}

fn rotate_bounds_about_center(
    bounds: Bounds<ScaledPixels>,
    angle: f32,
) -> Bounds<ScaledPixels> {
    if !angle.is_finite() || angle == 0.0 {
        return bounds;
    }
    let center = bounds.center();
    let (sin, cos) = angle.sin_cos();
    let left = bounds.origin.x.0;
    let top = bounds.origin.y.0;
    let right = bounds.right().0;
    let bottom = bounds.bottom().0;
    let cx = center.x.0;
    let cy = center.y.0;
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (x, y) in [(left, top), (right, top), (right, bottom), (left, bottom)] {
        let dx = x - cx;
        let dy = y - cy;
        let rotated_x = cx + dx * cos - dy * sin;
        let rotated_y = cy + dx * sin + dy * cos;
        min_x = min_x.min(rotated_x);
        min_y = min_y.min(rotated_y);
        max_x = max_x.max(rotated_x);
        max_y = max_y.max(rotated_y);
    }
    Bounds::new(
        point(ScaledPixels(min_x), ScaledPixels(min_y)),
        size(
            ScaledPixels((max_x - min_x).max(0.0)),
            ScaledPixels((max_y - min_y).max(0.0)),
        ),
    )
}

fn root_backdrop_influence_radius(blur: &PaintBackdropBlur) -> ScaledPixels {
    let radius = blur.radius.0.abs();
    if !radius.is_finite() || radius <= 0.0 {
        return ScaledPixels(0.0);
    }
    let linear_footprint = 0.5 * f32::from(blur.downsample.max(1));
    ScaledPixels(radius * 3.0 + linear_footprint)
}
