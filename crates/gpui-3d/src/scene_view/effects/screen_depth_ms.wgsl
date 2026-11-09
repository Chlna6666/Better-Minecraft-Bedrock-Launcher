// Multisampled depth inputs for the screen effects shader.
// Sample zero is read through textureLoad, the only legal access for multisampled textures.
@group(0) @binding(1) var opaque_depth:texture_multisampled_2d<f32>;
@group(0) @binding(3) var surface_depth:texture_multisampled_2d<f32>;
@group(0) @binding(4) var albedos:texture_multisampled_2d<f32>;
// Resolve multi-sample albedo in screen space, retaining coverage without a
// second geometry pass or an extra native color-resolve attachment.
fn load_material_albedo(at:vec2<i32>)->vec4<f32>{
    let count=textureNumSamples(albedos);
    var color=vec4<f32>(0.0);
    for(var sample=0u;sample<count;sample++){
        color+=textureLoad(albedos,at,i32(sample));
    }
    return color/f32(max(count,1u));
}
