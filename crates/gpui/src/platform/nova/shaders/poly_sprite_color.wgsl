// Keep the general entry unchanged: separate static specialization avoids
// grayscale work without changing fallback shader code generation.
@fragment
fn fs_poly_sprite_color(input: PolySpriteVarying) -> @location(0) vec4<f32> {
    let clip_coverage = content_mask_coverage_from_packed(content_position(input.position.xy), input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }
    if (clip_coverage <= 0.0) {
        return vec4<f32>(0.0);
    }
    if (input.opacity <= 0.0) {
        return vec4<f32>(0.0);
    }

    let distance = quad_sdf_from_packed(content_position(input.position.xy), input.bounds, input.corner_radii);
    let coverage = saturate(SDF_ANTIALIAS_THRESHOLD - distance);
    if (coverage <= 0.0) {
        return vec4<f32>(0.0);
    }

    let sample = sample_poly_sprite(input);
    if (sample.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    if (input.sampling == RESIDENT_TILE_SAMPLING) {
        return blend_resident_tile(sample, input.opacity * coverage * clip_coverage);
    }

    return blend_color(sample, input.opacity * coverage * clip_coverage);
}
