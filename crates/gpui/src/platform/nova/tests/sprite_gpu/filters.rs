use super::*;

pub(super) fn verify_pixels<D>(
    device: &mut D,
    resources: &RendererResources,
    target: TextureId,
    view: TextureViewId,
) where
    D: BackendResources + BackendPipelines + BackendPresentationCompat + TextureTransferDevice,
{
    let extent = Extent2d::new(16, 16).expect("extent");
    let (source, source_view) = color_target(device, extent);
    let (_, horizontal_view) = color_target(device, extent);
    let mut pixels = [0_u8; 16 * 16 * 4];
    for y in 0..16 {
        for x in 0..16 {
            pixels[(y * 16 + x) * 4 + 3] = 255;
            if (6..10).contains(&x) && (6..10).contains(&y) {
                pixels[(y * 16 + x) * 4 + 2] = 255;
            }
        }
    }
    device
        .write_texture(
            TextureWriteDescriptor {
                texture: source,
                mip_level: 0,
                origin: Origin2d { x: 0, y: 0 },
                size: extent,
                layout: TextureDataLayout {
                    offset: 0,
                    bytes_per_row: std::num::NonZeroU32::new(64).expect("row pitch"),
                    rows_per_image: std::num::NonZeroU32::new(16).expect("row count"),
                },
            },
            &pixels,
        )
        .expect("blur source upload");
    let buffers = resources.frame_resources[0].buffers;
    let mut kernel = Vec::new();
    write_backdrop_blur_pass(&mut kernel, 2.0);
    write_backdrop_blur_pass(&mut kernel, 2.0);
    device
        .write_buffer(buffers.backdrop_blur_pass_buffer, 0, &kernel)
        .expect("Gaussian kernel upload");
    for (index, input, output, pipeline) in [
        (
            0,
            source_view,
            horizontal_view,
            resources.pipelines.backdrop_blur_downsample,
        ),
        (
            1,
            horizontal_view,
            view,
            resources.pipelines.backdrop_blur_upsample,
        ),
    ] {
        let set = device
            .create_resource_set(&ResourceSetDescriptor {
                label: None,
                layout: resources.backdrop_blur_pass_resource_set_layout,
                bindings: backdrop_blur_pass_resource_bindings(
                    input,
                    resources.atlas_sampler,
                    buffers.backdrop_blur_pass_buffer,
                    buffers.animation_value_buffer,
                ),
            })
            .expect("blur pass bindings");
        device
            .render_steps_to_texture_compat(
                output,
                resources.render_pass,
                &[RenderStepDescriptor::Draw(DrawStepDescriptor {
                    pipeline,
                    resource_sets: resource_set_list([set]),
                    vertex_count: 4,
                    instance_count: 1,
                    first_vertex: 0,
                    first_instance: index,
                    scissor: None,
                })],
                LoadOp::Clear(ClearColor {
                    red: 0.0,
                    green: 0.0,
                    blue: 0.0,
                    alpha: 1.0,
                }),
                None,
            )
            .expect("Gaussian pass");
    }
    let pixels = device.read_texture(target).expect("blur readback");
    let center = primitives::pixel(&pixels, 8, 8)[2];
    assert!(
        center > 0 && center < 255,
        "Gaussian center is filtered: {center}"
    );
    let horizontal = primitives::pixel(&pixels, 5, 8)[2];
    let vertical = primitives::pixel(&pixels, 8, 5)[2];
    assert!(horizontal > 0 && vertical > 0, "both axes spread color");
    assert!(
        horizontal.abs_diff(vertical) <= 1,
        "symmetric Gaussian axes"
    );
    assert!(
        primitives::pixel(&pixels, 0, 0)[2] <= 1,
        "distant pixels retain the background"
    );
    println!("production horizontal and vertical Gaussian blur: native pixels verified");
}
