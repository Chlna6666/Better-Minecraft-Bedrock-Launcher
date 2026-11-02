@group(0) @binding(0) var<uniform> frame:EffectFrame;
@group(0) @binding(1) var<storage,read> lights:array<EffectLight>;
@group(0) @binding(2) var<storage,read_write> tiles:array<atomic<u32>>;
@compute @workgroup_size(64) fn cs_clear_light_tiles(@builtin(global_invocation_id) id:vec3<u32>){
    if(id.x<frame.light_meta.y*frame.light_meta.z){atomicStore(&tiles[id.x*65u],0u);}
}
@compute @workgroup_size(64) fn cs_light_tiles(@builtin(global_invocation_id) id:vec3<u32>){
    if(id.x>=frame.light_meta.x){return;}let light=lights[id.x];if(light.kind!=2u && light.kind!=3u && light.kind!=4u && light.kind!=5u){return;}
    let center=frame.view_projection*vec4<f32>(light.position,1.0);
    let reach=light.range*(abs(frame.view_projection[0].w)+abs(frame.view_projection[1].w)+abs(frame.view_projection[2].w));
    if(center.w+reach<=0.001){return;}
    var lo=vec2<f32>(1.0);var hi=vec2<f32>(-1.0);var clipped=false;
    for(var corner=0u;corner<8u;corner++){let offset=select(vec3<f32>(-light.range),vec3<f32>(light.range),vec3<bool>((corner&1u)!=0u,(corner&2u)!=0u,(corner&4u)!=0u));let p=light.position+offset;
        let clip=frame.view_projection*vec4<f32>(p,1.0);if(clip.w<=0.001){clipped=true;continue;}let ndc=clip.xy/clip.w;lo=min(lo,ndc);hi=max(hi,ndc);
    }
    if(clipped){lo=vec2<f32>(-1.0);hi=vec2<f32>(1.0);}
    if(any(lo>vec2<f32>(1.0)) || any(hi<vec2<f32>(-1.0))){return;}
    let dims=vec2<f32>(f32(frame.light_meta.y),f32(frame.light_meta.z));
    let lower=vec2<u32>(clamp(vec2<f32>(lo.x,-hi.y)*0.5+0.5,vec2<f32>(0.0),vec2<f32>(0.99999))*dims);
    let upper=vec2<u32>(clamp(vec2<f32>(hi.x,-lo.y)*0.5+0.5,vec2<f32>(0.0),vec2<f32>(0.99999))*dims);
    for(var y=lower.y;y<=upper.y;y++){for(var x=lower.x;x<=upper.x;x++){
        let base=(y*frame.light_meta.y+x)*65u;let index=atomicAdd(&tiles[base],1u);if(index<64u){atomicStore(&tiles[base+1u+index],id.x);}
    }}
}
