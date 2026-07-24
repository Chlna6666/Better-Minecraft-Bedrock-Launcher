// Core definitions shared by GPUI GPU shader bundles.
struct GlobalParams {
    viewport_size: vec2<f32>,
    premultiplied_alpha: u32,
    pad: u32,
}


@group(0) @binding(0) var<uniform> globals: GlobalParams;
@group(0) @binding(4) var t_sprite: texture_2d<f32>;
@group(0) @binding(5) var s_sprite: sampler;

// Clip strategy:
// Most Nova shaders pass software clip distances to the fragment stage and
// return transparent outside the clip, because hardware `clip_distance` is not
// available through the current Naga/backend path.

const M_PI_F: f32 = 3.1415926;
const SHADER_EPSILON: f32 = 0.000001;
const SDF_ANTIALIAS_THRESHOLD: f32 = 0.5;
const GRAYSCALE_FACTORS: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

struct Bounds {
    origin: vec2<f32>,
    size: vec2<f32>,
}

struct Corners {
    top_left: f32,
    top_right: f32,
    bottom_right: f32,
    bottom_left: f32,
}

struct ContentMask {
    bounds: Bounds,
    corner_bounds: Bounds,
    corner_radii: Corners,
}

fn pick_corner_radius(center_to_point: vec2<f32>, radii: Corners) -> f32 {
    let top_side = center_to_point.y < 0.0;
    let left_radius = select(radii.bottom_left, radii.top_left, top_side);
    let right_radius = select(radii.bottom_right, radii.top_right, top_side);
    return select(right_radius, left_radius, center_to_point.x < 0.0);
}

fn pick_corner_radius_from_packed(center_to_point: vec2<f32>, packed_radii: vec4<f32>) -> f32 {
    let top_side = center_to_point.y < 0.0;
    let left_radius = select(packed_radii.w, packed_radii.x, top_side);
    let right_radius = select(packed_radii.z, packed_radii.y, top_side);
    return select(right_radius, left_radius, center_to_point.x < 0.0);
}

fn quad_sdf(point: vec2<f32>, bounds: Bounds, corner_radii: Corners) -> f32 {
    let half_size = bounds.size / 2.0;
    let center = bounds.origin + half_size;
    let center_to_point = point - center;
    let corner_radius = pick_corner_radius(center_to_point, corner_radii);
    let corner_to_point = abs(center_to_point) - half_size;
    return quad_sdf_impl(corner_to_point + corner_radius, corner_radius);
}

fn quad_sdf_from_packed(point: vec2<f32>, packed_bounds: vec4<f32>, packed_corner_radii: vec4<f32>) -> f32 {
    let half_size = packed_bounds.zw / 2.0;
    let center_to_point = point - (packed_bounds.xy + half_size);
    let corner_radius = pick_corner_radius_from_packed(center_to_point, packed_corner_radii);
    let corner_to_point = abs(center_to_point) - half_size;
    return quad_sdf_impl(corner_to_point + corner_radius, corner_radius);
}

fn quad_sdf_impl(corner_center_to_point: vec2<f32>, corner_radius: f32) -> f32 {
    if (corner_radius == 0.0) {
        return max(corner_center_to_point.x, corner_center_to_point.y);
    }
    let signed_distance_to_inset_quad =
        length(max(vec2<f32>(0.0), corner_center_to_point)) +
        min(0.0, max(corner_center_to_point.x, corner_center_to_point.y));
    return signed_distance_to_inset_quad - corner_radius;
}

struct Edges {
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
}

struct Hsla {
    h: f32,
    s: f32,
    l: f32,
    a: f32,
}

struct LinearColorStop {
    color: Hsla,
    percentage: f32,
}

struct Background {
    // 0u is Solid
    // 1u is LinearGradient
    // 2u is PatternSlash
    tag: u32,
    // 0u is sRGB linear color
    // 1u is Oklab color
    color_space: u32,
    solid: Hsla,
    gradient_angle_or_pattern_height: f32,
    colors: array<LinearColorStop, 2>,
    pad: u32,
}

struct AtlasTextureId {
    index: u32,
    kind: u32,
}

struct AtlasBounds {
    origin: vec2<i32>,
    size: vec2<i32>,
}

struct AtlasTile {
    texture_id: AtlasTextureId,
    tile_id: u32,
    padding: u32,
    bounds: AtlasBounds,
}

struct TransformationMatrix {
    rotation_scale: mat2x2<f32>,
    translation: vec2<f32>,
}

fn to_device_position_impl(position: vec2<f32>) -> vec4<f32> {
    let viewport_size = max(globals.viewport_size, vec2<f32>(1.0));
    let device_position = position / viewport_size * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    return vec4<f32>(device_position, 0.0, 1.0);
}

