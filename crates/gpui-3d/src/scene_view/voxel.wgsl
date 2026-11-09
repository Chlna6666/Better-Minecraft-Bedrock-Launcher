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
    light_count: u32,
    fog_color: vec4<f32>,
    fog_range: vec4<f32>,
    voxel_settings:vec4<f32>,
    sun_direction:vec4<f32>,
    sun_color:vec4<f32>,
    shadow_matrix:mat4x4<f32>,
    shadow_settings:vec4<f32>,
};

struct VoxelOutput {
    @builtin(position) position: vec4<f32>,
    @location(2) vertex_color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) @interpolate(flat) draw_index: u32,
    @location(5) fog_distance: f32,
    @location(6) world_position:vec3<f32>,
    @location(7) surface_normal:vec3<f32>,
    @location(8) quad_uv:vec2<f32>,
    @location(9) @interpolate(flat) merged_run:u32,
};

const ALPHA_MODE_MASK: u32 = 1u;
const ALPHA_MODE_BLEND: u32 = 2u;
const TEXTURE_FLAG_ALBEDO: u32 = 1u;
const TEXTURE_FLAG_UV: u32 = 4u;
const PI:f32=3.141592653589793;

@group(0) @binding(0) var<storage, read> vertices: array<VoxelVertex>;
@group(0) @binding(1) var<storage, read> draws: array<DrawParams>;
@group(0) @binding(3) var albedo_texture: texture_2d<f32>;
@group(0) @binding(4) var material_sampler: sampler;
@group(0) @binding(5) var normal_texture:texture_2d<f32>;
@group(0) @binding(10) var material_texture:texture_2d<f32>;
@group(0) @binding(8) var<storage, read> instances: array<Instance>;
@group(0) @binding(9) var<uniform> frame: FrameParams;
@group(0) @binding(11) var<storage, read> visible_instances: array<u32>;
// Culling/batch order is logical; Section records use stable physical slots.
@group(0) @binding(16) var<storage, read> draw_slot_indices: array<u32>;
struct GeneratedTemplate { info:vec4<u32>, vertices:array<VoxelVertex,4> };
struct GeneratedFace {cell:u32,model_face:u32,height:f32,padding:u32};
@group(0) @binding(12) var<storage, read> generated_templates:array<GeneratedTemplate>;
@group(0) @binding(13) var<storage, read> generated_faces:array<GeneratedFace>;
@group(0) @binding(14) var<storage, read> lighting_words:array<u32>;

fn cell_levels(cell:u32,size:vec4<u32>)->vec2<f32> {
    let word=lighting_words[size.w+cell/4u];
    let packed=(word>>((cell%4u)*8u))&255u;
    return vec2<f32>(f32(packed&15u),f32(packed>>4u))/15.0;
}

fn static_levels(p:vec3<f32>,origin:vec4<f32>,size:vec4<u32>)->vec2<f32> {
    let q=vec3<i32>(floor(p-origin.xyz));
    let dims=size.xyz;
    if(any(q<vec3<i32>(0))||any(q>=vec3<i32>(dims))){return vec2<f32>(0.0);}
    return cell_levels((u32(q.z)*size.y+u32(q.y))*size.x+u32(q.x),size);
}

