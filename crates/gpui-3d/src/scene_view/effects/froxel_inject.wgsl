// GPU-resident volumetric lighting injection. Shared EffectFrame ABI is
// prepended at build time; no host-side voxel uploads or CPU shadow resolves.
const FROXEL_NEAR:f32 = 0.1;
const FROXEL_FAR:f32 = 512.0;
const PI:f32 = 3.14159265359;
@group(0) @binding(0) var<uniform> frame:EffectFrame;
@group(0) @binding(1) var<storage,read> lights:array<EffectLight>;
@group(0) @binding(2) var<storage,read> tiles:array<u32>;
@group(0) @binding(3) var shadow_near:texture_2d<f32>;
@group(0) @binding(4) var shadow_middle:texture_2d<f32>;
@group(0) @binding(5) var shadow_far:texture_2d<f32>;
@group(0) @binding(6) var injected:texture_storage_3d<rgba16float,write>;

fn shadow_depth(cascade:u32, pixel:vec2<i32>)->f32 {
    if (cascade == 0u) { return textureLoad(shadow_near,pixel,0).r; }
    if (cascade == 1u) { return textureLoad(shadow_middle,pixel,0).r; }
    return textureLoad(shadow_far,pixel,0).r;
}
fn shadow_matrix(cascade:u32)->mat4x4<f32> {
    if (cascade == 0u) { return frame.light_matrix; }
    if (cascade == 1u) { return frame.light_matrix_middle; }
    return frame.light_matrix_far;
}
fn shadow_visibility(p:vec3<f32>, distance:f32)->f32 {
    if (frame.display_target.z <= 0.0 || frame.sun_color.w <= 0.0) {
        return 1.0;
    }
    let cascade = select(select(2u,1u,distance < frame.cascade_splits.y),
                         0u,distance < frame.cascade_splits.x);
    let light = shadow_matrix(cascade);
    let clip = light * vec4<f32>(p,1.0);
    if (clip.w <= 0.0) { return 1.0; }
    let q = clip.xyz / clip.w;
    let uv = q.xy * vec2<f32>(0.5,-0.5) + vec2<f32>(0.5);
    if (any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))
        || q.z <= 0.0 || q.z >= 1.0) { return 1.0; }
    let size = vec2<i32>(textureDimensions(shadow_near));
    let at = vec2<i32>(uv * vec2<f32>(size));
    let bias = max(frame.display_target.w,0.00005) * 3.0;
    var visible = 0.0;
    // Equal 3x3 PCF kernel for all cascades, computed once per froxel.
    for (var y:i32 = -1; y <= 1; y++) {
        for (var x:i32 = -1; x <= 1; x++) {
            let sample_at = clamp(at+vec2<i32>(x,y),vec2<i32>(0),size-1);
            visible += select(0.0,1.0,q.z-bias <= shadow_depth(cascade,sample_at));
        }
    }
    return visible / 9.0;
}
fn local_scattering(p:vec3<f32>, screen_uv:vec2<f32>)->vec3<f32> {
    if (frame.light_meta.x == 0u) { return vec3<f32>(0.0); }
    let tile_xy = light_tile(screen_uv,frame.display_target.xy,frame.light_meta.yz);
    let base = (tile_xy.y * frame.light_meta.y + tile_xy.x) * 65u;
    let tile_count = tiles[base];
    let overflow = tile_count > 64u;
    // The stored list is incomplete on overflow. Test the full light set
    // against the froxel so relevant lights beyond the prefix still contribute.
    let count = select(tile_count, frame.light_meta.x, overflow);
    var radiance = vec3<f32>(0.0);
    for(var i=0u; i<count; i++){
        var light_id = i;
        if (!overflow) { light_id = tiles[base+1u+i]; }
        let light = lights[light_id];
        if(light.kind != 2u && light.kind != 3u && light.kind != 4u && light.kind != 5u){continue;}
        let delta = light.position - p;
        let distance_squared = dot(delta,delta);
        if(distance_squared < 0.000001 || distance_squared >= light.range*light.range){continue;}
        let distance = sqrt(distance_squared);
        let fade = max(1.0 - distance / light.range,0.0);
        var attenuation = fade*fade / max(distance_squared,0.04);
        if(light.kind == 3u){
            attenuation *= smoothstep(
                light.outer_cone_cosine, light.inner_cone_cosine,
                dot(light.direction, -delta/distance)
            );
        }
        radiance += light.color * light.intensity * attenuation * (1.0/(4.0*PI));
    }
    return radiance;
}
@compute @workgroup_size(8,8,4)
fn cs_froxel_inject(@builtin(global_invocation_id) id:vec3<u32>){
    let dimensions = textureDimensions(injected);
    if (any(id >= dimensions)){return;}
    if(frame.flags.z < 0.5 || frame.options.y <= 0.0){
        textureStore(injected,vec3<i32>(id),vec4<f32>(0.0));
        return;
    }
    let screen_uv = (vec2<f32>(id.xy)+vec2<f32>(0.5))/vec2<f32>(dimensions.xy);
    let clip = frame.inverse_view_projection * vec4<f32>(
        screen_uv * vec2<f32>(2.0,-2.0)+vec2<f32>(-1.0,1.0),1.0,1.0
    );
    let ray = normalize(clip.xyz / clip.w - frame.eye.xyz);
    let t = (f32(id.z)+0.5)/f32(dimensions.z);
    let distance = exp(mix(log(FROXEL_NEAR),log(FROXEL_FAR),t));
    let p = frame.eye.xyz + ray * distance;
    let density = frame.options.y * exp(-max(p.y+frame.origin.y-64.0,0.0)/80.0);
    let g = 0.55;
    let mu = dot(ray,frame.sun.xyz);
    let phase_denom = max(1.0+g*g-2.0*g*mu,0.0001);
    let phase = (1.0-g*g)/(4.0*PI*phase_denom*sqrt(phase_denom));
    let environment = select(
        vec3<f32>(0.08,0.13,0.22)*frame.sun.w,
        frame.sky.yzw*0.5,frame.sky.x < 0.0
    );
    var illumination = environment;
    if(frame.sun_color.w > 0.0){
        illumination += frame.sun_color.rgb * frame.sun_color.w * phase
            * shadow_visibility(p,distance);
    }
    illumination += local_scattering(p,screen_uv);
    textureStore(injected,vec3<i32>(id),vec4<f32>(illumination,density));
}
