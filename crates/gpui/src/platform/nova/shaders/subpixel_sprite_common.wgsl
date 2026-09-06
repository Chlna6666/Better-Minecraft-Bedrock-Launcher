// --- Shared Windows subpixel sprite helpers --- //

@vertex
fn vs_subpixel_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> MonoSpriteVarying {
    return sprite_varying(vertex_id, instance_id);
}

fn corrected_subpixel_coverage(input: MonoSpriteVarying) -> vec3<f32> {
    var sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0).rgb;
    if (text_raster_params.is_bgr != 0u) {
        sample = sample.bgr;
    }
    return apply_contrast_and_gamma_correction3(
        sample,
        input.color.rgb,
        text_raster_params.subpixel_enhanced_contrast,
        text_raster_params.gamma_ratios
    );
}
