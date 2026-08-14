// --- shared sprite layout --- //

struct MonochromeSprite {
    order: u32,
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
    @location(3) clip_distances: vec4<f32>,
    @location(4) @interpolate(flat) content_mask_bounds: vec4<f32>,
    @location(5) @interpolate(flat) content_mask_radii: vec4<f32>,
}

fn sprite_varying(vertex_id: u32, instance_id: u32) -> MonoSpriteVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = b_mono_sprites[instance_id];

    var out = MonoSpriteVarying();
    out.position = to_device_position_transformed(unit_vertex, sprite.bounds, sprite.transformation);
    out.tile_position = to_tile_position(unit_vertex, sprite.tile);
    out.color = rgba_to_vec4(sprite.color);
    out.clip_distances = distance_from_clip_rect_transformed(unit_vertex, sprite.bounds, sprite.content_mask.bounds, sprite.transformation);
    out.content_mask_bounds = vec4<f32>(sprite.content_mask.corner_bounds.origin, sprite.content_mask.corner_bounds.size);
    out.content_mask_radii = vec4<f32>(sprite.content_mask.corner_radii.top_left, sprite.content_mask.corner_radii.top_right, sprite.content_mask.corner_radii.bottom_right, sprite.content_mask.corner_radii.bottom_left);
    return out;
}
