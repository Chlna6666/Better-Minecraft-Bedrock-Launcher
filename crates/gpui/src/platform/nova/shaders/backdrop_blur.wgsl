// --- isolated backdrop blur --- //
//
// Nova uses a separable Gaussian filter for backdrop blur. The blur radius stays in floating-point
// source pixels from the GPUI primitive through the shader, so values such as 0.1px remain valid.
//
// Each axis uses bilinear tap pairing: the equivalent 17-tap kernel is evaluated with 9 texture
// samples (center + four positive/negative pairs). This keeps the visual kernel smooth while
// substantially reducing texture bandwidth during high-refresh-rate UI animation.

struct BackdropBlurPass {
    radius: f32,
    pad0: f32,
    pad1: f32,
    pad: u32,
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
    pad: vec2<u32>,
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
}

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

fn gaussian_weight(distance: f32, sigma: f32) -> f32 {
    return exp(-(distance * distance) / max(2.0 * sigma * sigma, 1e-8));
}

fn gaussian_blur(input: BackdropBlurPassVarying) -> vec4<f32> {
    let pass = b_backdrop_blur_passes[input.instance_id];
    let radius = max(pass.radius, 1.0 / 4096.0);
    let sigma = max(radius / 3.0, 1.0 / 4096.0);
    let tap_step = radius / 8.0;
    let source_size = max(vec2<f32>(textureDimensions(t_sprite, 0)), vec2<f32>(1.0));
    let horizontal = (input.instance_id & 1u) == 0u;
    let axis = select(vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), horizontal);
    let texel_axis = axis / source_size;

    // Center tap.
    let center_weight = gaussian_weight(0.0, sigma);
    var color = sample_backdrop_blur_texture(input.texture_coords) * center_weight;
    var weight_sum = center_weight;

    // Pair adjacent Gaussian taps and let the linear sampler interpolate between them. Four
    // paired distances on each side are equivalent to the old +/-8 discrete taps but require
    // eight texture samples instead of sixteen.
    for (var pair: i32 = 0; pair < 4; pair = pair + 1) {
        let tap0 = f32(pair * 2 + 1) * tap_step;
        let tap1 = f32(pair * 2 + 2) * tap_step;
        let weight0 = gaussian_weight(tap0, sigma);
        let weight1 = gaussian_weight(tap1, sigma);
        let pair_weight = weight0 + weight1;
        let paired_distance = (tap0 * weight0 + tap1 * weight1) / max(pair_weight, 1e-8);
        let delta = texel_axis * paired_distance;

        color += sample_backdrop_blur_texture(input.texture_coords + delta) * pair_weight;
        color += sample_backdrop_blur_texture(input.texture_coords - delta) * pair_weight;
        weight_sum += pair_weight * 2.0;
    }

    return color / max(weight_sum, 1e-6);
}

// Keep the existing pipeline entry names so the gfx abstraction remains backend-neutral.
// The first instance of a configuration is horizontal and the second is vertical.
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
    out.tint = hsla_to_rgba(blur.tint);
    return out;
}

fn saturate_color(color: vec3<f32>, saturation: f32) -> vec3<f32> {
    let luminance = dot(color, GRAYSCALE_FACTORS);
    return mix(vec3<f32>(luminance), color, max(saturation, 0.0));
}

@fragment
fn fs_backdrop_blur(input: BackdropBlurVarying) -> @location(0) vec4<f32> {
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
    if (input.saturation != 1.0) {
        color = vec4<f32>(saturate_color(color.rgb, input.saturation), color.a);
    }
    if (input.tint.a > 0.0) {
        color = over(color, input.tint);
    }
    return blend_color(color, alpha * clip_coverage);
}
