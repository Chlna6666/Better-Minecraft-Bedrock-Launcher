// --- backdrop blur --- //

struct BackdropBlurPass {
    offset: f32,
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
    @location(1) @interpolate(flat) offset: f32,
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
fn vs_backdrop_blur_pass(@builtin(vertex_index) vertex_id: u32) -> BackdropBlurPassVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    var out = BackdropBlurPassVarying();
    out.position = vec4<f32>(unit_vertex * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.texture_coords = unit_vertex;
    out.offset = b_backdrop_blur_passes[0].offset;
    return out;
}

fn sample_backdrop_blur_texture(texture_coords: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(t_sprite, s_sprite, texture_coords, 0.0);
}

fn kawase_downsample(texture_coords: vec2<f32>, texel_size: vec2<f32>, offset: f32) -> vec4<f32> {
    let delta = texel_size * offset;
    var color = sample_backdrop_blur_texture(texture_coords) * 4.0;
    color += sample_backdrop_blur_texture(texture_coords + vec2<f32>(-delta.x, -delta.y));
    color += sample_backdrop_blur_texture(texture_coords + vec2<f32>(delta.x, -delta.y));
    color += sample_backdrop_blur_texture(texture_coords + vec2<f32>(-delta.x, delta.y));
    color += sample_backdrop_blur_texture(texture_coords + vec2<f32>(delta.x, delta.y));
    return color * 0.125;
}

fn kawase_upsample(texture_coords: vec2<f32>, texel_size: vec2<f32>, offset: f32) -> vec4<f32> {
    let delta = texel_size * offset;
    var color = vec4<f32>(0.0);
    color += sample_backdrop_blur_texture(texture_coords + vec2<f32>(-2.0 * delta.x, 0.0));
    color += sample_backdrop_blur_texture(texture_coords + vec2<f32>(-delta.x, delta.y)) * 2.0;
    color += sample_backdrop_blur_texture(texture_coords + vec2<f32>(0.0, 2.0 * delta.y));
    color += sample_backdrop_blur_texture(texture_coords + vec2<f32>(delta.x, delta.y)) * 2.0;
    color += sample_backdrop_blur_texture(texture_coords + vec2<f32>(2.0 * delta.x, 0.0));
    color += sample_backdrop_blur_texture(texture_coords + vec2<f32>(delta.x, -delta.y)) * 2.0;
    color += sample_backdrop_blur_texture(texture_coords + vec2<f32>(0.0, -2.0 * delta.y));
    color += sample_backdrop_blur_texture(texture_coords + vec2<f32>(-delta.x, -delta.y)) * 2.0;
    return color * 0.0833333333;
}

@fragment
fn fs_backdrop_blur_downsample(input: BackdropBlurPassVarying) -> @location(0) vec4<f32> {
    let source_size = max(vec2<f32>(textureDimensions(t_sprite, 0)), vec2<f32>(1.0));
    return kawase_downsample(input.texture_coords, 1.0 / source_size, input.offset);
}

@fragment
fn fs_backdrop_blur_upsample(input: BackdropBlurPassVarying) -> @location(0) vec4<f32> {
    let source_size = max(vec2<f32>(textureDimensions(t_sprite, 0)), vec2<f32>(1.0));
    return kawase_upsample(input.texture_coords, 1.0 / source_size, input.offset);
}

@vertex
fn vs_backdrop_blur(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> BackdropBlurVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let blur = b_backdrop_blurs[instance_id];
    let screen_position = blur.bounds.origin + unit_vertex * blur.bounds.size;
    var out = BackdropBlurVarying();
    out.position = to_device_position(unit_vertex, blur.bounds);
    out.texture_coords = screen_position / max(blur.blurred_size, vec2<f32>(1.0));
    out.clip_distances = distance_from_clip_rect(unit_vertex, blur.bounds, blur.content_mask.bounds);
    out.content_mask_bounds = vec4<f32>(blur.content_mask.corner_bounds.origin, blur.content_mask.corner_bounds.size);
    out.content_mask_radii = vec4<f32>(blur.content_mask.corner_radii.top_left, blur.content_mask.corner_radii.top_right, blur.content_mask.corner_radii.bottom_right, blur.content_mask.corner_radii.bottom_left);
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
    return mix(vec3<f32>(luminance), color, saturation);
}

@fragment
fn fs_backdrop_blur(input: BackdropBlurVarying) -> @location(0) vec4<f32> {
    let clip_coverage = content_mask_coverage_from_packed(input.position.xy, input.content_mask_bounds, input.content_mask_radii);
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }
    if (clip_coverage <= 0.0) {
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
