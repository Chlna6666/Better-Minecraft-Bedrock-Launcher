// --- Transparent Windows grayscale subpixel-atlas sprites --- //

// ClearType RGB coverage cannot be represented faithfully once the window itself is composited over
// an unknown background. Collapse the raw subpixel-atlas coverage first, then run DirectWrite's
// scalar grayscale contrast/gamma correction and emit a normal premultiplied fragment. This avoids
// both color-fringe semantics and three unnecessary per-channel corrections on transparent surfaces.
@fragment
fn fs_subpixel_sprite_grayscale(input: MonoSpriteVarying) -> @location(0) vec4<f32> {
    let clip_coverage = content_mask_coverage_from_packed(input.position.xy, input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0)) || clip_coverage <= 0.0 || input.color.a <= 0.0) {
        discard;
    }

    let coverage = corrected_grayscale_subpixel_atlas_coverage(input) * clip_coverage;
    return blend_color(input.color, coverage);
}
