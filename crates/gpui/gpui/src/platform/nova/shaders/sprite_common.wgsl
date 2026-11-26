// --- shared sprite layout --- //

struct MonochromeSprite {
    animation_slot: u32,
    pad: u32,
    bounds: Bounds,
    content_mask: ContentMask,
    color: Rgba,
    tile: AtlasTile,
    transformation: TransformationMatrix,
}
@group(0) @binding(8) var<storage, read> b_mono_sprites: array<MonochromeSprite>;

struct MonoSpriteVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) tile_position: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) subpixel_raster_safe: u32,
    @location(3) clip_distances: vec4<f32>,
    @location(4) @interpolate(flat) content_mask_bounds: vec4<f32>,
    @location(5) @interpolate(flat) content_mask_radii: vec4<f32>,
}

fn sprite_varying(vertex_id: u32, instance_id: u32) -> MonoSpriteVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = b_mono_sprites[instance_id];
    let animation = resolve_visual_animation(sprite.animation_slot, sprite.bounds);
    let bounds = animation_bounds(sprite.bounds, animation);
    let content_mask = animation_content_mask(sprite.content_mask, animation);

    // A DirectWrite RGB glyph encodes coverage for one physical LCD pixel grid. The glyph atlas
    // can be intentionally rasterized above/below the scene's stable bounds so a renderer-owned
    // scale lands on a native 1:1 raster at its target. Reconstruct that raster density directly
    // from the atlas tile and the unanimated primitive bounds; no extra per-glyph GPU ABI is needed.
    var raster_scale = 1.0;
    let static_width = abs(sprite.bounds.size.x);
    let static_height = abs(sprite.bounds.size.y);
    if (static_width > 0.0001 && sprite.tile.bounds.size.x > 0) {
        raster_scale = f32(sprite.tile.bounds.size.x) / static_width;
    } else if (static_height > 0.0001 && sprite.tile.bounds.size.y > 0) {
        raster_scale = f32(sprite.tile.bounds.size.y) / static_height;
    }
    let geometry_scale = select(1.0, animation.scale, animation.scales_geometry != 0u);
    let scale_epsilon = max(0.001, abs(raster_scale) * 0.001);

    // Matching raster density alone is not enough for RGB ClearType. The three coverage channels
    // are tied to one physical LCD phase, so a fractional GPU translation or an additional sprite
    // rotation/scale makes the cached DirectWrite phase invalid even when the glyph still occupies
    // one texture texel per device pixel. Preserve RGB only for an axis-aligned 1:1 sprite transform
    // whose final raster origin remains on the integer device-pixel grid. Integer translations are
    // safe; fractional translation/transform frames fall back to neutral grayscale coverage.
    let transform_epsilon = 0.0001;
    let axis_aligned_identity =
        abs(sprite.transformation.rotation_scale[0][0] - 1.0) <= transform_epsilon &&
        abs(sprite.transformation.rotation_scale[0][1]) <= transform_epsilon &&
        abs(sprite.transformation.rotation_scale[1][0]) <= transform_epsilon &&
        abs(sprite.transformation.rotation_scale[1][1] - 1.0) <= transform_epsilon;
    let transformed_origin =
        transpose(sprite.transformation.rotation_scale) * bounds.origin +
        sprite.transformation.translation;
    let phase_epsilon = 0.001;
    let phase_aligned = all(
        abs(transformed_origin - round(transformed_origin)) <= vec2<f32>(phase_epsilon)
    );

    var out = MonoSpriteVarying();
    out.position = to_device_position_transformed(unit_vertex, bounds, sprite.transformation);
    out.tile_position = to_tile_position(unit_vertex, sprite.tile);
    out.color = rgba_to_vec4(sprite.color);
    out.color.a *= animation.opacity;
    out.subpixel_raster_safe = select(
        0u,
        1u,
        abs(geometry_scale - raster_scale) <= scale_epsilon &&
            axis_aligned_identity &&
            phase_aligned,
    );
    out.clip_distances = distance_from_clip_rect_transformed(unit_vertex, bounds, content_mask.bounds, sprite.transformation);
    out.content_mask_bounds = vec4<f32>(content_mask.corner_bounds.origin, content_mask.corner_bounds.size);
    out.content_mask_radii = vec4<f32>(content_mask.corner_radii.top_left, content_mask.corner_radii.top_right, content_mask.corner_radii.bottom_right, content_mask.corner_radii.bottom_left);
    return out;
}
