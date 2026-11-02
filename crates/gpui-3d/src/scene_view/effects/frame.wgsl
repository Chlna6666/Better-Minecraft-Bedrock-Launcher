struct EffectFrame {
    view_projection:mat4x4<f32>,inverse_view_projection:mat4x4<f32>,light_matrix:mat4x4<f32>,
    eye:vec4<f32>,sun:vec4<f32>,sun_color:vec4<f32>,work:vec4<f32>,flags:vec4<f32>,options:vec4<f32>,
    origin:vec4<f32>,bounds:vec4<f32>,display_target:vec4<f32>,light_meta:vec4<u32>,water_absorption:vec4<f32>,cloud:vec4<f32>,sky:vec4<f32>,
};
struct EffectLight {kind:u32,position:vec3<f32>,range:f32,direction:vec3<f32>,color:vec3<f32>,intensity:f32,outer_cone_cosine:f32,inner_cone_cosine:f32};
