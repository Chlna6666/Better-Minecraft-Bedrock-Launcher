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
}
