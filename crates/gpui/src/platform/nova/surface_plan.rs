use super::*;

/// Resolve the render plan used by the Nova swapchain path.
///
/// Preserve GPUI's precise damage only when the active swapchain can consume native dirty regions
/// and the scene does not contain backdrop filters. Backdrop blur targets are retained GPU pixel
/// caches whose output depends on the exact pixels rendered before each blur barrier. The current
/// spatial damage model is not yet strong enough to prove that every dependency stays valid across
/// native dirty-rect presentation, especially when transparent client titlebars, animated atlas
/// content, and modal overlays are combined.
///
/// Treat a backdrop-filter scene as a correctness boundary for now: rebuild the full surface and
/// every retained backdrop target for each presented blur frame. This is intentionally conservative
/// and scoped to scenes that actually contain backdrop filters; normal retained scenes continue to
/// use partial presentation.
pub(super) fn resolve_surface_render_plan(
    render_plan: FrameRenderPlan<'_>,
    surface_requires_full_redraw: bool,
) -> FrameRenderPlan<'_> {
    let has_backdrop_blurs = render_plan.scene.has_backdrop_blurs();
    if surface_requires_full_redraw || has_backdrop_blurs {
        FrameRenderPlan {
            partial_present_mode: PartialPresentMode::FullRedraw,
            force_full_backdrop_blur_refresh: render_plan.force_full_backdrop_blur_refresh
                || has_backdrop_blurs,
            ..render_plan
        }
    } else {
        render_plan
    }
}
