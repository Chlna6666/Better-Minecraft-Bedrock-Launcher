const PI:f32=3.14159265359;
@group(0) @binding(0) var scene:texture_2d<f32>;
@group(0) @binding(1) var opaque_depth:texture_2d<f32>;
@group(0) @binding(2) var surfaces:texture_2d<f32>;
@group(0) @binding(3) var surface_depth:texture_2d<f32>;
@group(0) @binding(4) var albedos:texture_2d<f32>;
@group(0) @binding(5) var linear_sampler:sampler;
@group(0) @binding(6) var<uniform> frame:EffectFrame;
@group(0) @binding(7) var shadow_depth:texture_2d<f32>;
@group(0) @binding(8) var cloud_texture:texture_2d<f32>;
@group(0) @binding(9) var<storage,read> lights:array<EffectLight>;
@group(0) @binding(10) var<storage,read> tiles:array<u32>;
@group(0) @binding(11) var cloud_sampler:sampler;
struct ScreenOutput {@builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32>};
struct Resolved {@location(0) color:vec4<f32>,@builtin(frag_depth) depth:f32};
@vertex fn vs_effects(@builtin(vertex_index) id:u32)->ScreenOutput{
    let uv=vec2<f32>(f32((id<<1u)&2u),f32(id&2u));var out:ScreenOutput;
    let pixel=frame.bounds.xy+uv*frame.bounds.zw;
    out.position=vec4<f32>(pixel/frame.display_target.xy*vec2<f32>(2.0,-2.0)+vec2<f32>(-1.0,1.0),0.0,1.0);out.uv=uv;return out;
}
fn pixel(uv:vec2<f32>)->vec2<i32>{return vec2<i32>(clamp(uv,vec2<f32>(0.0),vec2<f32>(0.999999))*vec2<f32>(textureDimensions(scene)));}
fn world(uv:vec2<f32>,depth:f32)->vec3<f32>{let p=frame.inverse_view_projection*vec4<f32>(uv*vec2<f32>(2.0,-2.0)+vec2<f32>(-1.0,1.0),depth,1.0);return p.xyz/p.w;}
fn project(p:vec3<f32>)->vec3<f32>{let c=frame.view_projection*vec4<f32>(p,1.0);if(c.w<=0.0){return vec3<f32>(-1.0);}return vec3<f32>(c.xy/c.w*vec2<f32>(0.5,-0.5)+0.5,c.z/c.w);}
fn normal(p:vec2<f32>)->vec3<f32>{var n=vec3<f32>(p,1.0-abs(p.x)-abs(p.y));if(n.z<0.0){n=vec3<f32>((1.0-abs(n.yx))*select(vec2<f32>(-1.0),vec2<f32>(1.0),n.xy>=vec2<f32>(0.0)),n.z);}return normalize(n);}
fn cloud_material(p:vec3<f32>)->vec4<f32>{
    if(frame.flags.w<0.5){return vec4<f32>(0.0);}let absolute=p+frame.origin.xyz;let h=(absolute.y-frame.options.z)/frame.cloud.x;
    if(h<=0.0 || h>=1.0){return vec4<f32>(0.0);}
    let uv=absolute.xz/frame.cloud.z+frame.work.w*frame.cloud.y*vec2<f32>(1.0,0.3)+vec2<f32>(0.004,-0.003)*(h-0.5);
    let material=textureSampleLevel(cloud_texture,cloud_sampler,uv,0.0);
    let edge=smoothstep(0.0,0.18,h)*(1.0-smoothstep(0.72,1.0,h));
    return vec4<f32>(material.rgb,smoothstep(0.02,0.6,material.a)*edge*frame.cloud.w);
}
fn cloud_density(p:vec3<f32>)->f32{return cloud_material(p).a;}
fn cloud_visibility(p:vec3<f32>)->f32{
    if(frame.flags.w<0.5 || frame.sun.y<0.02 || frame.display_target.z<=0.0){return 1.0;}
    let start=max((frame.options.z-frame.origin.y-p.y)/frame.sun.y,0.0);let end=(frame.options.z+frame.cloud.x-frame.origin.y-p.y)/frame.sun.y;if(end<=start){return 1.0;}
    var optical=0.0;let step=(end-start)/6.0;
    for(var i=0u;i<6u;i++){optical+=cloud_density(p+frame.sun.xyz*(start+(f32(i)+0.5)*step))*step;}
    return exp(-optical*0.09);
}
fn shadow_visibility(p:vec3<f32>,n:vec3<f32>)->f32{
    if(frame.display_target.z<=0.0){return 1.0;}let c=frame.light_matrix*vec4<f32>(p,1.0);let q=c.xyz/c.w;
    let uv=q.xy*vec2<f32>(0.5,-0.5)+0.5;if(any(uv<=vec2<f32>(0.0)) || any(uv>=vec2<f32>(1.0)) || q.z<=0.0 || q.z>=1.0){return 1.0;}
    let size=vec2<i32>(textureDimensions(shadow_depth));let at=vec2<i32>(uv*vec2<f32>(size));var lit=0.0;
    let bias=frame.display_target.w*(1.0+3.0*(1.0-max(dot(n,frame.sun.xyz),0.0)));
    for(var y=-1;y<=1;y++){for(var x=-1;x<=1;x++){let depth=textureLoad(shadow_depth,clamp(at+vec2<i32>(x,y),vec2<i32>(0),size-1),0).r;lit+=select(0.0,1.0,q.z-bias<=depth);}}
    return mix(1.0,lit/9.0,frame.display_target.z);
}
fn atmosphere(direction:vec3<f32>)->vec3<f32>{
    if(frame.sky.x<0.0){return frame.sky.yzw;}
    let horizon=pow(clamp(1.0-max(direction.y,0.0),0.0,1.0),4.0);
    let rayleigh=mix(vec3<f32>(0.055,0.17,0.42),vec3<f32>(0.46,0.57,0.7),horizon)*frame.sun.w;
    let mu=clamp(dot(direction,frame.sun.xyz),-1.0,1.0);
    let mie=0.018/pow(max(1.0+0.76*0.76-2.0*0.76*mu,0.02),1.5);
    let disk=smoothstep(0.99994,0.99998,mu)*8.0;
    return rayleigh+frame.sun_color.rgb*frame.sun_color.w*(mie+disk)+vec3<f32>(0.001,0.002,0.005);
}
fn cloud_volume(origin:vec3<f32>,direction:vec3<f32>,distance_limit:f32)->vec4<f32>{
    if(frame.flags.w<0.5 || abs(direction.y)<0.001){return vec4<f32>(0.0,0.0,0.0,1.0);}
    let a=(frame.options.z-frame.origin.y-origin.y)/direction.y;let b=(frame.options.z+frame.cloud.x-frame.origin.y-origin.y)/direction.y;
    let start=max(min(a,b),0.0);let end=min(max(a,b),distance_limit);if(end<=start){return vec4<f32>(0.0,0.0,0.0,1.0);}
    let step=(end-start)/frame.work.z;var transmittance=1.0;var radiance=vec3<f32>(0.0);
    for(var i=0u;i<u32(frame.work.z);i++){let p=origin+direction*(start+(f32(i)+0.5)*step);let material=cloud_material(p);if(material.a<=0.0){continue;}let extinction=exp(-material.a*step*0.09);
        let lighting=material.rgb*(vec3<f32>(0.12)*frame.sun.w+frame.sun_color.rgb*frame.sun_color.w*(0.25+0.75*cloud_visibility(p)));
        radiance+=transmittance*(1.0-extinction)*lighting;transmittance*=extinction;if(transmittance<0.01){break;}
    }return vec4<f32>(radiance,transmittance);
}
fn sky(direction:vec3<f32>)->vec3<f32>{let cloud=cloud_volume(frame.eye.xyz,direction,4096.0);return atmosphere(direction)*cloud.a+cloud.rgb;}
fn trace_reflection(origin:vec3<f32>,direction:vec3<f32>)->vec4<f32>{
    if(frame.flags.x<0.5){return vec4<f32>(0.0);}var previous=origin;let steps=u32(frame.work.x);
    for(var i=1u;i<=steps;i++){let distance=0.15+96.0*pow(f32(i)/f32(steps),1.5);let p=origin+direction*distance;let q=project(p);
        if(any(q.xy<=vec2<f32>(0.0)) || any(q.xy>=vec2<f32>(1.0)) || q.z<=0.0 || q.z>=1.0){break;}
        let depth=textureLoad(opaque_depth,pixel(q.xy),0).r;let hit=world(q.xy,depth);
        if(i>2u && depth<1.0 && q.z>=depth && length(hit-p)<max(0.2,length(p-previous)*1.5)){
            var a=previous;var b=p;for(var j=0u;j<5u;j++){let mid=(a+b)*0.5;let m=project(mid);if(m.z>textureLoad(opaque_depth,pixel(m.xy),0).r){b=mid;}else{a=mid;}}
            let uv=project((a+b)*0.5).xy;let edge=min(min(uv.x,uv.y),min(1.0-uv.x,1.0-uv.y));
            return vec4<f32>(textureSampleLevel(scene,linear_sampler,uv,0.0).rgb,clamp(edge*12.0,0.0,1.0));
        }previous=p;
    }return vec4<f32>(0.0);
}
fn local_visibility(p:vec3<f32>,light:vec3<f32>,radius:f32)->f32{
    let delta=light-p;let end=p+delta*max(1.0-radius/max(length(delta),0.001),0.0);
    for(var i=1u;i<9u;i++){let q=project(mix(p,end,f32(i)/9.0));if(any(q.xy<=vec2<f32>(0.0)) || any(q.xy>=vec2<f32>(1.0))){continue;}
        let depth=textureLoad(opaque_depth,pixel(q.xy),0).r;if(depth<1.0 && q.z>depth && length(world(q.xy,depth)-mix(p,end,f32(i)/9.0))<0.6){return 0.0;}}
    return 1.0;
}
fn local_lighting(uv:vec2<f32>,p:vec3<f32>,n:vec3<f32>,v:vec3<f32>,base:vec3<f32>,metallic:f32,roughness:f32)->vec3<f32>{
    let tile=vec2<u32>(pixel(uv))/16u;let at=(tile.y*frame.light_meta.y+tile.x)*65u;var radiance=vec3<f32>(0.0);
    let overflow=tiles[at]>64u;let count=select(tiles[at],frame.light_meta.x,overflow);
    for(var i=0u;i<count;i++){var light_id=i;if(!overflow){light_id=tiles[at+1u+i];}let light=lights[light_id];if(light.kind!=2u && light.kind!=3u && light.kind!=4u && light.kind!=5u){continue;}let delta=light.position-p;let distance=length(delta);if(distance>=light.range || distance<0.001 || dot(n,delta)<=0.0){continue;}
        var attenuation=pow(max(1.0-distance/light.range,0.0),2.0)/max(distance*distance,0.04);
        if(light.kind==3u){attenuation*=smoothstep(light.outer_cone_cosine,light.inner_cone_cosine,dot(light.direction,-delta/distance));}
        let radius=select(0.0,light.outer_cone_cosine,light.kind==4u || light.kind==5u);
        radiance+=local_brdf(base,metallic,roughness,n,v,delta/distance,light.color*light.intensity*attenuation,light.kind==4u,radius,distance)*local_visibility(p+n*0.04,light.position,radius);
    }return radiance;
}
fn fog_volume(direction:vec3<f32>,distance_limit:f32)->vec4<f32>{
    if(frame.flags.z<0.5){return vec4<f32>(0.0,0.0,0.0,1.0);}let distance=min(distance_limit,512.0);let step=distance/frame.work.y;var transmittance=1.0;var radiance=vec3<f32>(0.0);
    let g=0.55;let mu=dot(direction,frame.sun.xyz);let phase=(1.0-g*g)/(4.0*PI*pow(1.0+g*g-2.0*g*mu,1.5));
    for(var i=0u;i<u32(frame.work.y);i++){let p=frame.eye.xyz+direction*((f32(i)+0.5)*step);let density=frame.options.y*exp(-max(p.y+frame.origin.y-64.0,0.0)/80.0);
        let extinction=exp(-density*step);let environment=select(vec3<f32>(0.08,0.13,0.22)*frame.sun.w,frame.sky.yzw*0.5,frame.sky.x<0.0);let illumination=environment+frame.sun_color.rgb*frame.sun_color.w*phase*shadow_visibility(p,frame.sun.xyz)*cloud_visibility(p);
        radiance+=transmittance*(1.0-extinction)*illumination;transmittance*=extinction;
    }return vec4<f32>(radiance,transmittance);
}
fn display_color(color:vec3<f32>)->vec3<f32>{let c=max(color*frame.options.x,vec3<f32>(0.0));let mapped=clamp((c*(2.51*c+0.03))/(c*(2.43*c+0.59)+0.14),vec3<f32>(0.0),vec3<f32>(1.0));return select(mapped*12.92,1.055*pow(mapped,vec3<f32>(1.0/2.4))-0.055,mapped>vec3<f32>(0.0031308));}
@fragment fn fs_effects(input:ScreenOutput)->@location(0) vec4<f32>{
    let uv=input.uv;let at=pixel(uv);let depth=textureLoad(surface_depth,at,0).r;let p=world(uv,min(depth,0.99999));let ray=normalize(p-frame.eye.xyz);var color=vec3<f32>(0.0);var distance_limit=4096.0;
    if(depth>=1.0){color=atmosphere(ray);}
    if(depth<1.0){distance_limit=length(p-frame.eye.xyz);let material=textureLoad(surfaces,at,0);var n=normal(material.xy);let albedo=textureLoad(albedos,at,0);let v=-ray;
        if(material.w<0.0){color=textureSampleLevel(scene,linear_sampler,uv,0.0).rgb;}else{
        let water=material.w>=2.0 && material.w<4.0;let glass=material.w>=4.0;let metallic=select(clamp(material.w,0.0,1.0),0.0,water||glass);var roughness=material.z;
        if(water && frame.flags.y>0.5){let absolute=p+frame.origin.xyz;let phase=absolute.xz*vec2<f32>(0.3,0.22)+frame.work.w*vec2<f32>(0.8,-0.6);n=normalize(n+vec3<f32>(sin(phase.x)*0.12,0.0,cos(phase.y)*0.1));roughness=0.08;}
        let direct=cook_torrance_lighting(albedo.rgb,metallic,roughness,n,v,frame.sun.xyz,frame.sun_color.rgb*frame.sun_color.w*3.0);
        let visibility=shadow_visibility(p,n)*cloud_visibility(p);color=textureSampleLevel(scene,linear_sampler,uv,0.0).rgb;
        if(water||glass){
            let candidate=clamp(uv+n.xz*0.012,vec2<f32>(0.001),vec2<f32>(0.999));let refracted_depth=textureLoad(opaque_depth,pixel(candidate),0).r;
            let refracted=select(uv,candidate,refracted_depth>depth);let behind_depth=textureLoad(opaque_depth,pixel(refracted),0).r;var behind=textureSampleLevel(scene,linear_sampler,refracted,0.0).rgb;
            if(behind_depth>=1.0){behind=atmosphere(normalize(world(refracted,0.99999)-frame.eye.xyz));}
            let thickness=select(min(length(world(refracted,behind_depth)-p),32.0),0.0,behind_depth>=1.0 && !water);
            let absorption=select(-log(max(albedo.rgb,vec3<f32>(0.02)))*0.3,frame.water_absorption.xyz,water);let transmittance=exp(-absorption*thickness);
            color=behind*transmittance+albedo.rgb*(1.0-transmittance)*0.15+direct*visibility;
        }else{color=max(color-direct,vec3<f32>(0.0))+direct*visibility;}
        color+=local_lighting(uv,p,n,v,albedo.rgb,metallic,roughness);
        if((frame.flags.x>0.5 && roughness<0.5)||water||glass){let reflected=reflect(ray,n);let hit=trace_reflection(p+n*0.08,reflected);let environment=sky(reflected);let reflection=mix(environment,hit.rgb,hit.a);
            let f0=select(mix(vec3<f32>(0.04),albedo.rgb,metallic),vec3<f32>(0.02),water);let fresnel=fresnel_schlick(max(dot(n,v),0.0),f0)*(1.0-roughness*0.6);color=mix(color,reflection,clamp(fresnel,vec3<f32>(0.0),vec3<f32>(1.0)));}
    }
    }
    let clouds=cloud_volume(frame.eye.xyz,ray,distance_limit);color=color*clouds.a+clouds.rgb;
    let fog=fog_volume(ray,distance_limit);color=color*fog.a+fog.rgb;
    if(frame.options.w>0.5){let t=exp(-frame.water_absorption.xyz*min(distance_limit,64.0));color=color*t+vec3<f32>(0.015,0.08,0.12)*(1.0-t);}
    return vec4<f32>(display_color(color),1.0);
}
@fragment fn fs_composite(input:ScreenOutput)->Resolved {var out:Resolved;out.color=textureSampleLevel(scene,linear_sampler,input.uv,0.0);let at=vec2<i32>(clamp(input.uv,vec2<f32>(0.0),vec2<f32>(0.99999))*vec2<f32>(textureDimensions(opaque_depth)));out.depth=textureLoad(opaque_depth,at,0).r;return out;}
