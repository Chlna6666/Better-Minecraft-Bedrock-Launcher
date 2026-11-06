// One workgroup per resident batch. Opaque draws sharing page/material/size class
// are compacted into one indirect instance list; blended draws remain singletons.
struct Bounds {
    bounds_min: vec3<f32>,
    vertex_count: u32,
    bounds_max: vec4<f32>,
    origin: vec4<f32>,
};
struct Arguments { vertex_count: atomic<u32>, instance_count: atomic<u32>, first_vertex: u32, first_instance: u32 };
struct Frame { view_projection: mat4x4<f32> };
// minimum.w = 1 when Hi-Z has resident occluders, 0 when it is empty.
struct CullRange { minimum: vec4<f32>, maximum: vec4<f32> };
@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> bounds: array<Bounds>;
@group(0) @binding(2) var<storage, read_write> arguments: array<Arguments>;
@group(0) @binding(3) var<storage, read_write> visible_instances: array<u32>;
@group(0) @binding(4) var<storage, read> batches: array<vec4<u32>>;
@group(0) @binding(5) var<uniform> range: CullRange;
@group(0) @binding(6) var<storage, read> generated: array<vec4<u32>>;
@group(0) @binding(7) var<storage, read> hiz: array<u32>;
// The CPU publishes only 4-byte logical order changes, while resident Bounds
// remain in stable physical GPU slots across Section inserts and deletions.
@group(0) @binding(8) var<storage, read> logical_to_physical: array<u32>;
var<workgroup> planes: array<vec4<f32>, 6>;
var<workgroup> visible_count: atomic<u32>;
var<workgroup> largest_mesh: atomic<u32>;

fn is_occluded(box: Bounds) -> bool {
    if (box.bounds_max.w != 0.0) { return false; }
    var lo=vec2<f32>(1.0);var hi=vec2<f32>(0.0);var nearest=1.0;
    for(var i=0u;i<8u;i++){
        let p=select(box.bounds_min.xyz,box.bounds_max.xyz,vec3<bool>((i&1u)!=0u,(i&2u)!=0u,(i&4u)!=0u));
        let clip=frame.view_projection*vec4<f32>(p,1.0);
        if(clip.w<=0.0001 || clip.z<0.0){return false;}
        let ndc=clip.xyz/clip.w;let uv=clamp(ndc.xy*vec2<f32>(0.5,-0.5)+vec2<f32>(0.5),vec2<f32>(0.0),vec2<f32>(1.0));
        lo=min(lo,uv);hi=max(hi,uv);nearest=min(nearest,ndc.z);
    }
    var size=128u;var base=0u;
    for(var level=0u;level<7u;level++){
        if(all(floor(hi*f32(size))-floor(lo*f32(size))<=vec2<f32>(1.0))){break;}
        base+=size*size;size/=2u;
    }
    let a=min(vec2<u32>(lo*f32(size)),vec2<u32>(size-1u));
    let b=min(vec2<u32>(hi*f32(size)),vec2<u32>(size-1u));
    let depth=max(max(hiz[base+a.y*size+a.x],hiz[base+a.y*size+b.x]),
        max(hiz[base+b.y*size+a.x],hiz[base+b.y*size+b.x]));
    return nearest>bitcast<f32>(depth)+0.00001;
}

fn is_visible(box: Bounds) -> bool {
    if (box.origin.w != 0.0 &&
        (any(box.origin.xyz < range.minimum.xyz) || any(box.origin.xyz >= range.maximum.xyz))) {
        return false;
    }
    for (var plane = 0u; plane < 6u; plane++) {
        let normal = planes[plane];
        let positive = select(box.bounds_min.xyz, box.bounds_max.xyz, normal.xyz >= vec3<f32>(0.0));
        if (dot(normal.xyz, positive) + normal.w < 0.0) { return false; }
    }
    // Empty Hi-Z means no occluder can reject a draw. Preserve frustum/range
    // tests while avoiding eight clip transforms and four depth reads per draw.
    if (range.minimum.w == 0.0) { return true; }
    return !is_occluded(box);
}

