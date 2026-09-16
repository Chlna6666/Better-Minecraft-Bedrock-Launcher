// --- Shared Windows subpixel sprite helpers --- //

@vertex
fn vs_subpixel_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> MonoSpriteVarying {
    return sprite_varying(vertex_id, instance_id);
}

fn corrected_subpixel_coverage(input: MonoSpriteVarying) -> vec3<f32> {
    var sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0).rgb;
    let packed = text_raster_params.subpixel_config;
    if ((packed & 2147483648u) != 0u) {
        sample = sample.bgr;
    }

    // DirectWrite ClearTypeLevel is a continuous 0..1 blend between grayscale and full
    // subpixel coverage. The CPU stores its exact positive f32 bit pattern in bits 0..30;
    // bit 31 is reserved for pixel geometry (RGB/BGR).
    let clear_type_level = saturate(bitcast<f32>(packed & 2147483647u));
    let neutral_coverage = (sample.r + sample.g + sample.b) / 3.0;
    sample = mix(vec3<f32>(neutral_coverage), sample, clear_type_level);

    return apply_contrast_and_gamma_correction3(
        sample,
        input.color.rgb,
        text_raster_params.subpixel_enhanced_contrast,
        text_raster_params.gamma_ratios
    );
}
