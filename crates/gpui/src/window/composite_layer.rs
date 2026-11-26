use super::*;

impl Window {
    pub(crate) fn paint_composite_layer<R>(
        &mut self,
        bounds: Bounds<Pixels>,
        paint: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.paint_isolated_layer(bounds, 0.0, paint)
    }

    /// Resolves the actual pixel-producing bounds owned by one scene animation after paint.
    /// Composite layers use their finalized tight capture instead of the element's layout box.
    pub(crate) fn scene_animation_visual_bounds(
        &self,
        animation_id: crate::SceneAnimationId,
    ) -> Option<Bounds<Pixels>> {
        let scale_factor = self.scale_factor();
        if !scale_factor.is_finite() || scale_factor <= f32::EPSILON {
            return None;
        }
        let bounds = self
            .next_frame
            .painter
            .animation_visual_bounds(animation_id)?;
        Some(Bounds::new(
            point(
                px(bounds.origin.x.0 / scale_factor),
                px(bounds.origin.y.0 / scale_factor),
            ),
            size(
                px(bounds.size.width.0 / scale_factor),
                px(bounds.size.height.0 / scale_factor),
            ),
        ))
    }

    /// Replaces the conservative registration-time animation damage bounds after the first paint
    /// has revealed a tighter retained-scene footprint.
    pub(crate) fn set_scene_animation_dirty_bounds(
        &self,
        element_id: &crate::GlobalElementId,
        property: crate::TransitionProperty,
        bounds: Bounds<Pixels>,
    ) -> bool {
        self.animation_engine
            .borrow_mut()
            .set_transition_bounds(element_id, property, bounds)
    }
}
