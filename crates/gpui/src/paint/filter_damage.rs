use super::*;
const MAX_ACCUMULATED_BACKDROP_DAMAGE_RECTS: usize = 16;

#[derive(Clone, Debug, Default)]
pub(crate) struct BackdropBlurDamagePlan {
    entries: SmallVec<[BackdropBlurSourceDamage; 4]>,
}

#[derive(Clone, Debug)]
struct BackdropBlurSourceDamage {
    order: DrawOrder,
    source_damage: SmallVec<[Bounds<ScaledPixels>; 4]>,
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
    ) -> (bool, impl Iterator<Item = Bounds<ScaledPixels>>) {
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

    /// Conservatively accumulates damage that has not reached a submitted frame yet.
    ///
    /// Pending presentation is latest-wins: if UI commits B and then C while A is still on screen,
    /// C must carry both A→B and B→C blur-source damage. Full refresh always dominates spatial
    /// damage for the same blur order.
    pub(crate) fn merge_from(&mut self, previous: &Self) {
        for entry in &previous.entries {
            if entry.full_refresh {
                self.mark_full(entry.order);
                continue;
            }
            for damage in &entry.source_damage {
                self.push(entry.order, *damage);
                if self.entry_mut(entry.order).source_damage.len()
                    > MAX_ACCUMULATED_BACKDROP_DAMAGE_RECTS
                {
                    self.mark_full(entry.order);
                    break;
                }
            }
        }
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
            source_damage: SmallVec::new(),
            full_refresh: false,
        });
        &mut self.entries[index]
    }
}

/// Computes spatial source damage independently for every backdrop draw-order barrier.
pub(crate) fn backdrop_blur_damage_plan(
    scene: &Scene,
    previous: &Scene,
    presentation_values: &[SceneAnimationValue],
    previous_presentation_values: &[SceneAnimationValue],
) -> BackdropBlurDamagePlan {
    let mut plan = BackdropBlurDamagePlan::default();
    if !scene.has_backdrop_blurs() && !previous.has_backdrop_blurs() {
        return plan;
    }

    let current_blurs = backdrop_blur_operations(&scene.paint_operations);
    let previous_blurs = backdrop_blur_operations(&previous.paint_operations);
    if current_blurs.clone().count() != previous_blurs.clone().count() {
        for (_, blur) in current_blurs {
            plan.mark_full(blur.order);
        }
        return plan;
    }

    for ((current_index, current_blur), (previous_index, previous_blur)) in
        current_blurs.zip(previous_blurs)
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
            &scene.paint_operations[..current_index],
            &previous.paint_operations[..previous_index],
            source_region,
            |damage| plan.push(current_blur.order, damage),
        );
    }

    crate::paint::collect_backdrop_blur_animation_damage(
        scene,
        previous,
        presentation_values,
        previous_presentation_values,
        &mut plan,
    );
    plan
}

/// Computes per-backdrop source damage for an animation-only retained-scene frame.
pub(crate) fn backdrop_blur_animation_damage_plan(
    scene: &Scene,
    previous_presentation_values: &[SceneAnimationValue],
    presentation_values: &[SceneAnimationValue],
) -> BackdropBlurDamagePlan {
    let mut plan = BackdropBlurDamagePlan::default();
    crate::paint::collect_backdrop_blur_animation_damage(
        scene,
        scene,
        presentation_values,
        previous_presentation_values,
        &mut plan,
    );
    plan
}

