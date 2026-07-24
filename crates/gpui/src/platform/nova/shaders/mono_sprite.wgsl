// --- monochrome sprites --- //

struct MonochromeSprite {
    order: u32,
    pad: u32,
    bounds: Bounds,
    content_mask: ContentMask,
    color: Hsla,
    tile: AtlasTile,
    transformation: TransformationMatrix,
}
@group(0) @binding(8) var<storage, read> b_mono_sprites: array<MonochromeSprite>;

struct MonoSpriteVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) tile_position: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(3) clip_distances: vec4<f32>,
    @location(4) @interpolate(flat) content_mask_bounds: vec4<f32>,
    @location(5) @interpolate(flat) content_mask_radii: vec4<f32>,
}

@vertex
fn vs_mono_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> MonoSpriteVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = b_mono_sprites[instance_id];

    var out = MonoSpriteVarying();
    out.position = to_device_position_transformed(unit_vertex, sprite.bounds, sprite.transformation);
    out.tile_position = to_tile_position(unit_vertex, sprite.tile);
    out.color = hsla_to_rgba(sprite.color);
    out.clip_distances = distance_from_clip_rect_transformed(unit_vertex, sprite.bounds, sprite.content_mask.bounds, sprite.transformation);
    out.content_mask_bounds = vec4<f32>(sprite.content_mask.corner_bounds.origin, sprite.content_mask.corner_bounds.size);
    out.content_mask_radii = vec4<f32>(sprite.content_mask.corner_radii.top_left, sprite.content_mask.corner_radii.top_right, sprite.content_mask.corner_radii.bottom_right, sprite.content_mask.corner_radii.bottom_left);
    return out;
}

@fragment
fn fs_mono_sprite(input: MonoSpriteVarying) -> @location(0) vec4<f32> {
    let clip_coverage = content_mask_coverage_from_packed(input.position.xy, input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }
    if (clip_coverage <= 0.0) {
        return vec4<f32>(0.0);
    }
    if (input.color.a <= 0.0) {
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

    // convert to srgb space as the rest of the code (output swapchain) expects that
    return blend_color(input.color, alpha_corrected * clip_coverage);
}
