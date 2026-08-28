// Shared GPU Gaussian filter and filtered-layer compositor.
//
// Small kernels use adjacent texels: four bilinear pairs on each side plus the center (9 fetches).
// Wider kernels expand each pair back into two logical taps (17 fetches). Hardware linear
// filtering cannot correctly merge taps that are several source pixels apart.

struct BackdropBlurPass {
    offsets: vec4<f32>,
    weights: vec4<f32>,
    center_and_pad: vec4<f32>,
}

struct BackdropBlur {
    order: u32,
    downsample: u32,
    levels: u32,
    pad0: u32,
    bounds: Bounds,
    content_mask: ContentMask,
    corner_radii: Corners,
    tint: Hsla,
    radius: f32,
    saturation: f32,
    blurred_size: vec2<f32>,
    opacity: f32,
    pad: u32,
}

@group(0) @binding(15) var<storage, read> b_backdrop_blur_passes: array<BackdropBlurPass>;
@group(0) @binding(16) var<storage, read> b_backdrop_blurs: array<BackdropBlur>;

struct BackdropBlurPassVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) texture_coords: vec2<f32>,
    @location(1) @interpolate(flat) instance_id: u32,
}

struct BackdropBlurVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) texture_coords: vec2<f32>,
    @location(1) clip_distances: vec4<f32>,
    @location(2) @interpolate(flat) bounds: vec4<f32>,
    @location(3) @interpolate(flat) corner_radii: vec4<f32>,
    @location(4) @interpolate(flat) saturation: f32,
    @location(5) @interpolate(flat) tint: vec4<f32>,
    @location(6) @interpolate(flat) content_mask_bounds: vec4<f32>,
    @location(7) @interpolate(flat) content_mask_radii: vec4<f32>,
    @location(8) @interpolate(flat) opacity: f32,
}

// For wide kernels write_backdrop_blur_pass() uses sigma = radius and tap_step = 3 * sigma / 8. The
// Gaussian ratio between tap n+1 and n depends only on n, not on radius. The first packed pair
// centroid is likewise a fixed multiple of tap_step. Keeping those constants here lets us recover
// the exact 17 logical taps without per-fragment exp().
const GAUSSIAN_PAIR0_CENTROID_IN_TAPS: f32 = 1.4474603;
const GAUSSIAN_PAIR_RATIOS: array<f32, 4> = array<f32, 4>(
    0.8098247,
    0.6112877,
    0.4614242,
    0.3483013,
);

@vertex
fn vs_backdrop_blur_pass(
    @builtin(vertex_index) vertex_id: u32,
    @builtin(instance_index) instance_id: u32,
) -> BackdropBlurPassVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    var out = BackdropBlurPassVarying();
    out.position = vec4<f32>(
        unit_vertex * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0),
        0.0,
        1.0,
    );
    out.texture_coords = unit_vertex;
    out.instance_id = instance_id;
    return out;
}

fn sample_backdrop_blur_texture(texture_coords: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(t_sprite, s_sprite, texture_coords, 0.0);
}

fn gaussian_blur(input: BackdropBlurPassVarying) -> vec4<f32> {
    let blur_pass = b_backdrop_blur_passes[input.instance_id];
    let source_size = max(vec2<f32>(textureDimensions(t_sprite, 0)), vec2<f32>(1.0));
    let horizontal = (input.instance_id & 1u) == 0u;
    let axis = select(vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), horizontal);
    let texel_axis = axis / source_size;
    let tap_step = blur_pass.offsets.x / GAUSSIAN_PAIR0_CENTROID_IN_TAPS;

    var color = sample_backdrop_blur_texture(input.texture_coords) * blur_pass.center_and_pad.x;
    if (blur_pass.center_and_pad.y != 0.0) {
        for (var pair: u32 = 0u; pair < 4u; pair = pair + 1u) {
            let weight = blur_pass.weights[pair];
            if (weight > 0.0) {
                let delta = texel_axis * blur_pass.offsets[pair];
                color += sample_backdrop_blur_texture(input.texture_coords + delta) * weight;
                color += sample_backdrop_blur_texture(input.texture_coords - delta) * weight;
            }
        }
        return color;
    }
    for (var pair: u32 = 0u; pair < 4u; pair = pair + 1u) {
        let first_tap = f32(pair * 2u + 1u);
        let second_tap = first_tap + 1.0;
        let ratio = GAUSSIAN_PAIR_RATIOS[pair];
        let pair_weight = blur_pass.weights[pair];
        let first_weight = pair_weight / (1.0 + ratio);
        let second_weight = pair_weight - first_weight;
        let first_delta = texel_axis * (tap_step * first_tap);
        let second_delta = texel_axis * (tap_step * second_tap);

        color += sample_backdrop_blur_texture(input.texture_coords + first_delta) * first_weight;
        color += sample_backdrop_blur_texture(input.texture_coords - first_delta) * first_weight;
        color += sample_backdrop_blur_texture(input.texture_coords + second_delta) * second_weight;
        color += sample_backdrop_blur_texture(input.texture_coords - second_delta) * second_weight;
    }
    return color;
}

