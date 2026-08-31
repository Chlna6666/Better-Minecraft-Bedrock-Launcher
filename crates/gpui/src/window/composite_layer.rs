use super::*;

// paint_element_blur currently treats sigma <= 0 as a no-op. Route the capture through its mature
// subtree isolation/animation-promotion machinery with an identity kernel, then immediately turn
// the completed root capture into a real zero-filter compositor record.
const COMPOSITE_CAPTURE_IDENTITY_SIGMA: f32 = 1.0 / 4096.0;

impl Window {
    pub(crate) fn paint_composite_layer<R>(
        &mut self,
        bounds: Bounds<Pixels>,
        paint: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint();
        let checkpoint = self.next_frame.scene.composite_capture_checkpoint();
        let result = self.paint_element_blur(
            bounds,
            px(COMPOSITE_CAPTURE_IDENTITY_SIGMA),
            paint,
        );
        self.next_frame.scene.finalize_composite_capture(checkpoint);
        result
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
        let bounds = self.next_frame.scene.animation_visual_bounds(animation_id)?;
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
