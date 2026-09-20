// Shared GPU Gaussian filter and filtered-layer compositor.
//
// Small kernels use adjacent texels: four bilinear pairs on each side plus the center (9 fetches).
// Wider kernels expand each pair back into two logical taps (17 fetches). Hardware linear
// filtering cannot correctly merge taps that are several source pixels apart.

struct BackdropBlurPass {
    offsets: vec4<f32>,
    weights: vec4<f32>,
    // x = center weight, y = adjacent-tap flag, z = bitcast(animation_slot + 1), w = reserved.
    center_and_pad: vec4<f32>,
}

struct BackdropBlur {
    order: u32,
    downsample: u32,
    levels: u32,
    pad0: u32,
    // Root backdrop records use this as both source and display geometry. Element/composite records
    // keep this slot as the immutable source/filter geometry so renderer target planning remains
    // independent from visual animation.
    bounds: Bounds,
    content_mask: ContentMask,
    corner_radii: Corners,
    // Shared 16-byte auxiliary slot. Root backdrop records store HSLA tint here; element/composite
    // records store animated display bounds as (x, y, width, height) without changing the 136-byte ABI.
    tint: Hsla,
    radius: f32,
    saturation: f32,
    blurred_size: vec2<f32>,
    opacity: f32,
    composite_kind: u32,
}

@group(0) @binding(15) var<storage, read> b_backdrop_blur_passes: array<BackdropBlurPass>;
@group(0) @binding(16) var<storage, read> b_backdrop_blurs: array<BackdropBlur>;

struct GaussianKernel {
    offsets: vec4<f32>,
    weights: vec4<f32>,
    center_weight: f32,
    adjacent_taps: f32,
}

struct BackdropBlurPassVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) texture_coords: vec2<f32>,
    @location(1) @interpolate(flat) offsets: vec4<f32>,
    @location(2) @interpolate(flat) weights: vec4<f32>,
    @location(3) @interpolate(flat) center_weight: f32,
    @location(4) @interpolate(flat) adjacent_taps: f32,
    @location(5) @interpolate(flat) instance_id: u32,
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
    @location(9) local_position: vec2<f32>,
}

const GAUSSIAN_PAIR0_CENTROID_IN_TAPS: f32 = 1.4474603;
const GAUSSIAN_PAIR_RATIOS: array<f32, 4> = array<f32, 4>(
    0.8098247,
    0.6112877,
    0.4614242,
    0.3483013,
);
const MIN_BLUR_SIGMA: f32 = 1.0 / 4096.0;

fn gaussian_weight(distance: f32, sigma: f32, support: f32, adjacent_taps: bool) -> f32 {
    if (adjacent_taps && distance > support) {
        return 0.0;
    }
    return exp(-(distance * distance) / max(2.0 * sigma * sigma, 1e-8));
}

fn build_gaussian_kernel(radius: f32) -> GaussianKernel {
    let sigma = max(abs(radius), MIN_BLUR_SIGMA);
    let support = 3.0 * sigma;
    let adjacent_taps = support <= 8.0;
    let tap_step = select(support / 8.0, 1.0, adjacent_taps);

    var offsets = vec4<f32>(0.0);
    var pair_weights = vec4<f32>(0.0);
    let center_weight = 1.0;
    var weight_sum = center_weight;
    for (var pair: u32 = 0u; pair < 4u; pair = pair + 1u) {
        let tap0 = f32(pair * 2u + 1u) * tap_step;
        let tap1 = f32(pair * 2u + 2u) * tap_step;
        let weight0 = gaussian_weight(tap0, sigma, support, adjacent_taps);
        let weight1 = gaussian_weight(tap1, sigma, support, adjacent_taps);
        let pair_weight = weight0 + weight1;
        offsets[pair] = select(
            tap0,
            (tap0 * weight0 + tap1 * weight1) / max(pair_weight, 1e-8),
            pair_weight > 1e-8,
        );
        pair_weights[pair] = pair_weight;
        weight_sum += pair_weight * 2.0;
    }

    let normalization = 1.0 / max(weight_sum, 1e-8);
    var kernel: GaussianKernel;
    kernel.offsets = offsets;
    kernel.weights = pair_weights * normalization;
    kernel.center_weight = center_weight * normalization;
    kernel.adjacent_taps = select(0.0, 1.0, adjacent_taps);
    return kernel;
}

fn resolve_blur_pass_kernel(blur_pass: BackdropBlurPass) -> GaussianKernel {
    var kernel: GaussianKernel;
    kernel.offsets = blur_pass.offsets;
    kernel.weights = blur_pass.weights;
    kernel.center_weight = blur_pass.center_and_pad.x;
    kernel.adjacent_taps = blur_pass.center_and_pad.y;

    let animation_slot_plus_one = bitcast<u32>(blur_pass.center_and_pad.z);
    if (animation_slot_plus_one == 0u) {
        return kernel;
    }
    let value = b_animation_values[animation_slot_plus_one - 1u];
    if (value.enabled == 0u || value.property != 6u) {
        return kernel;
    }
    let sampled = value.from_value + (value.to_value - value.from_value) * value.progress;
    // The retained layer was captured with max(from, to). Clamp overshooting easing so the
    // animated kernel can never grow beyond that fixed sampling footprint.
    let from_radius = max(value.from_value.x, 0.0);
    let to_radius = max(value.to_value.x, 0.0);
    let min_radius = min(from_radius, to_radius);
    let max_radius = max(from_radius, to_radius);
    return build_gaussian_kernel(clamp(sampled.x, min_radius, max_radius));
}