// Entry names stay stable for the backend-neutral pipeline table. The first pass blurs X while the
// target planner downsamples X only; the second pass blurs Y while downsampling Y to final size.
@fragment
fn fs_backdrop_blur_downsample(input: BackdropBlurPassVarying) -> @location(0) vec4<f32> {
    return gaussian_blur(input);
}

@fragment
fn fs_backdrop_blur_upsample(input: BackdropBlurPassVarying) -> @location(0) vec4<f32> {
    return gaussian_blur(input);
}

@vertex
fn vs_backdrop_blur(
    @builtin(vertex_index) vertex_id: u32,
    @builtin(instance_index) instance_id: u32,
) -> BackdropBlurVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let blur = b_backdrop_blurs[instance_id];
    let screen_position = blur.bounds.origin + unit_vertex * blur.bounds.size;
    var out = BackdropBlurVarying();
    out.position = to_device_position(unit_vertex, blur.bounds);
    out.texture_coords = screen_position / max(blur.blurred_size, vec2<f32>(1.0));
    out.clip_distances = distance_from_clip_rect(unit_vertex, blur.bounds, blur.content_mask.bounds);
    out.content_mask_bounds = vec4<f32>(
        blur.content_mask.corner_bounds.origin,
        blur.content_mask.corner_bounds.size,
    );
    out.content_mask_radii = vec4<f32>(
        blur.content_mask.corner_radii.top_left,
        blur.content_mask.corner_radii.top_right,
        blur.content_mask.corner_radii.bottom_right,
        blur.content_mask.corner_radii.bottom_left,
    );
    out.bounds = vec4<f32>(blur.bounds.origin, blur.bounds.size);
    out.corner_radii = vec4<f32>(
        blur.corner_radii.top_left,
        blur.corner_radii.top_right,
        blur.corner_radii.bottom_right,
        blur.corner_radii.bottom_left,
    );
    out.saturation = blur.saturation;
    out.opacity = blur.opacity;
    out.tint = hsla_to_rgba(blur.tint);
    return out;
}

fn saturate_color(color: vec3<f32>, saturation: f32) -> vec3<f32> {
    let luminance = dot(color, GRAYSCALE_FACTORS);
    return mix(vec3<f32>(luminance), color, max(saturation, 0.0));
}

@fragment
fn fs_backdrop_blur(input: BackdropBlurVarying) -> @location(0) vec4<f32> {
    if (input.opacity <= 0.0) {
        return vec4<f32>(0.0);
    }
    let clip_coverage = content_mask_coverage_from_packed(
        input.position.xy,
        input.content_mask_bounds,
        input.content_mask_radii,
    );
    if (any(input.clip_distances < vec4<f32>(0.0)) || clip_coverage <= 0.0) {
        return vec4<f32>(0.0);
    }
    let distance = quad_sdf_from_packed(input.position.xy, input.bounds, input.corner_radii);
    let alpha = saturate(SDF_ANTIALIAS_THRESHOLD - distance);
    if (alpha <= 0.0) {
        return vec4<f32>(0.0);
    }

    var color = sample_backdrop_blur_texture(input.texture_coords);
    if (color.a <= 0.0 && input.tint.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    // Render targets contain premultiplied color, regardless of the swapchain output convention.
    // Filter premultiplied RGBA together, then recover straight color only for tint/compositing.
    // Multiplying a filtered premultiplied color by alpha again produces dark transparent edges.
    color = vec4<f32>(color.rgb / max(color.a, SHADER_EPSILON), color.a);
    if (input.saturation != 1.0) {
        color = vec4<f32>(saturate_color(color.rgb, input.saturation), color.a);
    }
    if (input.tint.a > 0.0) {
        color = over(color, input.tint);
    }
    return blend_color(color, alpha * clip_coverage * input.opacity);
}
