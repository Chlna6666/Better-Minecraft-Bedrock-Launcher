struct Preview3dDrawParameters {
    bounds_origin: vec2<f32>,
    bounds_size: vec2<f32>,
    content_mask_origin: vec2<f32>,
    content_mask_size: vec2<f32>,
    view_proj_model: mat4x4<f32>,
};

struct MeshAnimation {
    property_and_flags: vec4<u32>,
    sampled: vec4<f32>,
};

struct GlobalParams {
    viewport_size: vec2<f32>,
    premultiplied_alpha: u32,
    pad: u32,
};

struct Preview3dVertex {
    position_x: f32,
    position_y: f32,
    position_z: f32,
    color_rgba8: u32,
};

struct Preview3dVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) @interpolate(flat) draw_bounds: vec4<f32>,
    @location(2) @interpolate(flat) animation_opacity: f32,
};

@group(0) @binding(0) var<uniform> globals: GlobalParams;
@group(0) @binding(20) var<storage, read> preview_3d_draw_parameters: array<Preview3dDrawParameters>;
@group(0) @binding(21) var<storage, read> preview_3d_vertices: array<Preview3dVertex>;
@group(0) @binding(22) var<storage, read> preview_3d_animations: array<MeshAnimation>;

fn decode_preview_3d_color(encoded: u32) -> vec4<f32> {
    let red = f32(encoded & 0xffu) / 255.0;
    let green = f32((encoded >> 8u) & 0xffu) / 255.0;
    let blue = f32((encoded >> 16u) & 0xffu) / 255.0;
    let alpha_and_flags = (encoded >> 24u) & 0xffu;
    let alpha = f32(alpha_and_flags & 0x1fu) / 31.0;
    return vec4<f32>(red, green, blue, alpha);
}

fn mesh_animation_active(animation: MeshAnimation) -> bool {
    return animation.property_and_flags.y != 0u;
}

fn mesh_animation_opacity(animation: MeshAnimation) -> f32 {
    if (!mesh_animation_active(animation)) {
        return 1.0;
    }
    let property = animation.property_and_flags.x;
    if (property == 1u) {
        return animation.sampled.x;
    }
    if (property == 4u) {
        return animation.sampled.y;
    }
    return 1.0;
}

@vertex
fn vs_preview_3d(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> Preview3dVarying {
    let vertex = preview_3d_vertices[vertex_index];
    let draw_parameters = preview_3d_draw_parameters[instance_index];
    let animation = preview_3d_animations[instance_index];

    // GPUI visual animation is applied to the 2D primitive envelope only. The actual 3D camera,
    // model transform and depth remain entirely in view_proj_model.
    let base_bounds_origin = draw_parameters.bounds_origin;
    let base_bounds_size = draw_parameters.bounds_size;
    var bounds_origin = base_bounds_origin;
    var bounds_size = base_bounds_size;
    var content_origin = draw_parameters.content_mask_origin;
    var content_size = draw_parameters.content_mask_size;
    if (mesh_animation_active(animation)) {
        let property = animation.property_and_flags.x;
        if (property == 3u) {
            // Match GPUI Translation semantics: move the primitive bounds, not the content mask.
            bounds_origin = bounds_origin + animation.sampled.xy;
        } else if (property == 2u || property == 4u) {
            let scale = animation.sampled.x;
            var pivot = base_bounds_origin + base_bounds_size * vec2<f32>(0.5, 0.5);
            if (property == 4u) {
                pivot = animation.sampled.zw;
            }
            bounds_origin = pivot + (bounds_origin - pivot) * scale;
            bounds_size = bounds_size * scale;
            content_origin = pivot + (content_origin - pivot) * scale;
            content_size = content_size * scale;
        }
    }

    let model_position = vec4<f32>(vertex.position_x, vertex.position_y, vertex.position_z, 1.0);
    let clip_position = draw_parameters.view_proj_model * model_position;
    let safe_clip_w = select(
        min(clip_position.w, -0.0001),
        max(clip_position.w, 0.0001),
        clip_position.w >= 0.0,
    );
    let ndc = clip_position.xyz / safe_clip_w;

    let edge_inset = min(vec2<f32>(6.0, 6.0), bounds_size * vec2<f32>(0.08, 0.08));
    let mesh_origin = bounds_origin + edge_inset;
    let mesh_size = max(bounds_size - edge_inset * vec2<f32>(2.0, 2.0), vec2<f32>(1.0, 1.0));
    let draw_origin = max(mesh_origin, content_origin);
    let draw_max = min(mesh_origin + mesh_size, content_origin + content_size);
    let draw_size = max(draw_max - draw_origin, vec2<f32>(0.0, 0.0));
    let unit = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    let pixel_position = draw_origin + unit * draw_size;
    let viewport_size = max(globals.viewport_size, vec2<f32>(1.0));
    let device_position = pixel_position / viewport_size * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);

    var out: Preview3dVarying;
    out.position = vec4<f32>(
        device_position * clip_position.w,
        clip_position.z,
        clip_position.w,
    );
    out.color = decode_preview_3d_color(vertex.color_rgba8);
    out.draw_bounds = vec4<f32>(draw_origin, draw_origin + draw_size);
    out.animation_opacity = mesh_animation_opacity(animation);
    return out;
}