@vertex
fn vs_backdrop_blur_pass(
    @builtin(vertex_index) vertex_id: u32,
    @builtin(instance_index) instance_id: u32,
) -> BackdropBlurPassVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let blur_pass = b_backdrop_blur_passes[instance_id];
    let kernel = resolve_blur_pass_kernel(blur_pass);
    var out = BackdropBlurPassVarying();
    out.position = vec4<f32>(
        unit_vertex * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0),
        0.0,
        1.0,
    );
    out.texture_coords = unit_vertex;
    out.offsets = kernel.offsets;
    out.weights = kernel.weights;
    out.center_weight = kernel.center_weight;
    out.adjacent_taps = kernel.adjacent_taps;
    out.instance_id = instance_id;
    return out;
}

fn sample_backdrop_blur_texture(texture_coords: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(t_sprite, s_sprite, texture_coords, 0.0);
}

fn gaussian_blur(input: BackdropBlurPassVarying) -> vec4<f32> {
    let source_size = max(vec2<f32>(textureDimensions(t_sprite, 0)), vec2<f32>(1.0));
    let horizontal = (input.instance_id & 1u) == 0u;
    let axis = select(vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), horizontal);
    let texel_axis = axis / source_size;
    let tap_step = input.offsets.x / GAUSSIAN_PAIR0_CENTROID_IN_TAPS;

    var color = sample_backdrop_blur_texture(input.texture_coords) * input.center_weight;
    if (input.adjacent_taps != 0.0) {
        for (var pair: u32 = 0u; pair < 4u; pair = pair + 1u) {
            let weight = input.weights[pair];
            if (weight > 0.0) {
                let delta = texel_axis * input.offsets[pair];
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
        let pair_weight = input.weights[pair];
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
    let source_position = blur.bounds.origin + unit_vertex * blur.bounds.size;
    let composite_kind = blur.composite_kind & 3u;
    let animation_slot = blur.composite_kind >> 2u;
    let is_element_composite = composite_kind != 0u;
    let is_rotated_composite = composite_kind == 2u;
    var display_origin = blur.bounds.origin;
    var display_size = blur.bounds.size;
    if (is_element_composite) {
        display_origin = vec2<f32>(blur.tint.h, blur.tint.s);
        display_size = vec2<f32>(blur.tint.l, blur.tint.a);
    }
    var display_bounds = Bounds(display_origin, display_size);
    var content_mask = blur.content_mask;
    var animation_opacity = 1.0;
    if (is_element_composite && !is_rotated_composite) {
        let animation = resolve_visual_animation(animation_slot, display_bounds);
        display_bounds = animation_bounds(display_bounds, animation);
        content_mask = animation_content_mask(content_mask, animation);
        animation_opacity = animation.opacity;
        display_origin = display_bounds.origin;
        display_size = display_bounds.size;
    }
    let local_display_position = display_origin + unit_vertex * display_size;
    var display_position = local_display_position;
    if (is_rotated_composite) {
        let angle = blur.corner_radii.top_left;
        let pivot = vec2<f32>(blur.corner_radii.top_right, blur.corner_radii.bottom_right);
        let delta = local_display_position - pivot;
        let sine = sin(angle);
        let cosine = cos(angle);
        display_position = pivot + vec2<f32>(
            delta.x * cosine - delta.y * sine,
            delta.x * sine + delta.y * cosine,
        );
    }

    var out = BackdropBlurVarying();
    out.position = to_device_position_impl(display_position);
    out.local_position = local_display_position;
    out.texture_coords = source_position / max(blur.blurred_size, vec2<f32>(1.0));
    out.clip_distances = distance_from_clip_rect_impl(local_display_position, content_mask.bounds);
    out.content_mask_bounds = vec4<f32>(
        content_mask.corner_bounds.origin,
        content_mask.corner_bounds.size,
    );
    out.content_mask_radii = vec4<f32>(
        content_mask.corner_radii.top_left,
        content_mask.corner_radii.top_right,
        content_mask.corner_radii.bottom_right,
        content_mask.corner_radii.bottom_left,
    );
    out.bounds = vec4<f32>(display_origin, display_size);
    let packed_corner_radii = vec4<f32>(
        blur.corner_radii.top_left,
        blur.corner_radii.top_right,
        blur.corner_radii.bottom_right,
        blur.corner_radii.bottom_left,
    );
    out.corner_radii = select(packed_corner_radii, vec4<f32>(0.0), is_rotated_composite);
    out.saturation = blur.saturation;
    out.opacity = blur.opacity * animation_opacity;
    out.tint = select(hsla_to_rgba(blur.tint), vec4<f32>(0.0), is_element_composite);
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
        input.local_position,
        input.content_mask_bounds,
        input.content_mask_radii,
    );
    if (any(input.clip_distances < vec4<f32>(0.0)) || clip_coverage <= 0.0) {
        return vec4<f32>(0.0);
    }
    let distance = quad_sdf_from_packed(input.local_position, input.bounds, input.corner_radii);
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
