// Voxel PBR uses the same Cook-Torrance BRDF as ordinary scene-view materials.
@group(0) @binding(15) var<storage,read> shadow_depths:array<u32>;
fn sun_visibility(position:vec3<f32>,normal:vec3<f32>)->f32 {
    if(frame.shadow_settings.x<=0.0){return 1.0;}
    let clip=frame.shadow_matrix*vec4<f32>(position,1.0);let p=clip.xyz/clip.w;
    let uv=p.xy*vec2<f32>(0.5,-0.5)+vec2<f32>(0.5);
    if(p.z<=0.0 || p.z>=1.0 || any(uv<=vec2<f32>(0.0)) || any(uv>=vec2<f32>(1.0))){return 1.0;}
    let size=i32(frame.shadow_settings.z);let center=vec2<i32>(uv*f32(size));
    let bias=frame.shadow_settings.y*(1.0+3.0*(1.0-max(dot(normal,frame.sun_direction.xyz),0.0)));
    var visible=0.0;
    for(var y=-1;y<=1;y++){for(var x=-1;x<=1;x++){
        let sample=clamp(center+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(size-1));
        let depth=bitcast<f32>(shadow_depths[u32(sample.y*size+sample.x)]);
        visible+=select(0.0,1.0,p.z-bias<=depth);
    }}
    return mix(1.0,visible/9.0,frame.shadow_settings.x);
}
fn srgb_linear(value:vec3<f32>)->vec3<f32> {return select(value/12.92,pow((value+vec3<f32>(0.055))/1.055,vec3<f32>(2.4)),value>vec3<f32>(0.04045));}
fn linear_srgb(value:vec3<f32>)->vec3<f32> {return select(value*12.92,1.055*pow(max(value,vec3<f32>(0.0)),vec3<f32>(1.0/2.4))-vec3<f32>(0.055),value>vec3<f32>(0.0031308));}

fn surface_normal(input:VoxelOutput,draw:DrawParams)->vec3<f32> {
    let geometric=cross(dpdx(input.world_position),dpdy(input.world_position));
    var n=select(geometric,input.surface_normal,length(input.surface_normal)>0.00001);
    n=n/max(length(n),0.00001);
    if(dot(n,frame.camera_position-input.world_position)<0.0){n=-n;}
    if(frame.voxel_settings.x!=0.0 || (draw.texture_flags&32u)==0u){return n;}
    let dp1=dpdx(input.world_position);let dp2=dpdy(input.world_position);
    let du1=dpdx(input.uv);let du2=dpdy(input.uv);
    let det=du1.x*du2.y-du1.y*du2.x;
    if(abs(det)<0.00001){return n;}
    let t=normalize((dp1*du2.y-dp2*du1.y)/det);
    let b=normalize((-dp1*du2.x+dp2*du1.x)/det);
    let mapped=textureSample(normal_texture,material_sampler,input.uv).xyz*2.0-vec3<f32>(1.0);
    n=normalize(t*mapped.x+b*mapped.y+n*mapped.z);
    return n;
}

fn sky_radiance(direction:vec3<f32>)->vec3<f32> {
    let height=clamp(direction.y*0.5+0.5,0.0,1.0);
    return mix(vec3<f32>(0.28,0.36,0.48),vec3<f32>(0.08,0.22,0.5),height)*frame.voxel_settings.z;
}

fn shade_surface(input:VoxelOutput,draw:DrawParams,color:vec3<f32>)->vec3<f32> {
    let n=surface_normal(input,draw);
    let v=normalize(frame.camera_position-input.world_position);
    var metallic=draw.metallic;var roughness=draw.roughness;var emission=0.0;
    if(frame.voxel_settings.x==0.0 && (draw.texture_flags&8u)!=0u){
        let sample=textureSample(material_texture,material_sampler,input.uv);
        if((draw.texture_flags&16u)!=0u){metallic=sample.r;roughness=sample.b;emission=sample.g;}
        else{metallic*=sample.b;roughness*=sample.g;}
    }
    metallic=clamp(metallic,0.0,1.0);roughness=clamp(roughness,0.06,1.0);
    let base_color=clamp(color,vec3<f32>(0.0),vec3<f32>(1.0));
    let base=select(base_color,srgb_linear(base_color),frame.sun_color.w>0.5);
    let sun=frame.sun_direction.xyz;
    let direct=cook_torrance_lighting(base,metallic,roughness,n,v,sun,frame.sun_color.rgb*frame.sun_direction.w*3.0)*sun_visibility(input.world_position,n);
    let f0=mix(vec3<f32>(0.04),base,metallic);
    let ambient=base*(0.22+0.3*frame.voxel_settings.z)*(1.0-metallic);
    var radiance=ambient+direct+base*emission*6.0+draw.emissive.rgb*draw.emissive.a;
    // Display-referred UNORM output. No HDR scene/bloom pass is claimed here.
    if(frame.shadow_settings.w>0.5){return radiance;}
    radiance+=sky_radiance(normalize(mix(reflect(-v,n),n,roughness*roughness)))*environment_reflectance(max(dot(n,v),0.0),roughness,metallic,f0);
    return linear_srgb(radiance/(vec3<f32>(1.0)+radiance));
}
