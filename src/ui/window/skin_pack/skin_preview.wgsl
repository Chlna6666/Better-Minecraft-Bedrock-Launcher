struct SkinPreviewDrawParameters {
    bounds_origin: vec2<f32>,
    bounds_size: vec2<f32>,
    content_mask_origin: vec2<f32>,
    content_mask_size: vec2<f32>,
    view_proj_model: mat4x4<f32>,
};

struct GlobalParams {
    viewport_size: vec2<f32>,
    premultiplied_alpha: u32,
    pad: u32,
};

struct SkinPreviewVertex {
    position_x: f32,
    position_y: f32,
    position_z: f32,
    color_r: f32,
    color_g: f32,
    color_b: f32,
    color_a: f32,
};

struct SkinPreviewVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) clip_distances: vec4<f32>,
    @location(2) barycentric: vec3<f32>,
    @location(3) @interpolate(flat) edge_mask: u32,
};

@group(0) @binding(0) var<uniform> globals: GlobalParams;
@group(0) @binding(20) var<storage, read> skin_preview_draw_parameters: array<SkinPreviewDrawParameters>;
@group(0) @binding(21) var<storage, read> skin_preview_vertices: array<SkinPreviewVertex>;

@vertex
fn vs_skin_preview(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> SkinPreviewVarying {
    let vertex = skin_preview_vertices[vertex_index];
    let draw_parameters = skin_preview_draw_parameters[instance_index];
    let encoded_view_proj_model = draw_parameters.view_proj_model;
    let opacity = clamp(encoded_view_proj_model[0].w, 0.0, 1.0);
    let pixel_offset = vec2<f32>(encoded_view_proj_model[1].w, encoded_view_proj_model[2].w);
    let depth_bias = clamp(encoded_view_proj_model[3].w - 1.0, 0.0, 0.01);
    let view_proj_model = mat4x4<f32>(
        vec4<f32>(encoded_view_proj_model[0].xyz, 0.0),
        vec4<f32>(encoded_view_proj_model[1].xyz, 0.0),
        vec4<f32>(encoded_view_proj_model[2].xyz, 0.0),
        vec4<f32>(encoded_view_proj_model[3].xyz, 1.0),
    );
    let model_position = vec4<f32>(vertex.position_x, vertex.position_y, vertex.position_z, 1.0);
    let clip_position = view_proj_model * model_position;
    let ndc = clip_position.xyz / max(clip_position.w, 0.0001);

    let edge_inset = min(vec2<f32>(6.0, 6.0), draw_parameters.bounds_size * vec2<f32>(0.08, 0.08));
    let mesh_origin = draw_parameters.bounds_origin + edge_inset;
    let mesh_size = max(draw_parameters.bounds_size - edge_inset * vec2<f32>(2.0, 2.0), vec2<f32>(1.0, 1.0));
    let content_origin = draw_parameters.content_mask_origin;
    let content_size = draw_parameters.content_mask_size;
    let draw_origin = max(mesh_origin, content_origin);
    let draw_max = min(mesh_origin + mesh_size, content_origin + content_size);
    let draw_rect_size = max(draw_max - draw_origin, vec2<f32>(0.0, 0.0));
    let square_size = min(draw_rect_size.x, draw_rect_size.y);
    let square_offset = (draw_rect_size - vec2<f32>(square_size, square_size)) * vec2<f32>(0.5, 0.5);
    let square_origin = draw_origin + square_offset;
    let draw_size = vec2<f32>(square_size, square_size);
    let unit = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    let pixel_position = square_origin + unit * draw_size + pixel_offset;
    let viewport_size = max(globals.viewport_size, vec2<f32>(1.0));
    let device_position = pixel_position / viewport_size * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);

    let encoded_alpha = vertex.color_a;
    let edge_mask = u32(floor(encoded_alpha * 0.5));
    let vertex_alpha = encoded_alpha - f32(edge_mask) * 2.0;
    let triangle_vertex = vertex_index % 3u;
    var barycentric = vec3<f32>(0.0, 0.0, 1.0);
    if (triangle_vertex == 0u) {
        barycentric = vec3<f32>(1.0, 0.0, 0.0);
    } else if (triangle_vertex == 1u) {
        barycentric = vec3<f32>(0.0, 1.0, 0.0);
    }

    var out: SkinPreviewVarying;
    let depth = clamp(0.5 - ndc.z * 0.5 + depth_bias, 0.0, 1.0);
    out.position = vec4<f32>(device_position, depth, 1.0);
    out.color = vec4<f32>(vertex.color_r, vertex.color_g, vertex.color_b, vertex_alpha * opacity);
    let top_left = pixel_position - square_origin;
    let bottom_right = square_origin + draw_size - pixel_position;
    out.clip_distances = vec4<f32>(top_left.x, bottom_right.x, top_left.y, bottom_right.y);
    out.barycentric = barycentric;
    out.edge_mask = edge_mask;
    return out;
}

fn skin_preview_edge_alpha(barycentric: vec3<f32>, edge_mask: u32) -> f32 {
    let edge_width = max(fwidth(barycentric) * vec3<f32>(1.25), vec3<f32>(0.00001));
    let edge_alpha = smoothstep(vec3<f32>(0.0), edge_width, barycentric);
    var alpha = 1.0;
    if ((edge_mask & 1u) != 0u) {
        alpha = min(alpha, edge_alpha.x);
    }
    if ((edge_mask & 2u) != 0u) {
        alpha = min(alpha, edge_alpha.y);
    }
    if ((edge_mask & 4u) != 0u) {
        alpha = min(alpha, edge_alpha.z);
    }
    return alpha;
}

@fragment
fn fs_skin_preview(
    input: SkinPreviewVarying,
) -> @location(0) vec4<f32> {
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        discard;
    }
    let edge_alpha = clamp(min(min(input.clip_distances.x, input.clip_distances.y), min(input.clip_distances.z, input.clip_distances.w)), 0.0, 1.0);
    let mesh_edge_alpha = skin_preview_edge_alpha(input.barycentric, input.edge_mask);
    let alpha = input.color.a * edge_alpha * mesh_edge_alpha;
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(input.color.rgb * alpha, alpha);
}

@fragment
fn fs_skin_preview_unclipped(
    input: SkinPreviewVarying,
) -> @location(0) vec4<f32> {
    let alpha = input.color.a * skin_preview_edge_alpha(input.barycentric, input.edge_mask);
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(input.color.rgb * alpha, alpha);
}
