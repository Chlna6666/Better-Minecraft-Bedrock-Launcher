use super::*;

pub(super) fn resolve_surface_render_plan<'a>(
    render_plan: FrameRenderPlan<'a>,
    surface_requires_full_redraw: bool,
) -> FrameRenderPlan<'a> {
    if surface_requires_full_redraw {
        render_plan.with_full_redraw()
    } else {
        render_plan
    }
}
