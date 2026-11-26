// Bloom bright-pass and blur for the offscreen gpui-3d renderer.
//
// Reads the linear HDR scene target at half resolution and writes a thresholded, blurred bright
// component into an RGBA8 target. The post pass adds it back before tone mapping, so emissive and
// specular highlights bleed without clipping the scene target.
//
// Bind group 0:
//   0 scene color (RGBA16Float), 1 sampler, 2 frame uniform

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
    effects: vec4<f32>,
    effects2: vec4<f32>,
    effects3: vec4<f32>,
    inverse_view_projection: mat4x4<f32>,
    // intensity, rotation radians, environment mip count, enabled
    environment: vec4<f32>,
};

@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var<uniform> frame: Frame;

struct BloomOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_bloom(@builtin(vertex_index) vertex_index: u32) -> BloomOutput {
    let x = f32((vertex_index << 1u) & 2u);
    let y = f32(vertex_index & 2u);
    var output: BloomOutput;
    output.position = vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
    output.uv = vec2<f32>(x, 1.0 - y);
    return output;
}

fn luminance_value(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
}

@fragment
fn fs_bloom(input: BloomOutput) -> @location(0) vec4<f32> {
    let strength = frame.effects.y;
    if (strength <= 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let threshold = max(frame.effects.x, 0.0);
    let radius = clamp(frame.effects3.x, 0.5, 4.0);
    let scene_size = max(vec2<f32>(frame.effects3.z, frame.effects3.w), vec2<f32>(1.0));
    // The bloom target is half resolution, so one bloom texel covers two scene texels.
    let texel = radius * 2.0 / scene_size;
    let offsets = array<vec2<f32>, 9>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 0.0), vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0),
    );
    var accumulated = vec3<f32>(0.0);
    var weight_sum = 0.0;
    for (var index = 0u; index < 9u; index += 1u) {
        let sample_position = input.uv + offsets[index] * texel;
        let sample_color = textureSample(scene_texture, scene_sampler, sample_position).rgb;
        let luma = luminance_value(sample_color);
        let contribution = max(luma - threshold, 0.0) / max(luma, 0.0001);
        let weight = select(0.25, 1.0, index == 4u);
        accumulated += sample_color * contribution * weight;
        weight_sum += weight;
    }
    return vec4<f32>(clamp(accumulated / max(weight_sum, 1.0), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
