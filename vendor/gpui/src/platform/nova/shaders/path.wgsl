// --- path rasterization --- //

struct PathRasterizationVertex {
    xy_position: vec2<f32>,
    st_position: vec2<f32>,
    color: Background,
    bounds: Bounds,
}

@group(0) @binding(3) var<storage, read> b_path_vertices: array<PathRasterizationVertex>;

struct PathRasterizationVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) st_position: vec2<f32>,
    @location(1) @interpolate(flat) background_tag: u32,
    @location(2) @interpolate(flat) background_color_space: u32,
    @location(3) @interpolate(flat) background_solid: vec4<f32>,
    @location(4) @interpolate(flat) background_color0: vec4<f32>,
    @location(5) @interpolate(flat) background_color1: vec4<f32>,
    @location(6) @interpolate(flat) background_pattern_or_angle: f32,
    @location(7) @interpolate(flat) background_stop_percentages: vec2<f32>,
    @location(8) @interpolate(flat) bounds: vec4<f32>,
    // TODO: use `clip_distance` once Naga supports it.
    @location(9) clip_distances: vec4<f32>,
}

@vertex
fn vs_path_rasterization(@builtin(vertex_index) vertex_id: u32) -> PathRasterizationVarying {
    let v = b_path_vertices[vertex_id];

    var out = PathRasterizationVarying();
    out.position = to_device_position_impl(v.xy_position);
    out.st_position = v.st_position;
    let prepared_color = prepare_gradient_color(
        v.color.tag,
        v.color.color_space,
        v.color.solid,
        v.color.colors,
    );
    out.background_tag = v.color.tag;
    out.background_color_space = v.color.color_space;
    out.background_solid = prepared_color.solid;
    out.background_color0 = prepared_color.color0;
    out.background_color1 = prepared_color.color1;
    out.background_pattern_or_angle = v.color.gradient_angle_or_pattern_height;
    out.background_stop_percentages = vec2<f32>(
        v.color.colors[0].percentage,
        v.color.colors[1].percentage,
    );
    out.bounds = vec4<f32>(v.bounds.origin, v.bounds.size);
    out.clip_distances = distance_from_clip_rect_impl(v.xy_position, v.bounds);
    return out;
}

@fragment
fn fs_path_rasterization(input: PathRasterizationVarying) -> @location(0) vec4<f32> {
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    let dx = dpdx(input.st_position);
    let dy = dpdy(input.st_position);
    let edge_gradient = vec2<f32>(dx.x, dy.x);
    var alpha: f32;
    if (length(edge_gradient) < 0.001) {
        // If the gradient is too small, return a solid color.
        alpha = 1.0;
    } else {
        let gradient = 2.0 * input.st_position.xx * edge_gradient - vec2<f32>(dx.y, dy.y);
        let f = input.st_position.x * input.st_position.x - input.st_position.y;
        let distance = f / max(length(gradient), SHADER_EPSILON);
        alpha = saturate(SDF_ANTIALIAS_THRESHOLD - distance);
    }
    if (alpha <= 0.0) {
        return vec4<f32>(0.0);
    }

    var color = input.background_solid;
    if (input.background_tag == 0u && color.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    if (input.background_tag != 0u) {
        let background = Background(
            input.background_tag,
            input.background_color_space,
            Hsla(0.0, 0.0, 0.0, 0.0),
            input.background_pattern_or_angle,
            array<LinearColorStop, 2>(
                LinearColorStop(
                    Hsla(0.0, 0.0, 0.0, 0.0),
                    input.background_stop_percentages.x,
                ),
                LinearColorStop(
                    Hsla(0.0, 0.0, 0.0, 0.0),
                    input.background_stop_percentages.y,
                ),
            ),
            0u,
        );
        let bounds = Bounds(input.bounds.xy, input.bounds.zw);
        color = gradient_color(background, input.position.xy, bounds,
            input.background_solid, input.background_color0, input.background_color1);
    }
    if (color.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(color.rgb * color.a * alpha, color.a * alpha);
}

// --- paths --- //

struct PathSprite {
    bounds: Bounds,
}
@group(0) @binding(6) var<storage, read> b_path_sprites: array<PathSprite>;

struct PathVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) texture_coords: vec2<f32>,
}

@vertex
fn vs_path(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> PathVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = b_path_sprites[instance_id];
    // Don't apply content mask because it was already accounted for when rasterizing the path.
    let device_position = to_device_position(unit_vertex, sprite.bounds);
    // For screen-space intermediate texture, convert screen position to texture coordinates
    let screen_position = sprite.bounds.origin + unit_vertex * sprite.bounds.size;
    let texture_coords = viewport_texture_coords(screen_position);

    var out = PathVarying();
    out.position = device_position;
    out.texture_coords = texture_coords;

    return out;
}

@fragment
fn fs_path(input: PathVarying) -> @location(0) vec4<f32> {
    let sample = textureSampleLevel(t_sprite, s_sprite, input.texture_coords, 0.0);
    return sample;
}
