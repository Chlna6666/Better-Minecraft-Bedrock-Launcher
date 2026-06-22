// --- GPU mesh 3D --- //

struct GpuMesh3dParams {
    bounds_origin: vec2<f32>,
    bounds_size: vec2<f32>,
    content_mask_origin: vec2<f32>,
    content_mask_size: vec2<f32>,
    view_proj_model: mat4x4<f32>,
}

struct GpuMesh3dVertex {
    position_x: f32,
    position_y: f32,
    position_z: f32,
    color_r: f32,
    color_g: f32,
    color_b: f32,
    color_a: f32,
}

@group(0) @binding(20) var<uniform> gpu_mesh_3d_params: GpuMesh3dParams;
@group(0) @binding(21) var<storage, read> b_gpu_mesh_3d_vertices: array<GpuMesh3dVertex>;
@group(0) @binding(22) var t_gpu_mesh_3d: texture_2d<f32>;
@group(0) @binding(23) var s_gpu_mesh_3d: sampler;

struct GpuMesh3dVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) clip_distances: vec4<f32>,
}

fn gpu_mesh_3d_clip_bounds(params: GpuMesh3dParams) -> Bounds {
    let edge_inset = min(vec2<f32>(6.0, 6.0), params.bounds_size * vec2<f32>(0.08, 0.08));
    let mesh_bounds = Bounds(
        params.bounds_origin + edge_inset,
        max(params.bounds_size - edge_inset * vec2<f32>(2.0, 2.0), vec2<f32>(1.0, 1.0)),
    );
    let content_bounds = Bounds(params.content_mask_origin, params.content_mask_size);
    let clip_origin = max(mesh_bounds.origin, content_bounds.origin);
    let clip_max = min(
        mesh_bounds.origin + mesh_bounds.size,
        content_bounds.origin + content_bounds.size,
    );
    return Bounds(clip_origin, max(clip_max - clip_origin, vec2<f32>(0.0, 0.0)));
}

@vertex
fn vs_gpu_mesh_3d(@builtin(vertex_index) vertex_id: u32) -> GpuMesh3dVarying {
    let vertex = b_gpu_mesh_3d_vertices[vertex_id];
    let params = gpu_mesh_3d_params;
    let model_position = vec4<f32>(vertex.position_x, vertex.position_y, vertex.position_z, 1.0);
    let clip_position = params.view_proj_model * model_position;
    let ndc = clip_position.xyz / max(clip_position.w, 0.0001);
    let draw_bounds = gpu_mesh_3d_clip_bounds(params);
    let unit = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    let pixel_position = draw_bounds.origin + unit * draw_bounds.size;
    let device_position = to_device_position_impl(pixel_position);

    var out = GpuMesh3dVarying();
    out.position = vec4<f32>(device_position.xy, ndc.z, 1.0);
    out.color = vec4<f32>(vertex.color_r, vertex.color_g, vertex.color_b, vertex.color_a);
    out.clip_distances = distance_from_clip_rect_impl(pixel_position, draw_bounds);
    return out;
}

@fragment
fn fs_gpu_mesh_3d(input: GpuMesh3dVarying) -> @location(0) vec4<f32> {
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        discard;
    }
    if (input.color.a <= 0.0) {
        discard;
    }
    let edge_alpha = clamp(
        min(
            min(input.clip_distances.x, input.clip_distances.y),
            min(input.clip_distances.z, input.clip_distances.w),
        ),
        0.0,
        1.0,
    );
    let alpha = input.color.a * edge_alpha;
    return vec4<f32>(input.color.rgb * alpha, alpha);
}

struct GpuMesh3dCompositeVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) texture_coords: vec2<f32>,
}

@vertex
fn vs_gpu_mesh_3d_composite(@builtin(vertex_index) vertex_id: u32) -> GpuMesh3dCompositeVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let position = unit_vertex * gpu_mesh_3d_params.bounds_size + gpu_mesh_3d_params.bounds_origin;

    var out = GpuMesh3dCompositeVarying();
    out.position = to_device_position_impl(position);
    out.texture_coords = viewport_texture_coords(position);
    return out;
}

@fragment
fn fs_gpu_mesh_3d_composite(input: GpuMesh3dCompositeVarying) -> @location(0) vec4<f32> {
    return textureSampleLevel(t_gpu_mesh_3d, s_gpu_mesh_3d, input.texture_coords, 0.0);
}
