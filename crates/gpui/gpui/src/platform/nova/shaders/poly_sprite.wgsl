// --- polychrome sprites --- //

struct PolychromeSprite {
    animation_slot: u32,
    sampling: u32,
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
    @location(11) @interpolate(flat) sampling: u32,
}

@vertex
fn vs_poly_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> PolySpriteVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = b_poly_sprites[instance_id];
    let animation = resolve_visual_animation(sprite.animation_slot, sprite.bounds);
    var bounds = animation_bounds(sprite.bounds, animation);
    var content_mask = animation_content_mask(sprite.content_mask, animation);
    // Resident tiles use binding 17 as a placement table, independent of
    // normal animation state. Geometry moves while the viewport stays fixed.
    if (sprite.sampling == 2u && sprite.animation_slot != 0u) {
        let placement = b_animation_values[sprite.animation_slot - 1u];
        bounds = sprite.bounds;
        bounds.origin += placement.from_value.xy;
        let viewport = Bounds(placement.to_value.xy, placement.to_value.zw);
        content_mask.bounds = viewport;
        content_mask.corner_bounds = viewport;
    }
    let corner_radii = animation_corners(sprite.corner_radii, animation);

    var out = PolySpriteVarying();
    out.position = to_device_position(unit_vertex, bounds);
    out.tile_position = to_tile_position(unit_vertex, sprite.tile);
    out.grayscale = sprite.grayscale;
    out.opacity = sprite.opacity * animation.opacity;
    out.clip_distances = distance_from_clip_rect(unit_vertex, bounds, content_mask.bounds);
    out.content_mask_bounds = vec4<f32>(content_mask.corner_bounds.origin, content_mask.corner_bounds.size);
    out.content_mask_radii = vec4<f32>(content_mask.corner_radii.top_left, content_mask.corner_radii.top_right, content_mask.corner_radii.bottom_right, content_mask.corner_radii.bottom_left);
    out.bounds = vec4<f32>(bounds.origin, bounds.size);
    out.corner_radii = vec4<f32>(
        corner_radii.top_left,
        corner_radii.top_right,
        corner_radii.bottom_right,
        corner_radii.bottom_left,
    );
    out.tile_origin = vec2<i32>(sprite.tile.bounds.origin);
    out.tile_size = max(vec2<i32>(1), vec2<i32>(sprite.tile.bounds.size));
    out.texture_kind = sprite.tile.texture_id.kind;
    out.sampling = sprite.sampling;
    return out;
}

// A GPU raster tile stores premultiplied color, unlike ordinary image atlas
// texels. Sample exact texels at integer device-pixel placements, and never
// multiply cached RGB by its alpha again.
const RESIDENT_TILE_SAMPLING: u32 = 2u;

fn blend_resident_tile(sample: vec4<f32>, coverage: f32) -> vec4<f32> {
    if (globals.premultiplied_alpha != 0u) {
        return sample * coverage;
    }
    return vec4<f32>(sample.rgb / max(sample.a, SHADER_EPSILON), sample.a * coverage);
}

fn sample_poly_sprite(input: PolySpriteVarying) -> vec4<f32> {
    if (input.sampling == 1u || input.sampling == RESIDENT_TILE_SAMPLING || input.texture_kind == 2u) {
        let atlas_size = max(vec2<i32>(1), vec2<i32>(textureDimensions(t_sprite, 0)));
        let requested_texel = vec2<i32>(floor(input.tile_position * vec2<f32>(atlas_size)));
        let tile_max = input.tile_origin + input.tile_size - vec2<i32>(1);
        let texel = clamp(requested_texel, input.tile_origin, tile_max);
        return textureLoad(t_sprite, texel, 0);
    }
    return textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0);
}

@fragment
fn fs_poly_sprite(input: PolySpriteVarying) -> @location(0) vec4<f32> {
    let clip_coverage = content_mask_coverage_from_packed(content_position(input.position.xy), input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }
    if (clip_coverage <= 0.0) {
        return vec4<f32>(0.0);
    }
    if (input.opacity <= 0.0) {
        return vec4<f32>(0.0);
    }

    let distance = quad_sdf_from_packed(content_position(input.position.xy), input.bounds, input.corner_radii);
    let coverage = saturate(SDF_ANTIALIAS_THRESHOLD - distance);
    if (coverage <= 0.0) {
        return vec4<f32>(0.0);
    }

    let sample = sample_poly_sprite(input);
    if (sample.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    if (input.sampling == RESIDENT_TILE_SAMPLING) {
        return blend_resident_tile(sample, input.opacity * coverage * clip_coverage);
    }

    let grayscale = dot(sample.rgb, GRAYSCALE_FACTORS);
    let grayscale_factor = select(0.0, 1.0, (input.grayscale & 0xFFu) != 0u);
    let color = vec4<f32>(mix(sample.rgb, vec3<f32>(grayscale), grayscale_factor), sample.a);
    return blend_color(color, input.opacity * coverage * clip_coverage);
}
