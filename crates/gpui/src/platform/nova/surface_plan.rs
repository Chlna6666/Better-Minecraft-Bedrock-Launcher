use super::*;

/// Resolve the render plan used by the Nova swapchain path.
///
/// Backdrop-filter presence by itself is not a reason to widen presentation to the full surface.
/// Window scene construction already folds each backdrop's filtered-output damage into the final
/// dirty region, while blur topology changes force one conservative full redraw. Nova separately
/// decides whether a backdrop source/filter target must be refreshed, so keeping a cached blur does
/// not require presenting unrelated pixels around it.
///
/// The surface/backend capability remains the hard safety gate. Transparent Windows composition
/// surfaces, unsupported backends, or other presentation modes that cannot preserve unchanged
/// pixels pass `surface_requires_full_redraw = true` and keep the conservative full-present path.
pub(super) fn resolve_surface_render_plan(
    render_plan: FrameRenderPlan<'_>,
    surface_requires_full_redraw: bool,
) -> FrameRenderPlan<'_> {
    if surface_requires_full_redraw {
        FrameRenderPlan {
            partial_present_mode: PartialPresentMode::FullRedraw,
            ..render_plan
        }
    } else {
        render_plan
    }
}
