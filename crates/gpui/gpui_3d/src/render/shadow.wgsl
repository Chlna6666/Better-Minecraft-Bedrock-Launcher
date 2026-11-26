// Shadow-map depth pass for the offscreen scene renderer.
//
// The shadow target is an RGBA8 color texture (nova-gfx's phase-1 format set has no
// float render targets); fragment depth is packed into 24 bits across RGB so the
// comparison keeps sub-texel precision.
//
// Bind group 0:
//   0 - packed vertex storage buffer
//   1 - instance storage buffer
//   2 - frame uniform buffer (view-projection at offset 0, light view-projection at 64)

struct PackedVertex {
    position: vec3<f32>,
    barycentric_u: f32,
    normal: vec3<f32>,
    barycentric_v: f32,
    uv: vec2<f32>,
    barycentric_w: f32,
    edge_mask: f32,
    color: vec4<f32>,
};

struct Instance {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    offset: vec4<f32>,
};

struct ShadowFrame {
    view_projection: mat4x4<f32>,
    light_view_projection: mat4x4<f32>,
};

@group(0) @binding(0) var<storage, read> vertices: array<PackedVertex>;
@group(0) @binding(1) var<storage, read> instances: array<Instance>;
@group(0) @binding(2) var<uniform> frame: ShadowFrame;

fn pack_depth(value: f32) -> vec4<f32> {
    let scaled = clamp(value, 0.0, 1.0) * 16777215.0;
    let high = floor(scaled / 65536.0);
    let mid = floor((scaled - high * 65536.0) / 256.0);
    let low = scaled - high * 65536.0 - mid * 256.0;
    return vec4<f32>(high, mid, low, 1.0) / 255.0;
}

@vertex
fn vs_shadow_depth(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> @builtin(position) vec4<f32> {
    let vertex = vertices[vertex_index];
    let model = instances[instance_index].model;
    return frame.light_view_projection * (model * vec4<f32>(vertex.position, 1.0));
}

@fragment
fn fs_shadow_depth(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return pack_depth(position.z);
}
