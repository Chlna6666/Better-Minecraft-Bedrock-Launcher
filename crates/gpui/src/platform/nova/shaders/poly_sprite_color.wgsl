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

    var sample: vec4<f32>;
    if (input.sampling == 1u || input.texture_kind == 2u) {
        let atlas_size = max(vec2<i32>(1), vec2<i32>(textureDimensions(t_sprite, 0)));
        let requested_texel = vec2<i32>(floor(input.tile_position * vec2<f32>(atlas_size)));
        let tile_max = input.tile_origin + input.tile_size - vec2<i32>(1);
        let texel = clamp(requested_texel, input.tile_origin, tile_max);
        sample = textureLoad(t_sprite, texel, 0);
    } else {
        sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0);
    }
    if (sample.a <= 0.0) {
        return vec4<f32>(0.0);
    }

    return blend_color(sample, input.opacity * coverage * clip_coverage);
}
