// A lane owns four consecutive input records. The inclusive workgroup
// prefix over lane-local counts produces a stable compacted sequence.
// Each run has its own disjoint output capacity and count word.
@group(0) @binding(0) var<storage, read> source: array<vec4<u32>>;
// Four u32 header words per run, followed by the immutable gather list.
@group(0) @binding(1) var<storage, read> metadata: array<u32>;
@group(0) @binding(2) var<storage, read_write> output: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read_write> counts: array<u32>;
var<workgroup> prefix: array<u32, 64>;

@compute @workgroup_size(64)
fn cs_compact_draws(@builtin(workgroup_id) group: vec3<u32>,
                    @builtin(local_invocation_index) lane: u32) {
    let header = group.x * 4u;
    let run = vec4<u32>(metadata[header], metadata[header + 1u],
                        metadata[header + 2u], metadata[header + 3u]);
    var records: array<vec4<u32>, 4>;
    var local_count = 0u;
    for (var i = 0u; i < 4u; i += 1u) {
        let index = lane * 4u + i;
        if (index < run.y) {
            let argument = source[metadata[run.x + index]];
            if (argument.x != 0u && argument.y != 0u) {
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
            // Naga DX12 root constants followed by DRAW_ARGUMENTS and padding.
            // Match native_indirect_pack.wgsl without another dispatch.
            let target = (destination + i) * 2u;
            output[target] = vec4<u32>(argument.z, argument.w, argument.x, argument.y);
            output[target + 1u] = vec4<u32>(0u);
        } else {
            output[destination + i] = argument;
        }
    }
    if (lane == 63u) { counts[group.x] = prefix[lane]; }
}
