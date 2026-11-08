// Screen-space bridge between independently submitted 3D offscreen
// rendering and GPUI's painter-ordered GUI composition. The 3D texture
// stores premultiplied alpha; the UI blend mode preserves existing widgets.
@group(0) @binding(0) var scene_color: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
// Region is normalized to the host window's NDC. The producer texture
// contains only the cropped SceneView instead of a full-window attachment.
// This uniform has three vec4s matching sidecar_surface::placement_bytes.
// Rounded corner masking is performed only in the GUI composite pass:
// the 3D producer may render to its rectangular cropped attachment unchanged.
struct SidecarPlacement {
    rect_ndc: vec4<f32>,
    mask_bounds: vec4<f32>,
    mask_radii: vec4<f32>,
};
@group(0) @binding(2) var<uniform> placement: SidecarPlacement;

struct SidecarVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_sidecar_composite(@builtin(vertex_index) index: u32) -> SidecarVertex {
    let point = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var output: SidecarVertex;
    output.position = vec4<f32>(placement.rect_ndc.xy + point * placement.rect_ndc.zw, 0.0, 1.0);
    output.uv = point;
    return output;
}

// Mirrors GPUI's content_mask_coverage_from_packed: select per-corner radius,
// evaluate the rounded-quad SDF in host-window pixels, and antialias at 0.5px.
// A zero-radius mask is a strict fast path, preserving the original composite.
fn rounded_mask_coverage(position: vec2<f32>) -> f32 {
    let radii = placement.mask_radii;
    if (all(radii == vec4<f32>(0.0))) {
        return 1.0;
    }
    let half_size = placement.mask_bounds.zw * 0.5;
    let centered = position - (placement.mask_bounds.xy + half_size);
    let top = centered.y < 0.0;
    let left_radius = select(radii.w, radii.x, top);
    let right_radius = select(radii.z, radii.y, top);
    let radius = select(right_radius, left_radius, centered.x < 0.0);
    let inset = abs(centered) - half_size + vec2<f32>(radius);
    let outer = length(max(inset, vec2<f32>(0.0)));
    let inner = min(0.0, max(inset.x, inset.y));
    let distance = outer + inner - radius;
    return clamp(0.5 - distance, 0.0, 1.0);
}

@fragment
fn fs_sidecar_composite(input: SidecarVertex) -> @location(0) vec4<f32> {
    let color = textureSample(scene_color, scene_sampler, input.uv);
    // This texture is PREMULTIPLIED. Multiply RGB *and* A by the same
    // coverage; changing alpha alone exposes bright/transparent fringes
    // around rounded windows and overlay clipping regions.
    return color * rounded_mask_coverage(input.position.xy);
}
