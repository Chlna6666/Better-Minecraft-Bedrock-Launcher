// --- RGB subpixel sprites --- //

struct SubpixelSpriteFragmentOutput {
    @location(0) @blend_src(0) foreground: vec4<f32>,
    @location(0) @blend_src(1) coverage: vec4<f32>,
}

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

@fragment
fn fs_subpixel_sprite(input: MonoSpriteVarying) -> SubpixelSpriteFragmentOutput {
    let clip_coverage = content_mask_coverage_from_packed(input.position.xy, input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0)) || clip_coverage <= 0.0 || input.color.a <= 0.0) {
        // The opaque Windows RGB text pipeline uses dual-source blending. Discard clipped fragments
        // so neither destination color nor alpha is touched outside the glyph coverage.
        discard;
    }

    let corrected = corrected_subpixel_coverage(input);
    var out = SubpixelSpriteFragmentOutput();
    out.foreground = vec4<f32>(input.color.rgb, 1.0);
    out.coverage = vec4<f32>(input.color.a * corrected * clip_coverage, 1.0);
    return out;
}

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