@compute @workgroup_size(64)
fn cs_cull(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) lane: u32,
    @builtin(num_workgroups) grid: vec3<u32>) {
    let index = group.x + group.y * grid.x;
    if (index >= arrayLength(&batches)) { return; }
    let batch = batches[index];
    if (lane == 0u) {
        atomicStore(&visible_count, 0u);
        atomicStore(&largest_mesh, 0u);
        let matrix = frame.view_projection;
        let row0 = vec4<f32>(matrix[0].x, matrix[1].x, matrix[2].x, matrix[3].x);
        let row1 = vec4<f32>(matrix[0].y, matrix[1].y, matrix[2].y, matrix[3].y);
        let row2 = vec4<f32>(matrix[0].z, matrix[1].z, matrix[2].z, matrix[3].z);
        let row3 = vec4<f32>(matrix[0].w, matrix[1].w, matrix[2].w, matrix[3].w);
        planes = array<vec4<f32>, 6>(row3 + row0, row3 - row0, row3 + row1,
            row3 - row1, row2, row3 - row2);
    }
    // Generated meshes and blended ranges are singletons: one lane can publish
    // their result without workgroup synchronization or an instance-list atomic.
    if (batch.y == 1u) {
        if (lane == 0u) {
            let resident = bounds[logical_to_physical[batch.w]];
            var source = vec4<u32>(resident.vertex_count, 1u, batch.z, batch.w);
            if (batch.x != 0u) { source = generated[index]; }
            let visible = is_visible(resident);
            if (visible && batch.x == 0u) { visible_instances[batch.w] = batch.w; }
            atomicStore(&arguments[index].vertex_count, select(0u, source.x, visible));
            atomicStore(&arguments[index].instance_count, select(0u, source.y, visible));
            arguments[index].first_vertex = source.z;
            arguments[index].first_instance = source.w;
        }
        return;
    }
    workgroupBarrier();
    var vertex_count = 0u;
    for (var local = lane; local < batch.y; local += 64u) {
        let instance = batch.w + local;
        let resident = bounds[logical_to_physical[instance]];
        if (is_visible(resident)) {
            vertex_count = max(vertex_count, resident.vertex_count);
            let slot = atomicAdd(&visible_count, 1u);
            visible_instances[batch.w + slot] = instance;
        }
    }
    // Reduce in workgroup memory; only one lane publishes the indirect record.
    if (vertex_count != 0u) {
        atomicMax(&largest_mesh, vertex_count);
    }
    workgroupBarrier();
    if (lane == 0u) {
        atomicStore(&arguments[index].vertex_count, atomicLoad(&largest_mesh));
        atomicStore(&arguments[index].instance_count, atomicLoad(&visible_count));
        arguments[index].first_vertex = batch.z;
        arguments[index].first_instance = batch.w;
    }
}


// Generated Sections generally produce independent singleton batches.
// Instead of launching one 64-lane workgroup per Section/material part and
// leaving 63 lanes idle, assign one resident batch to each lane. The output
// ABI is identical to cs_cull and preserves original logical draw ordering.
//
// The number of *active* batches is stored numerically in range.maximum.w,
// because the GPU allocation is capacity-sized and may contain stale entries.
// A float represents practical streamed-world counts exactly (< 2^24).
@compute @workgroup_size(64)
fn cs_cull_singletons(
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(local_invocation_index) lane: u32,
    @builtin(num_workgroups) grid: vec3<u32>,
) {
    if (lane == 0u) {
        let matrix = frame.view_projection;
        let row0 = vec4<f32>(matrix[0].x, matrix[1].x, matrix[2].x, matrix[3].x);
        let row1 = vec4<f32>(matrix[0].y, matrix[1].y, matrix[2].y, matrix[3].y);
        let row2 = vec4<f32>(matrix[0].z, matrix[1].z, matrix[2].z, matrix[3].z);
        let row3 = vec4<f32>(matrix[0].w, matrix[1].w, matrix[2].w, matrix[3].w);
        planes = array<vec4<f32>, 6>(
            row3 + row0, row3 - row0, row3 + row1,
            row3 - row1, row2, row3 - row2,
        );
    }
    workgroupBarrier();
    let index = (group.x + group.y * grid.x) * 64u + lane;
    if (index >= u32(range.maximum.w)) {
        return;
    }
    let batch = batches[index];
    // The CPU selects this entry point only if all batch.instance_count == 1.
    let resident = bounds[logical_to_physical[batch.w]];
    var source = vec4<u32>(resident.vertex_count, 1u, batch.z, batch.w);
    if (batch.x != 0u) {
        source = generated[index];
    }
    let visible = is_visible(resident);
    if (visible && batch.x == 0u) {
        visible_instances[batch.w] = batch.w;
    }
    atomicStore(&arguments[index].vertex_count, select(0u, source.x, visible));
    atomicStore(&arguments[index].instance_count, select(0u, source.y, visible));
    arguments[index].first_vertex = source.z;
    arguments[index].first_instance = source.w;
}
