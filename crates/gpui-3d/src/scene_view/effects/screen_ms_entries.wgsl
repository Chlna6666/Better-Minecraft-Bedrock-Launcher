// Multisampled entry points: the shared body reads depth with textureLoad, so the
// only difference from the single-sampled build is the binding type.
@fragment fn fs_effects_ms(input:ScreenOutput)->@location(0) vec4<f32>{return effects_shade(input);}
@fragment fn fs_composite_ms(input:ScreenOutput)->Resolved{return composite_resolve(input);}
