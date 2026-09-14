// --- underlines --- //

struct UnderlineAnimationValue {
    animation_id: u32,
    property: u32,
    progress: f32,
    enabled: u32,
    from_value: vec4<f32>,
    to_value: vec4<f32>,
    pad: vec4<u32>,
}
@group(0) @binding(17) var<storage, read> b_underline_animation_values: array<UnderlineAnimationValue>;

struct UnderlineVisualAnimation {
    translation: vec2<f32>,
    origin: vec2<f32>,
    scale: f32,
    opacity: f32,
    scales_geometry: u32,
}

fn resolve_underline_visual_animation(slot_plus_one: u32, bounds: Bounds) -> UnderlineVisualAnimation {
    var animation: UnderlineVisualAnimation;
    animation.translation = vec2<f32>(0.0);
    animation.origin = bounds.origin + bounds.size * 0.5;
    animation.scale = 1.0;
    animation.opacity = 1.0;
    animation.scales_geometry = 0u;

    if (globals.pad == 0u || slot_plus_one == 0u) {
        return animation;
    }

    let value = b_underline_animation_values[slot_plus_one - 1u];
    if (value.enabled == 0u) {
        return animation;
    }
    let sampled = value.from_value + (value.to_value - value.from_value) * value.progress;
    switch value.property {
        // Opacity
        case 0u: {
            animation.opacity = clamp(sampled.x, 0.0, 1.0);
        }
        // Transform: scale + opacity around a shared explicit origin.
        case 1u: {
            animation.scale = max(sampled.x, 0.0);
            animation.opacity = clamp(sampled.y, 0.0, 1.0);
            animation.origin = sampled.zw;
            animation.scales_geometry = 1u;
        }
        // Translation keeps the clip mask fixed in screen space.
        case 2u: {
            animation.translation = sampled.xy;
        }
        // Scale around the primitive's static center.
        case 3u: {
            animation.scale = max(sampled.x, 0.0);
            animation.scales_geometry = 1u;
        }
        default: {}
    }
    return animation;
}

fn underline_animation_bounds(bounds: Bounds, animation: UnderlineVisualAnimation) -> Bounds {
    var result = bounds;
    if (animation.scales_geometry != 0u) {
        result.origin = animation.origin + (bounds.origin - animation.origin) * animation.scale;
        result.size = bounds.size * animation.scale;
    }
    result.origin += animation.translation;
    return result;
}

fn underline_animation_content_mask(mask: ContentMask, animation: UnderlineVisualAnimation) -> ContentMask {
    var result = mask;
    if (animation.scales_geometry != 0u) {
        result.bounds.origin =
            animation.origin + (mask.bounds.origin - animation.origin) * animation.scale;
        result.bounds.size = mask.bounds.size * animation.scale;
        result.corner_bounds.origin =
            animation.origin + (mask.corner_bounds.origin - animation.origin) * animation.scale;
        result.corner_bounds.size = mask.corner_bounds.size * animation.scale;
        result.corner_radii.top_left *= animation.scale;
        result.corner_radii.top_right *= animation.scale;
        result.corner_radii.bottom_right *= animation.scale;
        result.corner_radii.bottom_left *= animation.scale;
    }
    return result;
}

struct Underline {
    order: u32,
    animation_slot: u32,
    bounds: Bounds,
    content_mask: ContentMask,
    color: Rgba,
    thickness: f32,
    wavy: u32,
}
@group(0) @binding(7) var<storage, read> b_underlines: array<Underline>;

struct UnderlineVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) @interpolate(flat) bounds: vec4<f32>,
    @location(2) @interpolate(flat) thickness: f32,
    @location(3) @interpolate(flat) wavy: u32,
    // TODO: use `clip_distance` once Naga supports it.
    @location(4) clip_distances: vec4<f32>,
    @location(5) @interpolate(flat) content_mask_bounds: vec4<f32>,
    @location(6) @interpolate(flat) content_mask_radii: vec4<f32>,
}

@vertex
fn vs_underline(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> UnderlineVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let underline = b_underlines[instance_id];
    let animation = resolve_underline_visual_animation(underline.animation_slot, underline.bounds);
    let bounds = underline_animation_bounds(underline.bounds, animation);
    let content_mask = underline_animation_content_mask(underline.content_mask, animation);

    var out = UnderlineVarying();
    out.position = to_device_position(unit_vertex, bounds);
    out.color = rgba_to_vec4(underline.color);
    out.color.a *= animation.opacity;
    out.bounds = vec4<f32>(bounds.origin, bounds.size);
    out.thickness = underline.thickness * animation.scale;
    out.wavy = underline.wavy;
    out.clip_distances = distance_from_clip_rect(unit_vertex, bounds, content_mask.bounds);
    out.content_mask_bounds = vec4<f32>(content_mask.corner_bounds.origin, content_mask.corner_bounds.size);
    out.content_mask_radii = vec4<f32>(content_mask.corner_radii.top_left, content_mask.corner_radii.top_right, content_mask.corner_radii.bottom_right, content_mask.corner_radii.bottom_left);
    return out;
}

@fragment
fn fs_underline(input: UnderlineVarying) -> @location(0) vec4<f32> {
    const WAVE_FREQUENCY: f32 = 2.0;
    const WAVE_HEIGHT_RATIO: f32 = 0.8;

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

    let underline_height = input.bounds.w;
    if (underline_height <= SHADER_EPSILON || input.thickness <= SHADER_EPSILON) {
        return vec4<f32>(0.0);
    }

    if ((input.wavy & 0xFFu) == 0u)
    {
        return blend_color(input.color, clip_coverage);
    }

    let half_thickness = input.thickness * 0.5;

    let st = (input.position.xy - input.bounds.xy) / underline_height - vec2<f32>(0.0, 0.5);
    let frequency = M_PI_F * WAVE_FREQUENCY * input.thickness / underline_height;
    let amplitude = (input.thickness * WAVE_HEIGHT_RATIO) / underline_height;

    let sine = sin(st.x * frequency) * amplitude;
    let dSine = cos(st.x * frequency) * amplitude * frequency;
    let distance = (st.y - sine) / sqrt(1.0 + dSine * dSine);
    let distance_in_pixels = distance * underline_height;
    let distance_from_top_border = distance_in_pixels - half_thickness;
    let distance_from_bottom_border = distance_in_pixels + half_thickness;
    let stroke_distance = max(-distance_from_bottom_border, distance_from_top_border);
    let alpha = saturate(SDF_ANTIALIAS_THRESHOLD - stroke_distance);
    return blend_color(input.color, alpha * clip_coverage);
}
