// Screen-space bridge between independently submitted 3D offscreen
// rendering and GPUI's painter-ordered GUI composition. The 3D texture
// stores premultiplied alpha; the UI blend mode preserves existing widgets.
@group(0) @binding(0) var scene_color: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
// Region is normalized to the host window's NDC. The producer texture
// contains only the cropped SceneView instead of a full-window attachment.
@group(0) @binding(2) var<uniform> placement: vec4<f32>;

struct SidecarVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_sidecar_composite(@builtin(vertex_index) index: u32) -> SidecarVertex {
    let point = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var output: SidecarVertex;
    output.position = vec4<f32>(placement.xy + point * placement.zw, 0.0, 1.0);
    output.uv = point;
    return output;
}

@fragment
fn fs_sidecar_composite(input: SidecarVertex) -> @location(0) vec4<f32> {
    return textureSample(scene_color, scene_sampler, input.uv);
}
