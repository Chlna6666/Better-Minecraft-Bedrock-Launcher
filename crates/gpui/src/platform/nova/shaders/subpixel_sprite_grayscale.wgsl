// --- Transparent Windows grayscale subpixel-atlas sprites --- //

// ClearType RGB coverage cannot be represented faithfully once the window itself is composited over
// an unknown background. Collapse the already gamma-corrected RGB coverage to one scalar and emit a
// normal premultiplied fragment instead. This preserves the Windows subpixel glyph atlas while
// keeping transparent swapchain alpha correct.
@fragment
fn fs_subpixel_sprite_grayscale(input: MonoSpriteVarying) -> @location(0) vec4<f32> {
    let clip_coverage = content_mask_coverage_from_packed(input.position.xy, input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0)) || clip_coverage <= 0.0 || input.color.a <= 0.0) {
        discard;
    }

    let corrected = corrected_subpixel_coverage(input);
    let coverage = (corrected.r + corrected.g + corrected.b) * (1.0 / 3.0) * clip_coverage;
    return blend_color(input.color, coverage);
}