// Smooth lighting: the four front-plane cells around one face corner, averaged so
// adjacent faces interpolate instead of stepping one whole light level per block.
fn normal_from_delta(delta:i32,size:vec4<u32>)->vec3<i32> {
    let dims=vec3<i32>(size.xyz);
    var n=vec3<i32>(0);
    if(delta==1){n.x=1;}else if(delta==-1){n.x=-1;}
    else if(delta==dims.x){n.y=1;}else if(delta==-dims.x){n.y=-1;}
    else if(delta==dims.x*dims.y){n.z=1;}else if(delta==-dims.x*dims.y){n.z=-1;}
    return n;
}
fn corner_cell_light(cell:u32,delta:i32,local:vec3<f32>,size:vec4<u32>)->vec2<f32> {
    let n=normal_from_delta(delta,size);
    if(all(n==vec3<i32>(0))){return cell_levels(cell,size);}
    let dims=vec3<i32>(size.xyz);
    let c=vec3<i32>(i32(cell%size.x),i32(cell/size.x%size.y),i32(cell/(size.x*size.y)));
    var steps=array<vec3<i32>,2>(vec3<i32>(0),vec3<i32>(0));
    if(n.x!=0){steps[0]=vec3<i32>(0,select(-1,1,local.y>=0.5),0);steps[1]=vec3<i32>(0,0,select(-1,1,local.z>=0.5));}
    else if(n.y!=0){steps[0]=vec3<i32>(select(-1,1,local.x>=0.5),0,0);steps[1]=vec3<i32>(0,0,select(-1,1,local.z>=0.5));}
    else{steps[0]=vec3<i32>(select(-1,1,local.x>=0.5),0,0);steps[1]=vec3<i32>(0,select(-1,1,local.y>=0.5),0);}
    var sum=vec2<f32>(0.0);var count=0.0;
    for(var i=0u;i<4u;i++){
        var offset=n;
        if(i==1u){offset+=steps[0];}
        if(i==2u){offset+=steps[1];}
        if(i==3u){offset+=steps[0]+steps[1];}
        let p=c+offset;
        if(any(p<vec3<i32>(0))||any(p>=dims)){continue;}
        sum+=cell_levels(u32((p.z*dims.y+p.y)*dims.x+p.x),size);count+=1.0;
    }
    if(count==0.0){return cell_levels(cell,size);}
    return sum/vec2<f32>(count);
}

fn quad_coordinates(corner:u32)->vec2<f32> {return array<vec2<f32>,4>(vec2<f32>(0.0),vec2<f32>(1.0,0.0),vec2<f32>(1.0),vec2<f32>(0.0,1.0))[corner];}

fn quad_normal(a:vec3<f32>,b:vec3<f32>,c:vec3<f32>,model:mat4x4<f32>)->vec3<f32> {
    let x=(model*vec4<f32>(b-a,0.0)).xyz;
    let y=(model*vec4<f32>(c-a,0.0)).xyz;
    let n=cross(x,y);return n/max(length(n),0.00001);
}

fn template_position(v:VoxelVertex,height:f32)->vec3<f32> {return vec3<f32>(v.position_x,v.position_y*height,v.position_z);}

