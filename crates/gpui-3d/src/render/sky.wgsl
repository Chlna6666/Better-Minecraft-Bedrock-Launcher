// Environment sky background for the offscreen gpui-3d renderer.
//
// Drawn as the first scene-pass step with depth comparison but no depth write, so scene geometry
// overwrites it. Samples the scene environment map along the camera ray; only the sky is shaded
// here, lighting uses the same map in the PBR shader.
//
// Bind group 0: 0 environment texture, 1 sampler, 2 frame uniform.

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

@group(0) @binding(0) var environment_texture: texture_2d<f32>;
@group(0) @binding(1) var environment_sampler: sampler;
@group(0) @binding(2) var<uniform> frame: Frame;

struct SkyOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vertex_index: u32) -> SkyOutput {
    let x = f32((vertex_index << 1u) & 2u);
    let y = f32(vertex_index & 2u);
    var output: SkyOutput;
    output.position = vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 1.0, 1.0);
    output.uv = vec2<f32>(x, 1.0 - y);
    return output;
}

fn equirect_uv(direction: vec3<f32>, rotation: f32) -> vec2<f32> {
    var rotated = direction;
    let cos_rotation = cos(-rotation);
    let sin_rotation = sin(-rotation);
    rotated.x = direction.x * cos_rotation - direction.z * sin_rotation;
    rotated.z = direction.x * sin_rotation + direction.z * cos_rotation;
    let u = atan2(rotated.x, rotated.z) * 0.15915494 + 0.5;
    let v = acos(clamp(rotated.y, -1.0, 1.0)) * 0.31830989;
    return vec2<f32>(u, v);
}

@fragment
fn fs_sky(input: SkyOutput) -> @location(0) vec4<f32> {
    let ndc = vec2<f32>(input.uv.x * 2.0 - 1.0, 1.0 - input.uv.y * 2.0);
    let far = frame.inverse_view_projection * vec4<f32>(ndc, 1.0, 1.0);
    if (abs(far.w) <= 0.00001) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let world = far.xyz / far.w;
    let direction = normalize(world - frame.camera_position.xyz);
    let uv = equirect_uv(direction, frame.environment.y);
    let radiance = textureSampleLevel(environment_texture, environment_sampler, uv, 0.0).rgb;
    return vec4<f32>(radiance * frame.environment.x, 1.0);
}
