// Optional GPU-froxel screen path. The build script replaces the old per-
// fragment fog_volume call with froxel_fog for this shader artifact only.
// No additional 3D texture is required by legacy/unsupported backends.
@group(0) @binding(20) var froxel_integrated:texture_3d<f32>;
fn froxel_fog(uv:vec2<f32>,distance_limit:f32)->vec4<f32>{
    if (frame.flags.z < 0.5 || frame.options.y <= 0.0 || distance_limit <= 0.0) {
        return vec4<f32>(0.0,0.0,0.0,1.0);
    }
    let size = vec3<f32>(textureDimensions(froxel_integrated));
    let scaled = clamp(
        log(clamp(distance_limit,0.1,512.0)/0.1) / log(512.0/0.1),
        0.0,1.0
    );
    // Integration stores cumulative scattering/transmittance at the END of
    // each depth slice. Align texture filtering to those slice boundaries.
    let z = clamp(scaled - 0.5/size.z,0.0,1.0);
    return textureSampleLevel(
        froxel_integrated, linear_sampler,
        vec3<f32>(clamp(uv,vec2<f32>(0.0),vec2<f32>(1.0)),z),0.0
    );
}
@fragment fn fs_effects_froxel(input:ScreenOutput)->@location(0) vec4<f32> {
    return effects_shade(input);
}
@fragment fn fs_effects_froxel_ms(input:ScreenOutput)->@location(0) vec4<f32> {
    return effects_shade(input);
}