@vertex
fn vs_generated_voxel(@builtin(vertex_index) index:u32, @builtin(instance_index) instance_index:u32)->VoxelOutput {
    let physical_slot = draw_slot_indices[instance_index];
    let instance = instances[physical_slot];
    // The normal matrix is unused by unlit voxel expansion. Two exact
    // numeric halves preserve a u32 page index across DX11/GL f32 loads.
    let template_base = u32(instance.normal_matrix[0].y)
        | (u32(instance.normal_matrix[0].z) << 16u);
    let origin = bitcast<vec4<f32>>(generated_templates[template_base].info);
    let size = generated_templates[template_base+1u].info;
    let base=instance.vertex_base & 0x7fffffffu;
    let descriptor=generated_faces[base+index/6u];
    let corners=array<u32,6>(0u,1u,2u,0u,2u,3u);
    let face=generated_templates[template_base+2u+descriptor.model_face];
    var vertex=face.vertices[corners[index%6u]];
    let merged=descriptor.cell!=0xffffffffu && (descriptor.cell&0x80000000u)!=0u;
    let cell=descriptor.cell&0x7fffffffu;
    // Only tagged rectangles interpret the third descriptor word as two
    // packed u8 extents. Regular instance height is still a true f32.
    let packed=u32(descriptor.height);
    let span_u=select(1u,packed&255u,merged);
    let span_v=select(1u,(packed>>8u)&255u,merged);
    let model_height=select(descriptor.height,1.0,merged);
    var position=vec3<f32>(0.0);
    if(descriptor.cell!=0xffffffffu){
        position=origin.xyz+vec3<f32>(f32(cell%size.x),f32(cell/size.x%size.y),f32(cell/size.x/size.y));
    }
    if(merged){
        // Unit-cube face tangents: X-normal=>Z/Y, Y-normal=>X/Z,
        // Z-normal=>X/Y. Scale both tangents without stretching texture
        // texels; existing face UV rotation or mirroring stays intact.
        let delta=bitcast<i32>(face.info.y);
        let along_z=abs(delta)==1;
        let normal_y=abs(delta)==i32(size.x);
        var uv_u=vec2<f32>(0.0);
        var uv_v=vec2<f32>(0.0);
        for(var i=0u;i<4u;i++){
            let corner=face.vertices[i];
            let u=select(corner.position_x,corner.position_z,along_z);
            let v=select(corner.position_y,corner.position_z,normal_y);
            uv_u+=corner.uv*select(-0.5,0.5,u>=0.5);
            uv_v+=corner.uv*select(-0.5,0.5,v>=0.5);
        }
        let u=select(vertex.position_x,vertex.position_z,along_z);
        let v=select(vertex.position_y,vertex.position_z,normal_y);
        let extend_u=f32(span_u-1u)*u;
        let extend_v=f32(span_v-1u)*v;
        vertex.uv+=uv_u*extend_u+uv_v*extend_v;
        if(along_z){vertex.position_z+=extend_u;}
        else{vertex.position_x+=extend_u;}
        if(normal_y){vertex.position_z+=extend_v;}
        else{vertex.position_y+=extend_v;}
    }
    vertex.position_x+=position.x;
    vertex.position_y=vertex.position_y*model_height+position.y;
    vertex.position_z+=position.z;
    var shaded=shade_voxel_value(vertex,instance,physical_slot);
    shaded.merged_run=select(1u,2u,merged);
    let a=template_position(face.vertices[0],model_height);
    let b=template_position(face.vertices[1],model_height);
    let c=template_position(face.vertices[2],model_height);
    if(frame.voxel_settings.y==2.0){shaded.surface_normal=quad_normal(a,b,c,instance.model);}
    shaded.quad_uv=quad_coordinates(corners[index%6u]);
    if(frame.voxel_settings.y!=1.0 && size.w!=0u){
        var light=vec2<f32>(0.0);
        if(descriptor.cell==0xffffffffu){
            let center=(a+b+c+template_position(face.vertices[3],model_height))*0.25;
            let normal=cross(b-a,c-a);let n=normal/max(length(normal),0.00001)*0.001;
            light=max(static_levels(center-n,origin,size),static_levels(center+n,origin,size));
        }else{
            let delta=bitcast<i32>(face.info.y);
            let local=template_position(face.vertices[corners[index%6u]],model_height);
            light=corner_cell_light(cell,delta,local,size);
        }
        let sky=select(1.0,frame.voxel_settings.z,frame.voxel_settings.w!=0.0);
        let brightness=max(light.x,light.y*sky);
        let factor=origin.w+(1.0-origin.w)*brightness*brightness;
        shaded.vertex_color=vec4<f32>(shaded.vertex_color.rgb*factor,shaded.vertex_color.a);
    }
    if(frame.voxel_settings.y!=1.0 && descriptor.padding!=0u){
        let ao=f32((descriptor.padding>>(corners[index%6u]*8u))&255u)/255.0;
        shaded.vertex_color=vec4<f32>(shaded.vertex_color.rgb*ao,shaded.vertex_color.a);
    }
    return shaded;
}

@vertex
fn vs_voxel(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VoxelOutput {
    let slot = draw_slot_indices[instance_index];
    return shade_voxel(vertex_index, slot, instances[slot]);
}

@vertex
fn vs_voxel_gpu(@builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) visible_index: u32) -> VoxelOutput {
    // Direct indexed ranges keep their original instance index. Only procedural
    // quad batches reserve this bit and consume the compute-compacted instance list.
    // Every slot within a quad batch has that bit set before compaction.
    let direct_slot = draw_slot_indices[visible_index];
    let direct_instance = instances[direct_slot];
    if ((direct_instance.vertex_base & 0x80000000u) == 0u) {
        return shade_voxel(vertex_index, direct_slot, direct_instance);
    }
    let instance_index = draw_slot_indices[visible_instances[visible_index]];
    let instance = instances[instance_index];
    // normal_matrix is unused by the unlit voxel shader; its first word carries
    // the real vertex count for variable-size parts in a shared indirect batch.
    let vertex_count = bitcast<u32>(instance.normal_matrix[0].x);
    if (vertex_index >= vertex_count) {
        var clipped: VoxelOutput;
        clipped.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return clipped;
    }
    return shade_voxel(vertex_index, instance_index, instance);
}

