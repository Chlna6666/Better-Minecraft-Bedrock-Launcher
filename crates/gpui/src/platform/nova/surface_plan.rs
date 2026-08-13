use super::*;

/// Resolve the render plan used by the Nova swapchain path.
///
/// Nova's current retained partial-present implementation cannot submit native dirty rectangles to
/// the platform presentation API. A partial frame is therefore rendered into a full-size
/// `present_cache_texture` and then copied back to the swapchain with a second full-screen textured
/// pass. On Windows DirectComposition this makes a small UI animation pay an extra full-surface
/// sample/write pass every frame, which is often more expensive than redrawing the scene directly.
///
/// Normal interactive window frames currently use `FrameVisualEffectQuality::Full`. Prefer one
/// direct scene pass to the swapchain for that path while keeping the precise GPUI dirty region for
/// diagnostics and for a future native dirty-rect presentation API. Non-Full plans retain the old
/// partial-plan semantics so reduced/offscreen policies can opt into them explicitly.
pub(super) fn resolve_surface_render_plan<'a>(
    render_plan: FrameRenderPlan<'a>,
    surface_requires_full_redraw: bool,
) -> FrameRenderPlan<'a> {
    if surface_requires_full_redraw
        || render_plan.visual_effect_quality == FrameVisualEffectQuality::Full
    {
        render_plan.with_full_redraw()
    } else {
        render_plan
    }
}

pub(super) const fn can_present_retained_cache_only(
    present_cache_valid: bool,
    surface_requires_full_redraw: bool,
) -> bool {
    present_cache_valid && !surface_requires_full_redraw
}
