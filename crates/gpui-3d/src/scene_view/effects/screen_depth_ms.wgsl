// Multisampled depth inputs for the screen effects shader.
// Sample zero is read through textureLoad, the only legal access for multisampled textures.
@group(0) @binding(1) var opaque_depth:texture_multisampled_2d<f32>;
@group(0) @binding(3) var surface_depth:texture_multisampled_2d<f32>;
@group(0) @binding(4) var albedos:texture_multisampled_2d<f32>;
