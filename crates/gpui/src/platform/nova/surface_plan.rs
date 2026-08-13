use super::*;

/// Resolve the render plan used by the Nova swapchain path.
///
/// Preserve GPUI's precise damage only when the active swapchain can consume native dirty regions.
/// Other swapchains render directly in full without introducing a retained present-cache copy.
pub(super) fn resolve_surface_render_plan(
    render_plan: FrameRenderPlan<'_>,
    surface_requires_full_redraw: bool,
) -> FrameRenderPlan<'_> {
    if surface_requires_full_redraw {
        render_plan.surface_requires_full_redraw()
    } else {
        render_plan
    }
}
