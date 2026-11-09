// Single-sampled depth inputs for the screen effects shader.
// The multisampled build replaces these declarations with texture_multisampled_2d.
@group(0) @binding(1) var opaque_depth:texture_2d<f32>;
@group(0) @binding(3) var surface_depth:texture_2d<f32>;
@group(0) @binding(4) var albedos:texture_2d<f32>;
