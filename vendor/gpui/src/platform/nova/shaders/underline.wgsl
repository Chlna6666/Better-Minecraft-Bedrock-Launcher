// --- underlines --- //

struct Underline {
    order: u32,
    pad: u32,
    bounds: Bounds,
    content_mask: Bounds,
    color: Hsla,
    thickness: f32,
    wavy: u32,
}
@group(0) @binding(7) var<storage, read> b_underlines: array<Underline>;

struct UnderlineVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) @interpolate(flat) bounds: vec4<f32>,
    @location(2) @interpolate(flat) thickness: f32,
    @location(3) @interpolate(flat) wavy: u32,
    // TODO: use `clip_distance` once Naga supports it.
    @location(4) clip_distances: vec4<f32>,
}

@vertex
fn vs_underline(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> UnderlineVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let underline = b_underlines[instance_id];

    var out = UnderlineVarying();
    out.position = to_device_position(unit_vertex, underline.bounds);
    out.color = hsla_to_rgba(underline.color);
    out.bounds = vec4<f32>(underline.bounds.origin, underline.bounds.size);
    out.thickness = underline.thickness;
    out.wavy = underline.wavy;
    out.clip_distances = distance_from_clip_rect(unit_vertex, underline.bounds, underline.content_mask);
    return out;
}

@fragment
fn fs_underline(input: UnderlineVarying) -> @location(0) vec4<f32> {
    const WAVE_FREQUENCY: f32 = 2.0;
    const WAVE_HEIGHT_RATIO: f32 = 0.8;

    // Alpha clip first, since we don't have `clip_distance`.
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }
    if (input.color.a <= 0.0) {
        return vec4<f32>(0.0);
    }

    let underline_height = input.bounds.w;
    if (underline_height <= SHADER_EPSILON || input.thickness <= SHADER_EPSILON) {
        return vec4<f32>(0.0);
    }

    if ((input.wavy & 0xFFu) == 0u)
    {
        return blend_color(input.color, 1.0);
    }

    let half_thickness = input.thickness * 0.5;

    let st = (input.position.xy - input.bounds.xy) / underline_height - vec2<f32>(0.0, 0.5);
    let frequency = M_PI_F * WAVE_FREQUENCY * input.thickness / underline_height;
    let amplitude = (input.thickness * WAVE_HEIGHT_RATIO) / underline_height;

    let sine = sin(st.x * frequency) * amplitude;
    let dSine = cos(st.x * frequency) * amplitude * frequency;
    let distance = (st.y - sine) / sqrt(1.0 + dSine * dSine);
    let distance_in_pixels = distance * underline_height;
    let distance_from_top_border = distance_in_pixels - half_thickness;
    let distance_from_bottom_border = distance_in_pixels + half_thickness;
    let stroke_distance = max(-distance_from_bottom_border, distance_from_top_border);
    let alpha = saturate(SDF_ANTIALIAS_THRESHOLD - stroke_distance);
    return blend_color(input.color, alpha);
}
