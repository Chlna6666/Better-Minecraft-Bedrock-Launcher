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

    pub(crate) fn order_source_damage(
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
        if entry.full_refresh {
            return;
        }
        // Repeated hover/animation commits often report identical or nested
        // rectangles before the renderer can present. They do not add damage.
        if entry.source_damage.iter().any(|old| old.intersect(&damage) == damage) {
            return;
        }
        entry.source_damage.retain(|old| old.intersect(&damage) != *old);
        entry.source_damage.push(damage);
        if entry.source_damage.len() > MAX_ACCUMULATED_BACKDROP_DAMAGE_RECTS {
            // Bound metadata without promoting valid spatial damage to a full
            // Gaussian refresh. The bounding rectangle covers every source
            // pixel; the renderer still adds its normal convolution halo.
            let bounds = entry.source_damage.iter().copied()
                .reduce(|left, right| left.union(&right))
                .expect("nonempty backdrop damage");
            entry.source_damage.clear();
            entry.source_damage.push(bounds);
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
pub(crate) fn compute_backdrop_blur_damage(
    scene: &Scene,
    previous: &Scene,
    presentation_values: &[SceneAnimationValue],
    previous_presentation_values: &[SceneAnimationValue],
) -> BackdropBlurDamagePlan {
    let mut plan = BackdropBlurDamagePlan::default();
    if !scene.has_backdrop_blurs() && !previous.has_backdrop_blurs() {
        return plan;
    }

    let current_blurs = backdrop_blur_operations(scene.paint_operations.all());
    let previous_blurs = backdrop_blur_operations(previous.paint_operations.all());
    if current_blurs.clone().count() != previous_blurs.clone().count() {
        for (_, blur) in current_blurs {
            plan.mark_full(blur.order);
        }
        return plan;
    }

    // Each successive filter compares a longer prefix of the same two paint
    // streams. Verify the common prefix once instead of re-comparing static
    // chrome for every filter. Once a mismatch is found the existing
    // conservative per-barrier middle/suffix diff takes over.
    let mut verified_prefix = 0;
    let mut prefix_diverged = false;
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
        let common_end = current_index.min(previous_index);
        if !prefix_diverged {
            let matched = scene.paint_operations.range(verified_prefix..common_end)
                .iter()
                .zip(previous.paint_operations.range(verified_prefix..common_end).iter())
                .take_while(|(current, old)| current.visually_eq(old))
                .count();
            verified_prefix += matched;
            prefix_diverged = verified_prefix < common_end;
        }
        let start = verified_prefix.min(common_end);
        if start != current_index || start != previous_index {
            collect_paint_operation_damage(
                scene.paint_operations.range(start..current_index),
                previous.paint_operations.range(start..previous_index),
                source_region,
                |damage| plan.push(current_blur.order, damage),
            );
        }
    }

    crate::paint::collect_backdrop_blur_animation_damage(
        scene,
        previous,
        presentation_values,
        previous_presentation_values,
        &mut plan,
    );
    propagate_backdrop_output_damage(scene, &mut plan);
    plan
}

/// Computes per-backdrop source damage for an animation-only retained-scene frame.
pub(crate) fn compute_backdrop_blur_animation_damage(
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
    propagate_backdrop_output_damage(scene, &mut plan);
    plan
}

// A later backdrop samples the composited output of preceding barriers,
// not just the original primitives. A Gaussian can spread a changed source
// beyond the next filter's direct source footprint. Follow painter order so
// the expanded output of every affected barrier becomes downstream input.
// Same-order members may share a source snapshot; never feed them back into
// each other. Animated geometry without a proven output mapping retains a
// conservative downstream refresh.
fn propagate_backdrop_output_damage(scene: &Scene, plan: &mut BackdropBlurDamagePlan) {
    if plan.is_empty() {
        return;
    }
    let mut outputs = SmallVec::<[(DrawOrder, Bounds<ScaledPixels>); 16]>::new();
    let mut uncertain_order: Option<DrawOrder> = None;
    for (_, blur) in backdrop_blur_operations(scene.paint_operations.all()) {
        let source_region = backdrop_blur_source_region(blur);
        if source_region.is_empty() {
            continue;
        }
        if uncertain_order.is_some_and(|order| order < blur.order) {
            plan.mark_full(blur.order);
        } else {
            for (order, bounds) in &outputs {
                if *order < blur.order {
                    plan.push(blur.order, bounds.intersect(&source_region));
                }
            }
        }
        let (full, damage) = plan.order_source_damage(blur.order, blur.order);
        let mut damage = damage.peekable();
        if !full && damage.peek().is_none() {
            continue;
        }
        if blur.animation_id.is_some() {
            uncertain_order = Some(uncertain_order.map_or(blur.order, |order| order.min(blur.order)));
            continue;
        }
        let output = blur.bounds.intersect(&blur.content_mask.bounds);
        if full {
            if !output.is_empty() {
                outputs.push((blur.order, output));
            }
            continue;
        }
        let radius = backdrop_blur_influence_radius(blur);
        for source in damage {
            let affected = source.dilate(radius).intersect(&output);
            if !affected.is_empty() {
                outputs.push((blur.order, affected));
            }
        }
    }
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

    // A later barrier includes the preceding barrier's prefix. Scan each
    // operation at most once and reuse only changed swept bounds, rather
    // than re-evaluating every static primitive and animation per filter.
    let mut scanned_until = 0;
    let mut changed_bounds = SmallVec::<[Bounds<ScaledPixels>; 16]>::new();
    for (blur_index, blur) in backdrop_blur_operations(scene.paint_operations.all()) {
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
        for operation in scene.paint_operations.range(scanned_until..blur_index).iter() {
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
                && !damage.is_empty()
            {
                changed_bounds.push(damage);
            }
        }
        scanned_until = blur_index;
        for damage in &changed_bounds {
            if damage.intersects(&source_region) {
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
    for (_, blur) in backdrop_blur_operations(scene.paint_operations.all()) {
        let (full_refresh, source_damage) = plan.order_source_damage(blur.order, blur.order);
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
    operations: crate::scene::OperationRange<'_>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, width: f32) -> Bounds<ScaledPixels> {
        Bounds::new(crate::point(ScaledPixels(x), ScaledPixels(10.)),
            crate::size(ScaledPixels(width), ScaledPixels(4.)))
    }

    #[test]
    fn repeated_pending_damage_stays_spatial() {
        let mut accumulated = BackdropBlurDamagePlan::default();
        let mut next = BackdropBlurDamagePlan::default();
        next.push(0, rect(10., 4.));
        for _ in 0..64 {
            accumulated.merge_from(&next);
        }
        let (full, damage) = accumulated.order_source_damage(0, 0);
        assert!(!full);
        assert_eq!(damage.collect::<Vec<_>>(), vec![rect(10., 4.)]);
    }

    #[test]
    fn fragmented_source_damage_is_bounded_without_full_refresh() {
        let mut plan = BackdropBlurDamagePlan::default();
        for index in 0..=MAX_ACCUMULATED_BACKDROP_DAMAGE_RECTS {
            plan.push(0, rect(index as f32 * 10., 2.));
        }
        let (full, damage) = plan.order_source_damage(0, 0);
        assert!(!full);
        assert_eq!(damage.collect::<Vec<_>>(),
            vec![rect(0., MAX_ACCUMULATED_BACKDROP_DAMAGE_RECTS as f32 * 10. + 2.)]);
    }

    #[test]
    fn explicit_full_source_refresh_still_dominates() {
        let mut plan = BackdropBlurDamagePlan::default();
        plan.push(0, rect(10., 4.));
        let mut previous = BackdropBlurDamagePlan::default();
        previous.mark_full(0);
        plan.merge_from(&previous);
        plan.push(0, rect(20., 4.));
        let (full, mut damage) = plan.order_source_damage(0, 0);
        assert!(full);
        assert!(damage.next().is_none());
    }
}
