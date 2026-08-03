// --- polychrome sprites --- //

struct PolychromeSprite {
    order: u32,
    pad: u32,
    grayscale: u32,
    opacity: f32,
    bounds: Bounds,
    content_mask: ContentMask,
    corner_radii: Corners,
    tile: AtlasTile,
}
@group(0) @binding(9) var<storage, read> b_poly_sprites: array<PolychromeSprite>;

struct PolySpriteVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) tile_position: vec2<f32>,
    @location(1) @interpolate(flat) grayscale: u32,
    @location(2) @interpolate(flat) opacity: f32,
    @location(3) clip_distances: vec4<f32>,
    @location(4) @interpolate(flat) bounds: vec4<f32>,
    @location(5) @interpolate(flat) corner_radii: vec4<f32>,
    @location(6) @interpolate(flat) content_mask_bounds: vec4<f32>,
    @location(7) @interpolate(flat) content_mask_radii: vec4<f32>,
    @location(8) @interpolate(flat) tile_origin: vec2<i32>,
    @location(9) @interpolate(flat) tile_size: vec2<i32>,
    @location(10) @interpolate(flat) texture_kind: u32,
}

@vertex
fn vs_poly_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> PolySpriteVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = b_poly_sprites[instance_id];

    var out = PolySpriteVarying();
    out.position = to_device_position(unit_vertex, sprite.bounds);
    out.tile_position = to_tile_position(unit_vertex, sprite.tile);
    out.grayscale = sprite.grayscale;
    out.opacity = sprite.opacity;
    out.clip_distances = distance_from_clip_rect(unit_vertex, sprite.bounds, sprite.content_mask.bounds);
    out.content_mask_bounds = vec4<f32>(sprite.content_mask.corner_bounds.origin, sprite.content_mask.corner_bounds.size);
    out.content_mask_radii = vec4<f32>(sprite.content_mask.corner_radii.top_left, sprite.content_mask.corner_radii.top_right, sprite.content_mask.corner_radii.bottom_right, sprite.content_mask.corner_radii.bottom_left);
    out.bounds = vec4<f32>(sprite.bounds.origin, sprite.bounds.size);
    out.corner_radii = vec4<f32>(
        sprite.corner_radii.top_left,
        sprite.corner_radii.top_right,
        sprite.corner_radii.bottom_right,
        sprite.corner_radii.bottom_left,
    );
    out.tile_origin = vec2<i32>(sprite.tile.bounds.origin);
    out.tile_size = max(vec2<i32>(1), vec2<i32>(sprite.tile.bounds.size));
    out.texture_kind = sprite.tile.texture_id.kind;
    return out;
}

@fragment
fn fs_poly_sprite(input: PolySpriteVarying) -> @location(0) vec4<f32> {
    let clip_coverage = content_mask_coverage_from_packed(input.position.xy, input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }
    if (clip_coverage <= 0.0) {
        return vec4<f32>(0.0);
    }
    if (input.opacity <= 0.0) {
        return vec4<f32>(0.0);
    }

    let distance = quad_sdf_from_packed(input.position.xy, input.bounds, input.corner_radii);
    let coverage = saturate(SDF_ANTIALIAS_THRESHOLD - distance);
    if (coverage <= 0.0) {
        return vec4<f32>(0.0);
    }

    var sample: vec4<f32>;
    if (input.texture_kind == 2u) {
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

    let grayscale = dot(sample.rgb, GRAYSCALE_FACTORS);
    let grayscale_factor = select(0.0, 1.0, (input.grayscale & 0xFFu) != 0u);
    let color = vec4<f32>(mix(sample.rgb, vec3<f32>(grayscale), grayscale_factor), sample.a);
    return blend_color(color, input.opacity * coverage * clip_coverage);
}