pub(crate) fn collect_backdrop_blur_animation_damage(
    scene: &Scene,
    previous_scene: &Scene,
    presentation_values: &[SceneAnimationValue],
    previous_presentation_values: &[SceneAnimationValue],
    plan: &mut BackdropBlurDamagePlan,
) {
    if !scene.has_backdrop_blurs() {
        return;
    }

    for (blur_index, blur) in backdrop_blur_operations(&scene.paint_operations) {
        if animation_value_changed(
            scene,
            previous_scene,
            blur.animation_id,
            presentation_values,
            previous_presentation_values,
        ) {
            // A backdrop's own OPACITY animation affects only its final
            // composite. The already-filtered Gaussian texture still has
            // identical source pixels and sigma. Reuse the result even
            // while the overlay fades at display refresh rate.
            //
            // This does NOT suppress opacity changes of a backdrop that
            // appears before ANOTHER blur: the source loop below still
            // reports its changing pixels to that later filter.
            let composite_opacity_only = blur.animation_id.is_some_and(|animation_id| {
                matches!(
                    (
                        animation_value_for(scene, animation_id, presentation_values),
                        animation_value_for(previous_scene, animation_id, previous_presentation_values),
                    ),
                    (Some(current), Some(previous))
                        if current.property == TransitionProperty::Opacity
                            && previous.property == TransitionProperty::Opacity
                )
            });
            if !composite_opacity_only {
                plan.mark_full(blur.order);
                continue;
            }
        }
        let source_region = backdrop_blur_source_region(blur);
        if source_region.is_empty() {
            continue;
        }
        for operation in &scene.paint_operations[..blur_index] {
            let damage = match operation {
                PaintOperation::Primitive(primitive)
                    if animation_value_changed(
                        scene,
                        previous_scene,
                        primitive.animation_id(),
                        presentation_values,
                        previous_presentation_values,
                    ) =>
                {
                    Some(animation_swept_bounds(
                        scene,
                        previous_scene,
                        primitive,
                        presentation_values,
                        previous_presentation_values,
                    ))
                }
                PaintOperation::StartBlur(capture)
                    if animation_value_changed(
                        scene,
                        previous_scene,
                        capture.animation_id,
                        presentation_values,
                        previous_presentation_values,
                    ) =>
                {
                    Some(blur_capture_animation_swept_bounds(
                        scene,
                        previous_scene,
                        capture,
                        presentation_values,
                        previous_presentation_values,
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

pub(crate) fn backdrop_blur_damage(
    scene: &Scene,
    damage: Bounds<ScaledPixels>,
) -> impl Iterator<Item = Bounds<ScaledPixels>> {
    let mut damage_regions = SmallVec::<[Bounds<ScaledPixels>; 8]>::new();
    crate::paint::collect_backdrop_blur_damage(scene, damage, &mut damage_regions);
    damage_regions.into_iter()
}

pub(crate) fn backdrop_blur_output_damage(
    scene: &Scene,
    plan: &BackdropBlurDamagePlan,
) -> impl Iterator<Item = Bounds<ScaledPixels>> {
    let mut output_damage = SmallVec::<[Bounds<ScaledPixels>; 8]>::new();
    for (_, blur) in backdrop_blur_operations(&scene.paint_operations) {
        let (full_refresh, source_damage) = plan.source_damage_for_orders(blur.order, blur.order);
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

pub(crate) fn collect_backdrop_blur_damage(
    scene: &Scene,
    damage: Bounds<ScaledPixels>,
    damage_regions: &mut SmallVec<[Bounds<ScaledPixels>; 8]>,
) {
    for blur in &scene.backdrop_blurs {
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
    for blur in &scene.blurs {
        crate::paint::collect_backdrop_blur_damage(&blur.content, damage, damage_regions);
    }
}

pub(crate) fn backdrop_blur_operations(
    operations: &[PaintOperation],
) -> impl Iterator<Item = (usize, &PaintBackdropBlur)> + Clone {
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
}

pub(crate) fn backdrop_blur_source_region(blur: &PaintBackdropBlur) -> Bounds<ScaledPixels> {
    blur.bounds
        .intersect(&blur.content_mask.bounds)
        .dilate(backdrop_blur_influence_radius(blur))
}

pub(crate) fn backdrop_blur_influence_radius(blur: &PaintBackdropBlur) -> ScaledPixels {
    let radius = blur.radius.0.abs();
    if !radius.is_finite() || radius <= 0.0 {
        return ScaledPixels(0.0);
    }

    // CSS blur radius is the Gaussian standard deviation. Three sigma contains the practical
    // filter support; add half of the source texel footprint for linear filtering and downsampling.
    let linear_footprint = 0.5 * f32::from(blur.downsample.max(1));
    ScaledPixels(radius * 3.0 + linear_footprint)
}
