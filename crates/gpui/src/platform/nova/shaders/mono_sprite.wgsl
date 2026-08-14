// --- monochrome sprites --- //

@vertex
fn vs_mono_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> MonoSpriteVarying {
    return sprite_varying(vertex_id, instance_id);
}

@fragment
fn fs_mono_sprite(input: MonoSpriteVarying) -> @location(0) vec4<f32> {
    let clip_coverage = content_mask_coverage_from_packed(input.position.xy, input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }
    if (clip_coverage <= 0.0 || input.color.a <= 0.0) {
        return vec4<f32>(0.0);
    }

    let sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0).r;
    if (sample <= 0.0) {
        return vec4<f32>(0.0);
    }

    let alpha_corrected = apply_contrast_and_gamma_correction(
        sample,
        input.color.rgb,
        text_raster_params.grayscale_enhanced_contrast,
        text_raster_params.gamma_ratios
    );
    return blend_color(input.color, alpha_corrected * clip_coverage);
}
