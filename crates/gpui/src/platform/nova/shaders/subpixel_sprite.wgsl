// --- Opaque Windows RGB subpixel sprites --- //

struct SubpixelSpriteFragmentOutput {
    @location(0) @blend_src(0) foreground: vec4<f32>,
    @location(0) @blend_src(1) coverage: vec4<f32>,
}

@fragment
fn fs_subpixel_sprite(input: MonoSpriteVarying) -> SubpixelSpriteFragmentOutput {
    let clip_coverage = content_mask_coverage_from_packed(input.position.xy, input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0)) || clip_coverage <= 0.0 || input.color.a <= 0.0) {
        // The opaque Windows RGB text pipeline uses dual-source blending. Discard clipped fragments
        // so neither destination color nor alpha is touched outside the glyph coverage.
        discard;
    }

    var corrected = corrected_subpixel_coverage(input);
    if (input.subpixel_scale_safe == 0u) {
        // RGB ClearType coverage is tied to the physical LCD stripe grid. While a renderer-owned
        // scale samples the glyph at any density other than its DirectWrite raster density, using
        // the three channels independently changes subpixel phase and apparent stroke weight.
        // Preserve the already gamma/contrast-corrected outline, but neutralize it to scalar
        // coverage until geometry returns to a native 1:1 raster mapping.
        let neutral = (corrected.r + corrected.g + corrected.b) * (1.0 / 3.0);
        corrected = vec3<f32>(neutral);
    }

    var out = SubpixelSpriteFragmentOutput();
    out.foreground = vec4<f32>(input.color.rgb, 1.0);
    out.coverage = vec4<f32>(input.color.a * corrected * clip_coverage, 1.0);
    return out;
}
