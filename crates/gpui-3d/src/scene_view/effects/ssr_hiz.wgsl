// GPU screen-depth reduction for hierarchical screen-space reflections.
// Every mip stores the *minimum* (nearest) opaque NDC depth in its footprint.
// A ray known to be closer than this minimum cannot intersect that tile.
@group(0) @binding(0) var source_depth: texture_2d<f32>;
struct HiZVertex { @builtin(position) clip_position:vec4<f32> };
@vertex fn vs_ssr_hiz(@builtin(vertex_index) id:u32)->HiZVertex {
    let uv=vec2<f32>(f32((id<<1u)&2u),f32(id&2u));
    var output:HiZVertex;
    output.clip_position=vec4<f32>(uv*vec2<f32>(2.0,-2.0)+vec2<f32>(-1.0,1.0),0.0,1.0);
    return output;
}
@fragment fn fs_ssr_hiz(input:HiZVertex)->@location(0) f32 {
    let size=vec2<i32>(textureDimensions(source_depth));
    let p=vec2<i32>(input.clip_position.xy)*2;
    let x=min(p.x,size.x-1);
    let y=min(p.y,size.y-1);
    let next_x=min(p.x+1,size.x-1);
    let next_y=min(p.y+1,size.y-1);
    var nearest=min(
        min(textureLoad(source_depth,vec2<i32>(x,y),0).r,
            textureLoad(source_depth,vec2<i32>(next_x,y),0).r),
        min(textureLoad(source_depth,vec2<i32>(x,next_y),0).r,
            textureLoad(source_depth,vec2<i32>(next_x,next_y),0).r)
    );
    // Native texture mip sizes round DOWN. When a level is odd, the final
    // output texel must also cover its third source column/row. Otherwise
    // a tiny far-edge occluder could be missed by the coarse SSR skip test.
    let last_x=(size.x&1)==1 && x>=max(size.x/2-1,0)*2;
    let last_y=(size.y&1)==1 && y>=max(size.y/2-1,0)*2;
    let third_x=min(p.x+2,size.x-1);
    let third_y=min(p.y+2,size.y-1);
    if(last_x){
        nearest=min(nearest,textureLoad(source_depth,vec2<i32>(third_x,y),0).r);
        nearest=min(nearest,textureLoad(source_depth,vec2<i32>(third_x,next_y),0).r);
    }
    if(last_y){
        nearest=min(nearest,textureLoad(source_depth,vec2<i32>(x,third_y),0).r);
        nearest=min(nearest,textureLoad(source_depth,vec2<i32>(next_x,third_y),0).r);
    }
    if(last_x && last_y){
        nearest=min(nearest,textureLoad(source_depth,vec2<i32>(third_x,third_y),0).r);
    }
    return nearest;
}