// Forward one loaded physical instance to vertex expansion, transform and
// normal calculation. Indirect batches previously fetched the same 144-byte
// instance row separately for vertex count, base address and transformation.
fn shade_voxel(vertex_index: u32, instance_index: u32, instance:Instance) -> VoxelOutput {
    let quad_expansion = (instance.vertex_base & 0x80000000u) != 0u;
    let base = instance.vertex_base & 0x7fffffffu;
    let corners = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    let local_vertex = select(
        vertex_index,
        (vertex_index / 6u) * 4u + corners[vertex_index % 6u],
        quad_expansion,
    );
    let vertex = vertices[local_vertex + base];
    var shaded=shade_voxel_value(vertex,instance,instance_index);
    shaded.quad_uv=quad_coordinates(local_vertex%4u);
    if(quad_expansion && frame.voxel_settings.y==2.0){
        let q=local_vertex/4u*4u+base;
        shaded.surface_normal=quad_normal(template_position(vertices[q],1.0),template_position(vertices[q+1u],1.0),template_position(vertices[q+2u],1.0),instance.model);
    }
    return shaded;
}

fn shade_voxel_value(vertex:VoxelVertex,instance:Instance,instance_index:u32)->VoxelOutput {
    let world_position = instance.model * vec4<f32>(
        vertex.position_x, vertex.position_y, vertex.position_z, 1.0,
    );
    let clip_position = frame.view_projection * world_position;
    // Algebraically combine the element viewport and render-target transforms
    // in clip space. For ordinary vertices (|w| >= epsilon), the previous path
    // divided by clip.w only to multiply by clip.w again. Avoid three per-vertex
    // perspective divisions, and preserve per-instance pixel/depth offsets.
    let target_size = max(frame.render_target_size, vec2<f32>(1.0));
    let viewport_scale = frame.element_bounds_size / target_size;
    let viewport_center = frame.element_bounds_origin
        + frame.element_bounds_size * 0.5 + instance.pixel_offset;
    let viewport_bias = (viewport_center / target_size) * vec2<f32>(2.0, -2.0)
        + vec2<f32>(-1.0, 1.0);
    var clip_target: vec4<f32>;
    if (abs(clip_position.w) < 0.0001) {
        // Keep the original safe-divide behavior for vertices at the eye plane.
        // A direct clip-space transform is not equivalent when w is clamped.
        let safe_clip_w = select(-0.0001, 0.0001, clip_position.w >= 0.0);
        let ndc = clip_position.xyz / safe_clip_w;
        let screen = frame.element_bounds_origin
            + (ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5))
                * frame.element_bounds_size + instance.pixel_offset;
        let normalized = screen / target_size;
        clip_target = vec4<f32>(
            (normalized * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0))
                * clip_position.w,
            (ndc.z - instance.depth_bias) * clip_position.w,
            clip_position.w,
        );
    } else {
        clip_target = vec4<f32>(
            clip_position.xy * viewport_scale + viewport_bias * clip_position.w,
            clip_position.z - instance.depth_bias * clip_position.w,
            clip_position.w,
        );
    }

    var output: VoxelOutput;
    output.draw_index = instance_index;
    output.position = clip_target;
    output.vertex_color = vec4<f32>(
        f32(vertex.color_rgba8 & 255u),
        f32((vertex.color_rgba8 >> 8u) & 255u),
        f32((vertex.color_rgba8 >> 16u) & 255u),
        f32((vertex.color_rgba8 >> 24u) & 255u),
    ) / 255.0;
    output.uv = vertex.uv;
    // Fog is usually disabled; avoid a square-root per vertex when unused.
    output.fog_distance = 0.0;
    if (frame.fog_color.a > 0.0 && frame.shadow_settings.w <= 0.5) {
        output.fog_distance = distance(world_position.xyz, frame.camera_position);
    }
    output.world_position=world_position.xyz;output.surface_normal=vec3<f32>(0.0);output.quad_uv=vec2<f32>(0.0);
    output.merged_run=1u;
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

