// Shadow rendering helpers and entry points.
fn gaussian_weight(x: f32, gaussian_scale: f32, gaussian_exponent_scale: f32) -> f32 {
    return exp(-(x * x) * gaussian_exponent_scale) * gaussian_scale;
}

// This approximates the error function, needed for the gaussian integral
fn erf(v: vec2<f32>) -> vec2<f32> {
    let s = sign(v);
    let a = abs(v);
    let r1 = 1.0 + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
    let r2 = r1 * r1;
    return s - s / (r2 * r2);
}

fn blur_along_x(x: f32, y: f32, inverse_sigma: f32, corner: f32, half_size: vec2<f32>) -> f32 {
    let delta = min(half_size.y - corner - abs(y), 0.0);
    let curved = half_size.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let integral = 0.5 + 0.5 * erf((x + vec2<f32>(-curved, curved)) * (sqrt(0.5) * inverse_sigma));
    return integral.y - integral.x;
}

// --- shadows --- //

struct Shadow {
    order: u32,
    blur_radius: f32,
    bounds: Bounds,
    corner_radii: Corners,
    content_mask: ContentMask,
    color: Hsla,
}
@group(0) @binding(2) var<storage, read> b_shadows: array<Shadow>;

struct ShadowVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) @interpolate(flat) blur_radius: f32,
    @location(2) @interpolate(flat) bounds: vec4<f32>,
    @location(3) @interpolate(flat) corner_radii: vec4<f32>,
    // TODO: use `clip_distance` once Naga supports it.
    @location(4) clip_distances: vec4<f32>,
    @location(5) @interpolate(flat) content_mask_bounds: vec4<f32>,
    @location(6) @interpolate(flat) content_mask_radii: vec4<f32>,
}

@vertex
fn vs_shadow(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> ShadowVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    var shadow = b_shadows[instance_id];
    let shadow_bounds = shadow.bounds;

    let margin = 3.0 * shadow.blur_radius;
    // Set the bounds of the shadow and adjust its size based on the shadow's
    // spread radius to achieve the spreading effect
    shadow.bounds.origin -= vec2<f32>(margin);
    shadow.bounds.size += 2.0 * vec2<f32>(margin);

    var out = ShadowVarying();
    out.position = to_device_position(unit_vertex, shadow.bounds);
    out.color = hsla_to_rgba(shadow.color);
    out.blur_radius = shadow.blur_radius;
    out.bounds = vec4<f32>(shadow_bounds.origin, shadow_bounds.size);
    out.corner_radii = vec4<f32>(
        shadow.corner_radii.top_left,
        shadow.corner_radii.top_right,
        shadow.corner_radii.bottom_right,
        shadow.corner_radii.bottom_left,
    );
    out.clip_distances = distance_from_clip_rect(unit_vertex, shadow.bounds, shadow.content_mask.bounds);
    out.content_mask_bounds = vec4<f32>(shadow.content_mask.corner_bounds.origin, shadow.content_mask.corner_bounds.size);
    out.content_mask_radii = vec4<f32>(shadow.content_mask.corner_radii.top_left, shadow.content_mask.corner_radii.top_right, shadow.content_mask.corner_radii.bottom_right, shadow.content_mask.corner_radii.bottom_left);
    return out;
}

@fragment
fn fs_shadow(input: ShadowVarying) -> @location(0) vec4<f32> {
    // Alpha clip first, since we don't have `clip_distance`.
    let clip_coverage = content_mask_coverage_from_packed(input.position.xy, input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }
    if (clip_coverage <= 0.0) {
        return vec4<f32>(0.0);
    }
    if (input.color.a <= 0.0) {
        return vec4<f32>(0.0);
    }

    if (input.blur_radius <= SHADER_EPSILON) {
        return vec4<f32>(0.0);
    }

    let half_size = input.bounds.zw / 2.0;
    let center = input.bounds.xy + half_size;
    let center_to_point = input.position.xy - center;

    let corner_radius = pick_corner_radius_from_packed(center_to_point, input.corner_radii);

    // The signal is only non-zero in a limited range, so don't waste samples
    let low = center_to_point.y - half_size.y;
    let high = center_to_point.y + half_size.y;
    let start = clamp(-3.0 * input.blur_radius, low, high);
    let end = clamp(3.0 * input.blur_radius, low, high);
    if (end <= start) {
        return vec4<f32>(0.0);
    }

    let inverse_sigma = 1.0 / input.blur_radius;
    let gaussian_scale = inverse_sigma / sqrt(2.0 * M_PI_F);
    let gaussian_exponent_scale = 0.5 * inverse_sigma * inverse_sigma;

    // Accumulate samples (we can get away with surprisingly few samples)
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var alpha = 0.0;
    for (var i = 0; i < 4; i += 1) {
        let blur = blur_along_x(center_to_point.x, center_to_point.y - y,
            inverse_sigma, corner_radius, half_size);
        alpha +=  blur * gaussian_weight(y, gaussian_scale, gaussian_exponent_scale) * step;
        y += step;
    }

    return blend_color(input.color, alpha * clip_coverage);
}
