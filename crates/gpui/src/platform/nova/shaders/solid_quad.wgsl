// Fast path for solid, unrounded, borderless quads.

// Keep in sync with quad.wgsl; both read the same packed Quad buffer.
struct Quad {
    animation_slot: u32,
    border_style: u32,
    bounds: Bounds,
    content_mask: ContentMask,
    background: Background,
    border_color: Rgba,
    corner_radii: Corners,
    border_widths: Edges,
}
@group(0) @binding(1) var<storage, read> b_quads: array<Quad>;

struct SolidQuadVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) clip_distances: vec4<f32>,
    @location(2) @interpolate(flat) content_mask_bounds: vec4<f32>,
    @location(3) @interpolate(flat) content_mask_radii: vec4<f32>,
}

@vertex
fn vs_solid_quad(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> SolidQuadVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let quad = b_quads[instance_id];
    let animation = resolve_visual_animation(quad.animation_slot, quad.bounds);
    let bounds = animation_bounds(quad.bounds, animation);
    let content_mask = animation_content_mask(quad.content_mask, animation);

    var out = SolidQuadVarying();
    out.position = to_device_position(unit_vertex, bounds);
    out.color = rgba_to_vec4(quad.background.solid);
    out.color.a *= animation.opacity;
    out.clip_distances = distance_from_clip_rect(unit_vertex, bounds, content_mask.bounds);
    out.content_mask_bounds = vec4<f32>(content_mask.corner_bounds.origin, content_mask.corner_bounds.size);
    out.content_mask_radii = vec4<f32>(content_mask.corner_radii.top_left, content_mask.corner_radii.top_right, content_mask.corner_radii.bottom_right, content_mask.corner_radii.bottom_left);
    return out;
}

@fragment
fn fs_solid_quad(input: SolidQuadVarying) -> @location(0) vec4<f32> {
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

    return blend_color(input.color, clip_coverage);
}
