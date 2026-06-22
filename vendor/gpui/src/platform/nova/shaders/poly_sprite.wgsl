// --- polychrome sprites --- //

struct PolychromeSprite {
    order: u32,
    pad: u32,
    grayscale: u32,
    opacity: f32,
    bounds: Bounds,
    content_mask: Bounds,
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
    out.clip_distances = distance_from_clip_rect(unit_vertex, sprite.bounds, sprite.content_mask);
    out.bounds = vec4<f32>(sprite.bounds.origin, sprite.bounds.size);
    out.corner_radii = vec4<f32>(
        sprite.corner_radii.top_left,
        sprite.corner_radii.top_right,
        sprite.corner_radii.bottom_right,
        sprite.corner_radii.bottom_left,
    );
    return out;
}

@fragment
fn fs_poly_sprite(input: PolySpriteVarying) -> @location(0) vec4<f32> {
    if (any(input.clip_distances < vec4<f32>(0.0))) {
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

    let sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0);
    if (sample.a <= 0.0) {
        return vec4<f32>(0.0);
    }

    let grayscale = dot(sample.rgb, GRAYSCALE_FACTORS);
    let grayscale_factor = select(0.0, 1.0, (input.grayscale & 0xFFu) != 0u);
    let color = vec4<f32>(mix(sample.rgb, vec3<f32>(grayscale), grayscale_factor), sample.a);
    return blend_color(color, input.opacity * coverage);
}
