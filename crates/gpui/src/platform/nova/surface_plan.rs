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
pub(super) fn resolve_surface_packet(
    packet: &mut PresentationPacket,
    surface_requires_full_redraw: bool,
) {
    if surface_requires_full_redraw || packet.paint.scene.has_backdrop_blurs() {
        packet.partial_present_mode = PartialPresentMode::FullRedraw;
    }
}

/// Size transitions invalidate the whole drawable: previously recorded
/// dirty rectangles and retained source scissors refer to another extent.
/// Stable animations and static filters still use precise swept-pixel damage,
/// avoiding repeated full-surface rerasterization on every active frame.
/// Keep this independent from native partial-present capability.
pub(super) fn requires_full_drawable_damage(
    live_resize_active: bool,
    viewport_changed: bool,
) -> bool {
    // Only an actual drawable-extent transition destroys the correctness
    // proof for existing offscreen texels and their scissors. A nested
    // Capture + active compositor animation is NOT a resize: that old test
    // forced full damage and full filter refresh on every Dropdown tick,
    // overriding the precise GPU-sample damage computed before this call.
    //
    // WSI may still require a full swapchain Present (resolve_surface_packet),
    // but that is independent of the dirty region and retained Filter inputs.
    live_resize_active || viewport_changed
}

/// A newly recreated swapchain has no trustworthy old backbuffer pixels.
/// Partial GPU scissor and retained target damage must therefore cover its
/// entire current extent for the first *successfully presented* new-size frame.
/// The caller keeps `swapchain_warmup_frames` armed on a deferred/failed Present.
pub(super) fn force_full_drawable_damage_after_resize(
    packet: &mut PresentationPacket,
    drawable_size: super::renderer::DrawableSize,
) {
    let bounds = crate::bounds(
        crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
        crate::size(
            crate::ScaledPixels(drawable_size.width as f32),
            crate::ScaledPixels(drawable_size.height as f32),
        ),
    );
    packet.dirty_region = crate::DirtyRegion::full(bounds);
    packet.partial_present_mode = PartialPresentMode::FullRedraw;
    packet.force_full_backdrop_blur_refresh = true;
}
