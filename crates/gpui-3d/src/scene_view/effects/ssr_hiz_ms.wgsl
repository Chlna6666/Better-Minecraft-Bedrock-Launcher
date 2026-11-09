// First reduction of a 4x multisampled opaque depth buffer. Each output
// texel conservatively covers every sample in its 2x2 input footprint.
@group(0) @binding(0) var source_depth_ms:texture_multisampled_2d<f32>;
struct HiZVertexMs { @builtin(position) clip_position:vec4<f32> };
@fragment fn fs_ssr_hiz_ms(input:HiZVertexMs)->@location(0) f32 {
    let size=vec2<i32>(textureDimensions(source_depth_ms));
    let p=vec2<i32>(input.clip_position.xy)*2;
    let taps=vec4<i32>(
        min(p.x,size.x-1),min(p.y,size.y-1),
        min(p.x+1,size.x-1),min(p.y+1,size.y-1)
    );
    var nearest=1.0;
    for(var sample=0;sample<textureNumSamples(source_depth_ms);sample++){
        nearest=min(nearest,textureLoad(source_depth_ms,taps.xy,sample).r);
        nearest=min(nearest,textureLoad(source_depth_ms,taps.zy,sample).r);
        nearest=min(nearest,textureLoad(source_depth_ms,taps.xw,sample).r);
        nearest=min(nearest,textureLoad(source_depth_ms,taps.zw,sample).r);
    }
    return nearest;
}
