use super::*;

/// Resolve the render plan used by the Nova swapchain path.
///
/// Nova's current retained partial-present implementation cannot submit native dirty rectangles to
/// the platform presentation API. A partial frame is therefore rendered into a full-size
/// `present_cache_texture` and then copied back to the swapchain with a second full-screen textured
/// pass. On Windows DirectComposition this makes a small UI animation pay an extra full-surface
/// sample/write pass every frame, which is often more expensive than redrawing the scene directly.
///
/// Keep GPUI's precise dirty region for diagnostics and for a future native dirty-rect present path,
/// but do not turn it into Nova's retained-cache copy path. Until the backends expose true partial
/// presentation, one direct scene pass to the swapchain is the lower-overhead and more predictable
/// presentation strategy.
pub(super) fn resolve_surface_render_plan<'a>(
    render_plan: FrameRenderPlan<'a>,
    _surface_requires_full_redraw: bool,
) -> FrameRenderPlan<'a> {
    render_plan.with_full_redraw()
}

pub(super) const fn can_present_retained_cache_only(
    _present_cache_valid: bool,
    _surface_requires_full_redraw: bool,
) -> bool {
    // The retained cache is intentionally not part of the active Nova presentation path. Returning
    // it directly would still require the same full-screen textured copy that this policy removes.
    false
}
