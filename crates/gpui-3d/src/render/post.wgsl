// HDR resolve for the offscreen gpui-3d renderer.
//
// Reads the linear RGBA16Float scene target, applies exposure, tone mapping, FXAA-style edge
// filtering, vignette, film grain, ordered dithering, and gamma encoding into the RGBA8 post
// target that is read back to the CPU.
//
// Bind group 0:
//   0 scene color (scene target, RGBA16Float), 1 sampler, 2 frame uniform

struct Frame {
    view_projection: mat4x4<f32>,
    light_view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
    light_direction: vec4<f32>,
    light_color: vec4<f32>,
    ambient: vec4<f32>,
    // exposure, tonemap mode, debug view, shadow strength
    post: vec4<f32>,
    // shadow texel x, shadow texel y, depth bias, shadow enabled
    shadow: vec4<f32>,
    // target width, target height, normal bias, post enabled
    misc: vec4<f32>,
    // bloom threshold, bloom strength, shadow softness, FXAA enabled
    effects: vec4<f32>,
    // vignette, film grain, dither, debug-view bypass
    effects2: vec4<f32>,
    // bloom radius, bloom threshold duplicate, unused, unused
    effects3: vec4<f32>,
    inverse_view_projection: mat4x4<f32>,
    // intensity, rotation radians, environment mip count, enabled
    environment: vec4<f32>,
};

@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var<uniform> frame: Frame;
@group(0) @binding(3) var bloom_texture: texture_2d<f32>;

const TONEMAP_NONE: u32 = 0u;
const TONEMAP_REINHARD: u32 = 1u;
const TONEMAP_ACES: u32 = 2u;

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

fn tonemap_aces(color: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((color * (a * color + vec3<f32>(b))) / (color * (c * color + vec3<f32>(d)) + vec3<f32>(e)), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn tonemap(color: vec3<f32>, mode: u32) -> vec3<f32> {
    if (mode == TONEMAP_REINHARD) {
        return color / (color + vec3<f32>(1.0));
    }
    if (mode == TONEMAP_ACES) {
        return tonemap_aces(color);
    }
    return clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn hash(value: vec2<f32>) -> f32 {
    return fract(sin(dot(value, vec2<f32>(12.9898, 78.233))) * 43758.546875);
}

fn bayer4(pixel: vec2<f32>) -> f32 {
    let x = u32(pixel.x) & 3u;
    let y = u32(pixel.y) & 3u;
    let index = x + y * 4u;
    let values = array<f32, 16>(
        0.0, 8.0, 2.0, 10.0,
        12.0, 4.0, 14.0, 6.0,
        3.0, 11.0, 1.0, 9.0,
        15.0, 7.0, 13.0, 5.0,
    );
    return values[index] / 16.0;
}

fn resolve_linear(scene_color: vec3<f32>) -> vec3<f32> {
    let exposed = max(scene_color, vec3<f32>(0.0)) * frame.post.x;
    let mapped = tonemap(exposed, u32(round(frame.post.y)));
    return pow(max(mapped, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}

fn luma(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn output_texel() -> vec2<f32> {
    return vec2<f32>(1.0, 1.0) / max(vec2<f32>(frame.misc.x, frame.misc.y), vec2<f32>(1.0));
}

fn sample_scene(uv: vec2<f32>) -> vec4<f32> {
    // Render scale 1 samples the HDR target directly. Other scales take four taps across the
    // output-pixel footprint so supersampled frames downsample and low-resolution frames upscale
    // without aliasing the scene texture.
    let scale_ratio = frame.effects3.z / max(frame.misc.x, 1.0);
    if (abs(scale_ratio - 1.0) <= 0.001) {
        return textureSample(scene_texture, scene_sampler, uv);
    }
    let offset = output_texel() * 0.25;
    let top_left = textureSample(scene_texture, scene_sampler, uv + vec2<f32>(-offset.x, -offset.y));
    let top_right = textureSample(scene_texture, scene_sampler, uv + vec2<f32>(offset.x, -offset.y));
    let bottom_left = textureSample(scene_texture, scene_sampler, uv + vec2<f32>(-offset.x, offset.y));
    let bottom_right = textureSample(scene_texture, scene_sampler, uv + vec2<f32>(offset.x, offset.y));
    return (top_left + top_right + bottom_left + bottom_right) * 0.25;
}

@fragment
fn fs_post(input: PostOutput) -> @location(0) vec4<f32> {
    // Debug views and object ids are already display data; never grade or filter them, and read
    // exactly one scene texel so object ids still decode when the scene is supersampled.
    if (frame.misc.w < 0.5 || frame.effects2.w > 0.5) {
        let direct_scene = textureSample(scene_texture, scene_sampler, input.uv);
        return vec4<f32>(clamp(direct_scene.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    }
    let raw_scene = sample_scene(input.uv);

    // Bloom is accumulated in linear HDR before exposure/tonemapping so highlights keep their head.
    let bloom_sample = textureSample(bloom_texture, scene_sampler, input.uv);
    let bloom_add = bloom_sample.rgb * frame.effects.y;
    var color = resolve_linear(raw_scene.rgb + bloom_add);
    if (frame.effects.w > 0.5) {
        // FXAA-style edge blend: compare luma of the four nearest vertical/horizontal samples and
        // blend the ones on the opposite side of a local edge. The scene target is HDR, so each
        // tap is exposed and tone mapped before comparison.
        let texel = output_texel();
        let sample_left = resolve_linear(
            sample_scene(input.uv - vec2<f32>(texel.x, 0.0)).rgb,
        );
        let sample_right = resolve_linear(
            sample_scene(input.uv + vec2<f32>(texel.x, 0.0)).rgb,
        );
        let sample_down = resolve_linear(
            sample_scene(input.uv - vec2<f32>(0.0, texel.y)).rgb,
        );
        let sample_up = resolve_linear(
            sample_scene(input.uv + vec2<f32>(0.0, texel.y)).rgb,
        );
        let center_luma = luma(color);
        let contrast = max(
            abs(luma(sample_left) - luma(sample_right)),
            abs(luma(sample_down) - luma(sample_up)),
        );
        let blend = clamp(contrast * 8.0, 0.0, 0.6);
        let horizontal = (sample_left + sample_right) * 0.5;
        let vertical = (sample_down + sample_up) * 0.5;
        let neighborhood = mix(horizontal, vertical, step(vertical.g, horizontal.g));
        color = mix(color, neighborhood, blend);
    }

    let centered = input.uv - vec2<f32>(0.5);
    let radius = length(centered) * 1.41421356;
    let vignette = 1.0 - smoothstep(0.55, 1.05, radius) * 0.55 * frame.effects2.x;
    color *= vignette;
    let pixel = input.position.xy;
    let grain = (hash(pixel) - 0.5) * frame.effects2.y * 0.35;
    color += vec3<f32>(grain);
    let dither = (bayer4(pixel) - 0.5) * frame.effects2.z / 255.0;
    color += vec3<f32>(dither);
    return vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
