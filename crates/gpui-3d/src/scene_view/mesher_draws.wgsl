@group(0) @binding(0) var<storage,read> source:array<u32>;
@group(0) @binding(1) var<storage,read> mapping:array<vec4<u32>>;
@group(0) @binding(2) var<storage,read_write> destination:array<u32>;
@compute @workgroup_size(64)
fn cs_mesh_draws(@builtin(workgroup_id) group:vec3<u32>, @builtin(num_workgroups) groups:vec3<u32>, @builtin(local_invocation_index) lane:u32){
 let index=(group.y*groups.x+group.x)*64u+lane;
 if(index>=arrayLength(&mapping)){return;}
 let map=mapping[index]; let offset=map.y*4u;
 destination[offset]=source[map.x*4u]; destination[offset+1u]=1u;
 destination[offset+2u]=0u; destination[offset+3u]=map.z;
}
