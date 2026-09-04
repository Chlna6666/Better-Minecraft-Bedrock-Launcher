use super::*;

/// Resolve the render plan used by the Nova swapchain path.
///
/// Backdrop-filter scenes still use a full swapchain presentation until Nova can prove native
/// dirty-rect composition is correct for every transparent/titlebar/atlas combination. That is a
/// presentation constraint only: the renderer already owns retained backdrop targets and precise
/// source-order damage, so a full present must not imply that every Gaussian target is invalid.
///
/// Keeping those two decisions independent is important for full-window background glass. A later
/// page/list animation may require a full swapchain present while leaving all pixels *before* the
/// background blur barrier unchanged. In that case the cached blur target remains valid and should
/// be sampled directly instead of recapturing and filtering the whole window.
pub(super) fn resolve_surface_render_plan(
    render_plan: FrameRenderPlan<'_>,
    surface_requires_full_redraw: bool,
) -> FrameRenderPlan<'_> {
    let has_backdrop_blurs = render_plan.scene.has_backdrop_blurs();
    if surface_requires_full_redraw || has_backdrop_blurs {
        FrameRenderPlan {
            partial_present_mode: PartialPresentMode::FullRedraw,
            ..render_plan
        }
    } else {
        render_plan
    }
}