fn viewport_texture_coords(position: vec2<f32>) -> vec2<f32> {
    return position / max(globals.viewport_size, vec2<f32>(1.0));
}

fn to_device_position(unit_vertex: vec2<f32>, bounds: Bounds) -> vec4<f32> {
    let position = unit_vertex * vec2<f32>(bounds.size) + bounds.origin;
    return to_device_position_impl(position);
}

fn to_device_position_transformed(unit_vertex: vec2<f32>, bounds: Bounds, transform: TransformationMatrix) -> vec4<f32> {
    let position = unit_vertex * vec2<f32>(bounds.size) + bounds.origin;
    //Note: Rust side stores it as row-major, so transposing here
    let transformed = transpose(transform.rotation_scale) * position + transform.translation;
    return to_device_position_impl(transformed);
}

fn to_tile_position(unit_vertex: vec2<f32>, tile: AtlasTile) -> vec2<f32> {
    let atlas_size = max(vec2<f32>(textureDimensions(t_sprite, 0)), vec2<f32>(1.0));
    return (vec2<f32>(tile.bounds.origin) + unit_vertex * vec2<f32>(tile.bounds.size)) / atlas_size;
}

fn distance_from_clip_rect_impl(position: vec2<f32>, clip_bounds: Bounds) -> vec4<f32> {
    let tl = position - clip_bounds.origin;
    let br = clip_bounds.origin + clip_bounds.size - position;
    return vec4<f32>(tl.x, br.x, tl.y, br.y);
}

fn distance_from_clip_rect(unit_vertex: vec2<f32>, bounds: Bounds, clip_bounds: Bounds) -> vec4<f32> {
    let position = unit_vertex * vec2<f32>(bounds.size) + bounds.origin;
    return distance_from_clip_rect_impl(position, clip_bounds);
}

fn distance_from_clip_rect_transformed(unit_vertex: vec2<f32>, bounds: Bounds, clip_bounds: Bounds, transform: TransformationMatrix) -> vec4<f32> {
    let position = unit_vertex * vec2<f32>(bounds.size) + bounds.origin;
    let transformed = transpose(transform.rotation_scale) * position + transform.translation;
    return distance_from_clip_rect_impl(transformed, clip_bounds);
}

fn content_mask_coverage(position: vec2<f32>, content_mask: ContentMask) -> f32 {
    let packed_bounds = vec4<f32>(content_mask.corner_bounds.origin, content_mask.corner_bounds.size);
    let packed_radii = vec4<f32>(
        content_mask.corner_radii.top_left,
        content_mask.corner_radii.top_right,
        content_mask.corner_radii.bottom_right,
        content_mask.corner_radii.bottom_left,
    );
    return content_mask_coverage_from_packed(position, packed_bounds, packed_radii);
}

fn content_mask_coverage_from_packed(position: vec2<f32>, packed_bounds: vec4<f32>, packed_radii: vec4<f32>) -> f32 {
    if (all(packed_radii == vec4<f32>(0.0))) {
        return 1.0;
    }
    let distance = quad_sdf_from_packed(position, packed_bounds, packed_radii);
    return saturate(SDF_ANTIALIAS_THRESHOLD - distance);
}

/// Hsla to linear RGBA conversion.
fn hsla_to_rgba(hsla: Hsla) -> vec4<f32> {
    let chroma = (1.0 - abs(2.0 * hsla.l - 1.0)) * hsla.s;
    let rgb = clamp(
        abs(fract(hsla.h + vec3<f32>(0.0, 0.6666667, 0.33333334)) * 6.0 - vec3<f32>(3.0)) - vec3<f32>(1.0),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    let color = vec3<f32>(hsla.l) + chroma * (rgb - vec3<f32>(0.5));
    return vec4<f32>(color, hsla.a);
}

fn over(below: vec4<f32>, above: vec4<f32>) -> vec4<f32> {
    let alpha = above.a + below.a * (1.0 - above.a);
    if (alpha <= SHADER_EPSILON) {
        return vec4<f32>(0.0);
    }

    let color = (above.rgb * above.a + below.rgb * below.a * (1.0 - above.a)) / alpha;
    return vec4<f32>(color, alpha);
}

// Abstract away the final color transformation based on the
// target alpha compositing mode.
fn blend_color(color: vec4<f32>, alpha_factor: f32) -> vec4<f32> {
    let alpha = color.a * alpha_factor;
    let multiplier = select(1.0, alpha, globals.premultiplied_alpha != 0u);
    return vec4<f32>(color.rgb * multiplier, alpha);
}
