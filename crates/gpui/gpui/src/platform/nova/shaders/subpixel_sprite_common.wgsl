// --- Shared Windows subpixel sprite helpers --- //

@vertex
fn vs_subpixel_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> MonoSpriteVarying {
    return sprite_varying(vertex_id, instance_id);
}

fn corrected_grayscale_coverage_from_subpixel_sample(sample: vec3<f32>, input: MonoSpriteVarying) -> f32 {
    // The current atlas stores DirectWrite's CLEARTYPE_3x1 coverage. Collapsing the raw channels is
    // the neutral-coverage approximation; apply the actual grayscale contrast/gamma model only
    // after that collapse instead of correcting three ClearType channels and averaging afterward.
    let neutral_sample = (sample.r + sample.g + sample.b) * (1.0 / 3.0);
    return apply_contrast_and_gamma_correction(
        neutral_sample,
        input.color.rgb,
        text_raster_params.grayscale_enhanced_contrast,
        text_raster_params.gamma_ratios
    );
}

fn corrected_grayscale_subpixel_atlas_coverage(input: MonoSpriteVarying) -> f32 {
    let sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0).rgb;
    return corrected_grayscale_coverage_from_subpixel_sample(sample, input);
}

fn corrected_subpixel_coverage(input: MonoSpriteVarying) -> vec3<f32> {
    let raw_sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0).rgb;
    let packed = text_raster_params.subpixel_config;

    // DirectWrite ClearTypeLevel is a continuous 0..1 blend between grayscale and full
    // subpixel coverage. The CPU stores its exact positive f32 bit pattern in bits 0..30;
    // bit 31 is reserved for pixel geometry (RGB/BGR).
    let clear_type_level = saturate(bitcast<f32>(packed & 2147483647u));

    // Keep both endpoints cheap and semantically distinct. ClearTypeLevel=0 uses the grayscale
    // contrast/gamma path; the common ClearTypeLevel=1 case executes only the RGB correction.
    if (clear_type_level <= 0.0) {
        return vec3<f32>(corrected_grayscale_coverage_from_subpixel_sample(raw_sample, input));
    }

    var sample = raw_sample;
    if ((packed & 2147483648u) != 0u) {
        sample = sample.bgr;
    }
    let subpixel = apply_contrast_and_gamma_correction3(
        sample,
        input.color.rgb,
        text_raster_params.subpixel_enhanced_contrast,
        text_raster_params.gamma_ratios
    );
    if (clear_type_level >= 1.0) {
        return subpixel;
    }

    let grayscale = corrected_grayscale_coverage_from_subpixel_sample(raw_sample, input);
    return mix(vec3<f32>(grayscale), subpixel, clear_type_level);
}
