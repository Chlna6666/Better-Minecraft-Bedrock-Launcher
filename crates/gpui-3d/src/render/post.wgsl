// Post-processing resolve for the offscreen gpui-3d renderer.
//
// Bind group 0:
//   0 scene color, 1 sampler, 2 frame uniform (shared layout with the scene pass)

struct Frame {
    view_projection: mat4x4<f32>,
    light_view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
    light_direction: vec4<f32>,
    light_color: vec4<f32>,
    ambient: vec4<f32>,
    post: vec4<f32>,
    shadow: vec4<f32>,
    misc: vec4<f32>,
};

@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var<uniform> frame: Frame;

struct PostOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_post(@builtin(vertex_index) vertex_index: u32) -> PostOutput {
    let x = f32((vertex_index << 1u) & 2u);
    let y = f32(vertex_index & 2u);
    var output: PostOutput;
    output.position = vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
    output.uv = vec2<f32>(x, 1.0 - y);
    return output;
}

@fragment
fn fs_post(input: PostOutput) -> @location(0) vec4<f32> {
    var color = textureSample(scene_texture, scene_sampler, input.uv);
    // frame.misc.w stores the post-processing quality selector: 0 disables grading,
    // 1 applies the vignette and gamma-aware dither used by the lab captures.
    let quality = frame.misc.w;
    if (quality >= 1.0) {
        let centered = input.uv - vec2<f32>(0.5);
        let radius = length(centered) * 1.41421356;
        let vignette = 1.0 - smoothstep(0.55, 1.05, radius) * 0.55;
        color = vec4<f32>(color.rgb * vignette, color.a);
    }
    return color;
}