fn preview_3d_fragment_inside(input: Preview3dVarying) -> bool {
    let fragment_position = input.position.xy;
    let draw_bounds = input.draw_bounds;
    return fragment_position.x >= draw_bounds.x
        && fragment_position.x <= draw_bounds.z
        && fragment_position.y >= draw_bounds.y
        && fragment_position.y <= draw_bounds.w;
}

fn preview_3d_edge_alpha(input: Preview3dVarying) -> f32 {
    let fragment_position = input.position.xy;
    let draw_bounds = input.draw_bounds;
    return clamp(min(
        min(fragment_position.x - draw_bounds.x, draw_bounds.z - fragment_position.x),
        min(fragment_position.y - draw_bounds.y, draw_bounds.w - fragment_position.y),
    ), 0.0, 1.0);
}

@fragment
fn fs_preview_3d(input: Preview3dVarying) -> @location(0) vec4<f32> {
    if (!preview_3d_fragment_inside(input)) {
        discard;
    }
    let alpha = input.color.a * preview_3d_edge_alpha(input) * input.animation_opacity;
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(input.color.rgb * alpha, alpha);
}

@fragment
fn fs_preview_3d_opaque(input: Preview3dVarying) -> @location(0) vec4<f32> {
    if (!preview_3d_fragment_inside(input)) {
        discard;
    }
    let alpha = input.animation_opacity;
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(input.color.rgb * alpha, alpha);
}

@fragment
fn fs_preview_3d_cutout(input: Preview3dVarying) -> @location(0) vec4<f32> {
    if (!preview_3d_fragment_inside(input)) {
        discard;
    }
    // The live GPUI preview currently carries material/UV metadata on CPU but does not bind an
    // albedo texture here. Keep the geometry opaque until that sampler path is wired.
    let alpha = input.animation_opacity;
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(input.color.rgb * alpha, alpha);
}

@fragment
fn fs_preview_3d_transparent(input: Preview3dVarying) -> @location(0) vec4<f32> {
    if (!preview_3d_fragment_inside(input)) {
        discard;
    }
    let alpha = input.color.a * preview_3d_edge_alpha(input) * input.animation_opacity;
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(input.color.rgb * alpha, alpha);
}

@fragment
fn fs_preview_3d_unclipped(input: Preview3dVarying) -> @location(0) vec4<f32> {
    let alpha = input.color.a * input.animation_opacity;
    return vec4<f32>(input.color.rgb * input.animation_opacity, alpha);
}
