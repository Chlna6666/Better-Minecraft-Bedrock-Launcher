// Device-neutral portable DrawIndirect ABI -> DX12 root constants + draw args.
// Source: [vertex_count, instance_count, first_vertex, first_instance].
// Native: [first_vertex, first_instance, vertex_count, instance_count, 0, 0, 0, 0].
// The pipeline consumes this as two u32 root constants followed by
// D3D12_DRAW_ARGUMENTS; the remaining words are record padding.
struct PackParams {
    source_word: u32,
    destination_word: u32,
    command_count: u32,
    indexed: u32,
};
@group(0) @binding(0) var<storage, read> portable: array<u32>;
@group(0) @binding(1) var<storage, read_write> native_args: array<u32>;
@group(0) @binding(2) var<uniform> params: PackParams;

@compute @workgroup_size(64, 1, 1)
fn cs_pack_native_indirect(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index = gid.x;
    if (index >= params.command_count) {
        return;
    }
    let source = params.source_word + index * select(4u, 5u, params.indexed != 0u);
    let dest_word = params.destination_word + index * 8u;
    // Indexed base vertex is passed in the shader offset word, as in direct
    // indexed draws. The IA arguments retain first_index and use zero offsets.
    native_args[dest_word] = portable[source + select(2u, 3u, params.indexed != 0u)];
    native_args[dest_word + 1u] = portable[source + select(3u, 4u, params.indexed != 0u)];
    native_args[dest_word + 2u] = portable[source];
    native_args[dest_word + 3u] = portable[source + 1u];
    native_args[dest_word + 4u] = select(0u, portable[source + 2u], params.indexed != 0u);
    native_args[dest_word + 5u] = 0u;
    native_args[dest_word + 6u] = 0u;
    native_args[dest_word + 7u] = 0u;
}
