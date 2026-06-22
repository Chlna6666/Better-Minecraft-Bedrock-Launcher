// Fast path for solid, unrounded, borderless quads.

// Keep in sync with quad.wgsl; both read the same packed Quad buffer.
struct Quad {
    order: u32,
    border_style: u32,
    bounds: Bounds,
    content_mask: Bounds,
    background: Background,
    border_color: Hsla,
    corner_radii: Corners,
    border_widths: Edges,
}
@group(0) @binding(1) var<storage, read> b_quads: array<Quad>;

struct SolidQuadVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) clip_distances: vec4<f32>,
}

@vertex
fn vs_solid_quad(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> SolidQuadVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let quad = b_quads[instance_id];

    var out = SolidQuadVarying();
    out.position = to_device_position(unit_vertex, quad.bounds);
    out.color = hsla_to_rgba(quad.background.solid);
    out.clip_distances = distance_from_clip_rect(unit_vertex, quad.bounds, quad.content_mask);
    return out;
}

@fragment
fn fs_solid_quad(input: SolidQuadVarying) -> @location(0) vec4<f32> {
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }
    if (input.color.a <= 0.0) {
        return vec4<f32>(0.0);
    }

    return blend_color(input.color, 1.0);
}
