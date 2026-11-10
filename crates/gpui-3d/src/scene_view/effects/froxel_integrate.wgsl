// GPU-only front-to-back integration: one thread owns each screen-space
// froxel column and writes every depth slice in order, avoiding global
// inter-slice atomics or a CPU-controlled prefix scan.
const FROXEL_NEAR:f32 = 0.1;
const FROXEL_FAR:f32 = 512.0;
@group(0) @binding(0) var injected:texture_3d<f32>;
@group(0) @binding(1) var integrated:texture_storage_3d<rgba16float,write>;

@compute @workgroup_size(8,8,1)
fn cs_froxel_integrate(@builtin(global_invocation_id) id:vec3<u32>){
    let dimensions = textureDimensions(integrated);
    if(id.x >= dimensions.x || id.y >= dimensions.y || id.z != 0u){return;}
    var transmittance = 1.0;
    var radiance = vec3<f32>(0.0);
    let ln_near = log(FROXEL_NEAR);
    let ln_far = log(FROXEL_FAR);
    var previous_depth = exp(ln_near);
    for(var slice=0u; slice<dimensions.z; slice++){
        let z1 = f32(slice+1u)/f32(dimensions.z);
        let depth = exp(mix(ln_near,ln_far,z1));
        let segment_length = depth - previous_depth;
        previous_depth = depth;
        let source = textureLoad(injected,vec3<i32>(vec2<i32>(id.xy),i32(slice)),0);
        let extinction = exp(-max(source.a,0.0)*segment_length);
        radiance += transmittance*(1.0-extinction)*source.rgb;
        transmittance *= extinction;
        textureStore(integrated,vec3<i32>(vec2<i32>(id.xy),i32(slice)),
                     vec4<f32>(radiance,transmittance));
    }
}
