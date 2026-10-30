// Unlit voxel geometry stores only the attributes consumed by this shader (24 bytes/vertex).
// The high bit of vertex_base selects GPU expansion of ordered quads into two triangles.
struct VoxelVertex {
    position_x: f32,
    position_y: f32,
    position_z: f32,
    color_rgba8: u32,
    uv: vec2<f32>,
};

struct Instance {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    pixel_offset: vec2<f32>,
    depth_bias: f32,
    vertex_base: u32,
};

struct DrawParams {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    metallic: f32,
    roughness: f32,
    alpha_cutoff: f32,
    normal_mapping_enabled: u32,
    occlusion_strength: f32,
    alpha_mode: u32,
    light_count: u32,
    shading_model: u32,
    texture_flags: u32,
};

struct FrameParams {
    view_projection: mat4x4<f32>,
    camera_position: vec3<f32>,
    element_bounds_origin: vec2<f32>,
    element_bounds_size: vec2<f32>,
    render_target_size: vec2<f32>,
    blend_edge_feather_px: f32,
    _padding: f32,
};

struct VoxelOutput {
    @builtin(position) position: vec4<f32>,
    @location(2) vertex_color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) @interpolate(flat) draw_index: u32,
};

const ALPHA_MODE_MASK: u32 = 1u;
const ALPHA_MODE_BLEND: u32 = 2u;
const TEXTURE_FLAG_ALBEDO: u32 = 1u;
const TEXTURE_FLAG_UV: u32 = 4u;

@group(0) @binding(0) var<storage, read> vertices: array<VoxelVertex>;
@group(0) @binding(1) var<storage, read> draws: array<DrawParams>;
@group(0) @binding(3) var albedo_texture: texture_2d<f32>;
@group(0) @binding(4) var material_sampler: sampler;
@group(0) @binding(8) var<storage, read> instances: array<Instance>;
@group(0) @binding(9) var<uniform> frame: FrameParams;
@group(0) @binding(11) var<storage, read> visible_instances: array<u32>;
struct GeneratedTemplate { info:vec4<u32>, vertices:array<VoxelVertex,4> };
struct GeneratedTemplates {origin:vec4<f32>,size:vec4<u32>,faces:array<GeneratedTemplate>};
struct GeneratedFace {cell:u32,model_face:u32,height:f32,padding:u32};
@group(0) @binding(12) var<storage, read> generated_templates:GeneratedTemplates;
@group(0) @binding(13) var<storage, read> generated_faces:array<GeneratedFace>;

@vertex
fn vs_generated_voxel(@builtin(vertex_index) index:u32, @builtin(instance_index) instance_index:u32)->VoxelOutput {
    let base=instances[instance_index].vertex_base & 0x7fffffffu;
    let descriptor=generated_faces[base+index/6u];
    let corners=array<u32,6>(0u,1u,2u,0u,2u,3u);
    var vertex=generated_templates.faces[descriptor.model_face].vertices[corners[index%6u]];
    var position=vec3<f32>(0.0);
    if(descriptor.cell!=0xffffffffu){
        let size=generated_templates.size;
        position=generated_templates.origin.xyz+vec3<f32>(f32(descriptor.cell%size.x),f32(descriptor.cell/size.x%size.y),f32(descriptor.cell/size.x/size.y));
    }
    vertex.position_x+=position.x;
    vertex.position_y=vertex.position_y*descriptor.height+position.y;
    vertex.position_z+=position.z;
    return shade_voxel_value(vertex,instance_index);
}

@vertex
fn vs_voxel(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VoxelOutput {
    return shade_voxel(vertex_index, instance_index);
}

@vertex
fn vs_voxel_gpu(@builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) visible_index: u32) -> VoxelOutput {
    // Direct indexed ranges keep their original instance index. Only procedural
    // quad batches reserve this bit and consume the compute-compacted instance list.
    // Every slot within a quad batch has that bit set before compaction.
    if ((instances[visible_index].vertex_base & 0x80000000u) == 0u) {
        return shade_voxel(vertex_index, visible_index);
    }
    let instance_index = visible_instances[visible_index];
    // normal_matrix is unused by the unlit voxel shader; its first word carries
    // the real vertex count for variable-size parts in a shared indirect batch.
    let vertex_count = bitcast<u32>(instances[instance_index].normal_matrix[0].x);
    if (vertex_index >= vertex_count) {
        var clipped: VoxelOutput;
        clipped.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return clipped;
    }
    return shade_voxel(vertex_index, instance_index);
}

