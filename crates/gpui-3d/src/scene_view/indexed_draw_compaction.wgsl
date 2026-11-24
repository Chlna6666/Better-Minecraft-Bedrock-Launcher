// Stable indexed draw compaction. Source is the portable 20-byte ABI;
// Vulkan writes 20 bytes, DX12 writes 32-byte root-constant records.
struct IndexedArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: u32,
    first_instance: u32,
};
@group(0) @binding(0) var<storage, read> source: array<IndexedArgs>;
@group(0) @binding(1) var<storage, read> metadata: array<u32>;
@group(0) @binding(2) var<storage, read_write> output: array<u32>;
@group(0) @binding(3) var<storage, read_write> counts: array<u32>;
var<workgroup> prefix: array<u32, 64>;

@compute @workgroup_size(64)
fn cs_compact_indexed_draws(@builtin(workgroup_id) group: vec3<u32>,
                            @builtin(local_invocation_index) lane: u32) {
    let header = group.x * 4u;
    let run = vec4<u32>(metadata[header], metadata[header + 1u],
                        metadata[header + 2u], metadata[header + 3u]);
    var records: array<IndexedArgs, 4>;
    var local_count = 0u;
    for (var i = 0u; i < 4u; i += 1u) {
        let index = lane * 4u + i;
        if (index < run.y) {
            let argument = source[metadata[run.x + index]];
            if (argument.index_count != 0u && argument.instance_count != 0u) {
                records[local_count] = argument;
                local_count += 1u;
            }
        }
    }
    prefix[lane] = local_count;
    workgroupBarrier();
    for (var stride = 1u; stride < 64u; stride *= 2u) {
        var before = 0u;
        if (lane >= stride) { before = prefix[lane - stride]; }
        workgroupBarrier();
        prefix[lane] += before;
        workgroupBarrier();
    }
    let destination = run.z + prefix[lane] - local_count;
    for (var i = 0u; i < local_count; i += 1u) {
        let argument = records[i];
        if (run.w == 2u) {
            let target = (destination + i) * 8u;
            output[target] = argument.base_vertex;
            output[target + 1u] = argument.first_instance;
            output[target + 2u] = argument.index_count;
            output[target + 3u] = argument.instance_count;
            output[target + 4u] = argument.first_index;
            output[target + 5u] = 0u;
            output[target + 6u] = 0u;
            output[target + 7u] = 0u;
        } else {
            let target = (destination + i) * 5u;
            output[target] = argument.index_count;
            output[target + 1u] = argument.instance_count;
            output[target + 2u] = argument.first_index;
            output[target + 3u] = argument.base_vertex;
            output[target + 4u] = argument.first_instance;
        }
    }
    if (lane == 63u) { counts[group.x] = prefix[lane]; }
}
