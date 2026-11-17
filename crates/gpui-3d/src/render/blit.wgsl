// Fullscreen blit: sample one display-referred texture and write it to a color attachment.
//
// Used by external presenters that hold an OffscreenRenderer::color_view and need to put those
// pixels into a native swapchain without a CPU readback. Bindings are fixed at group 0:
//   @binding(0) texture_2d<f32>  -- the source color view
//   @binding(1) sampler         -- linear clamp sampler
// The fullscreen triangle emits three vertices and needs no vertex buffer.

@group(0) @binding(0) var src_texture: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

struct BlitOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_blit(@builtin(vertex_index) vertex_index: u32) -> BlitOutput {
    let x = f32((vertex_index << 1u) & 2u);
    let y = f32(vertex_index & 2u);
    var output: BlitOutput;
    output.position = vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
    // Match post.wgsl's Y orientation so the blit is not vertically flipped.
    output.uv = vec2<f32>(x, 1.0 - y);
    return output;
}

@fragment
fn fs_blit(input: BlitOutput) -> @location(0) vec4<f32> {
    return textureSample(src_texture, src_sampler, input.uv);
}