fn shade_voxel(vertex_index: u32, instance_index: u32) -> VoxelOutput {
    let instance = instances[instance_index];
    let quad_expansion = (instance.vertex_base & 0x80000000u) != 0u;
    let base = instance.vertex_base & 0x7fffffffu;
    let corners = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    let local_vertex = select(
        vertex_index,
        (vertex_index / 6u) * 4u + corners[vertex_index % 6u],
        quad_expansion,
    );
    let vertex = vertices[local_vertex + base];
    return shade_voxel_value(vertex,instance_index);
}

fn shade_voxel_value(vertex:VoxelVertex,instance_index:u32)->VoxelOutput {
    let instance=instances[instance_index];
    let world_position = instance.model * vec4<f32>(
        vertex.position_x, vertex.position_y, vertex.position_z, 1.0,
    );
    let clip_position = frame.view_projection * world_position;
    let safe_clip_w = select(
        min(clip_position.w, -0.0001),
        max(clip_position.w, 0.0001),
        clip_position.w >= 0.0,
    );
    let ndc = clip_position.xyz / safe_clip_w;
    let screen_position_pixels = frame.element_bounds_origin
        + (ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5)) * frame.element_bounds_size
        + instance.pixel_offset;
    let normalized_target_position = screen_position_pixels
        / max(frame.render_target_size, vec2<f32>(1.0));

    var output: VoxelOutput;
    output.draw_index = instance_index;
    output.position = vec4<f32>(
        (normalized_target_position * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0))
            * clip_position.w,
        (ndc.z - instance.depth_bias) * clip_position.w,
        clip_position.w,
    );
    output.vertex_color = vec4<f32>(
        f32(vertex.color_rgba8 & 255u),
        f32((vertex.color_rgba8 >> 8u) & 255u),
        f32((vertex.color_rgba8 >> 16u) & 255u),
        f32((vertex.color_rgba8 >> 24u) & 255u),
    ) / 255.0;
    output.uv = vertex.uv;
    return output;
}

fn element_bounds_coverage(screen_position: vec2<f32>) -> f32 {
    if (frame.blend_edge_feather_px <= 0.0) {
        return 1.0;
    }
    let origin = frame.element_bounds_origin;
    let size = frame.element_bounds_size;
    let edge_distance_pixels = min(
        min(screen_position.x - origin.x, origin.x + size.x - screen_position.x),
        min(screen_position.y - origin.y, origin.y + size.y - screen_position.y),
    );
    return clamp(edge_distance_pixels / frame.blend_edge_feather_px, 0.0, 1.0);
}

@fragment
fn fs_voxel(input: VoxelOutput) -> @location(0) vec4<f32> {
    let draw = draws[input.draw_index];
    var color = draw.base_color * input.vertex_color;
    if ((draw.texture_flags & TEXTURE_FLAG_ALBEDO) != 0u) {
        if ((draw.texture_flags & TEXTURE_FLAG_UV) != 0u) {
            color *= textureSample(albedo_texture, material_sampler, input.uv);
        } else {
            color *= textureSample(albedo_texture, material_sampler, vec2<f32>(0.5, 0.5));
        }
    }
    let alpha = clamp(color.a, 0.0, 1.0);
    if (draw.alpha_mode == ALPHA_MODE_MASK && alpha < draw.alpha_cutoff) {
        discard;
    }
    let rgb = color.rgb + draw.emissive.rgb * draw.emissive.a;
    if (draw.alpha_mode == ALPHA_MODE_BLEND) {
        let coverage = element_bounds_coverage(input.position.xy);
        let output_alpha = alpha * coverage;
        return vec4<f32>(rgb * output_alpha, output_alpha);
    }
    return vec4<f32>(rgb, 1.0);
}