// Material parameters are fetched once at each fragment entry point and
// threaded through alpha, material, lighting and capture helpers. Otherwise
// nested helpers can each independently load the same 80-byte DrawParams row.
// Repeat only merged full-cube UVs. Keep pre-fracture derivatives to avoid
// discontinuous mip selection at individual Minecraft texture cell seams.
fn voxel_tiled_uv(input:VoxelOutput)->vec2<f32> {
    return select(input.uv,fract(input.uv),input.merged_run>1u);
}
fn voxel_color(input:VoxelOutput,draw:DrawParams)->vec4<f32> {
    var color = draw.base_color * input.vertex_color;
    if (frame.voxel_settings.x==0.0 && (draw.texture_flags & TEXTURE_FLAG_ALBEDO) != 0u) {
        var texel=vec4<f32>(1.0);
        if ((draw.texture_flags & TEXTURE_FLAG_UV) != 0u) {
            texel = textureSampleGrad(albedo_texture,material_sampler,voxel_tiled_uv(input),dpdx(input.uv),dpdy(input.uv));
        } else {
            texel = textureSample(albedo_texture, material_sampler, vec2<f32>(0.5, 0.5));
        }
        if((draw.texture_flags&64u)!=0u && frame.shadow_settings.w>2.5){texel=vec4<f32>(1.0,1.0,1.0,texel.a);}
        color*=texel;
    }
    return color;
}
fn voxel_fragment(input:VoxelOutput,draw:DrawParams)->vec4<f32> {
    let color=voxel_color(input,draw);
    let alpha = clamp(color.a, 0.0, 1.0);
    if (draw.alpha_mode == ALPHA_MODE_MASK && alpha < draw.alpha_cutoff) {
        discard;
    }
    var fog = 0.0;
    if (frame.fog_color.a > 0.0 && frame.shadow_settings.w <= 0.5) {
        fog = clamp((input.fog_distance - frame.fog_range.x)
            / max(frame.fog_range.y - frame.fog_range.x, 0.0001), 0.0, 1.0)
            * frame.fog_color.a;
    }
    var lit=color.rgb+draw.emissive.rgb*draw.emissive.a;
    if(frame.voxel_settings.y==2.0){lit=shade_surface(input,draw,color.rgb);}
    var rgb = mix(lit, frame.fog_color.rgb, fog);
    if(frame.voxel_settings.x==2.0){
        let edge=min(input.quad_uv,vec2<f32>(1.0)-input.quad_uv)/max(fwidth(input.quad_uv),vec2<f32>(0.00001));
        // Retain full face depth: discarding interiors exposes every deeper layer's edges.
        let line=1.0-smoothstep(0.7,1.2,min(edge.x,edge.y));
        rgb=mix(frame.fog_color.rgb,rgb,line);
    }
    if (draw.alpha_mode == ALPHA_MODE_BLEND) {
        let coverage = element_bounds_coverage(input.position.xy);
        let output_alpha = alpha * coverage;
        return vec4<f32>(rgb * output_alpha, output_alpha);
    }
    return vec4<f32>(rgb, 1.0);
}
@fragment fn fs_voxel(input:VoxelOutput)->@location(0) vec4<f32>{
    return voxel_fragment(input,draws[input.draw_index]);
}
@fragment fn fs_voxel_capture(input:VoxelOutput)->@location(0) vec4<f32>{
    let draw=draws[input.draw_index];
    if(draw.alpha_mode==ALPHA_MODE_BLEND && (draw.texture_flags&192u)!=0u){discard;}
    return voxel_fragment(input,draw);
}
fn test_voxel_alpha(input:VoxelOutput,draw:DrawParams)->vec4<f32> {
    let color=voxel_color(input,draw);
    if(draw.alpha_mode==ALPHA_MODE_MASK && color.a<draw.alpha_cutoff || color.a<0.001){discard;}
    return color;
}
fn encode_normal(n:vec3<f32>)->vec2<f32>{
    let p=n/(abs(n.x)+abs(n.y)+abs(n.z));
    return select(p.xy,(vec2<f32>(1.0)-abs(p.yx))*select(vec2<f32>(-1.0),vec2<f32>(1.0),p.xy>=vec2<f32>(0.0)),p.z<0.0);
}
@fragment fn fs_voxel_surface(input:VoxelOutput)->@location(0) vec4<f32>{
    let draw=draws[input.draw_index];
    let color=test_voxel_alpha(input,draw);
    var roughness=draw.roughness;var metallic=draw.metallic;
    if((draw.texture_flags&8u)!=0u){let p=textureSampleGrad(material_texture,material_sampler,voxel_tiled_uv(input),dpdx(input.uv),dpdy(input.uv));if((draw.texture_flags&16u)!=0u){roughness=p.b;metallic=p.r;}else{roughness*=p.g;metallic*=p.b;}}
    // Coverage does not imply transmission: ordinary alpha-blended surfaces stay diffuse.
    let blended=draw.alpha_mode==ALPHA_MODE_BLEND;
    var kind=select(metallic,6.0+metallic,blended);
    if(blended && (draw.texture_flags&128u)!=0u){kind=4.0+metallic;}
    if(blended && (draw.texture_flags&64u)!=0u){kind=2.0+metallic;}
    return vec4<f32>(encode_normal(surface_normal(input,draw)),clamp(roughness,0.06,1.0),kind);
}
@fragment fn fs_voxel_albedo(input:VoxelOutput)->@location(0) vec4<f32>{
    let draw=draws[input.draw_index];
    let color=test_voxel_alpha(input,draw);
    let base=select(color.rgb,srgb_linear(color.rgb),frame.sun_color.w>0.5);
    return vec4<f32>(base,clamp(color.a,0.0,1.0));
}
struct VoxelAttributeTargets {
    @location(0) surface:vec4<f32>,
    @location(1) albedo:vec4<f32>,
};
@fragment fn fs_voxel_attributes_mrt(input:VoxelOutput)->VoxelAttributeTargets {
    let draw=draws[input.draw_index];
    let color=test_voxel_alpha(input,draw);
    var roughness=draw.roughness;var metallic=draw.metallic;
    if((draw.texture_flags&8u)!=0u){
        let p=textureSampleGrad(material_texture,material_sampler,voxel_tiled_uv(input),dpdx(input.uv),dpdy(input.uv));
        if((draw.texture_flags&16u)!=0u){roughness=p.b;metallic=p.r;}
        else{roughness*=p.g;metallic*=p.b;}
    }
    let blended=draw.alpha_mode==ALPHA_MODE_BLEND;
    var kind=select(metallic,6.0+metallic,blended);
    if(blended && (draw.texture_flags&128u)!=0u){kind=4.0+metallic;}
    if(blended && (draw.texture_flags&64u)!=0u){kind=2.0+metallic;}
    let surface=vec4<f32>(encode_normal(surface_normal(input,draw)),clamp(roughness,0.06,1.0),kind);
    let base=select(color.rgb,srgb_linear(color.rgb),frame.sun_color.w>0.5);
    return VoxelAttributeTargets(surface,vec4<f32>(base,clamp(color.a,0.0,1.0)));
}
@fragment fn fs_voxel_shadow(input:VoxelOutput)->@location(0) vec4<f32>{
    let draw=draws[input.draw_index];
    if(draw.alpha_mode==ALPHA_MODE_BLEND){discard;}
    let color=test_voxel_alpha(input,draw);return vec4<f32>(1.0);
}
