use super::*;

pub(super) fn verify_pixels<D>(
    device: &mut D,
    resources: &RendererResources,
    target: TextureId,
    view: TextureViewId,
) where
    D: BackendResources + BackendPipelines + BackendPresentationCompat + TextureTransferDevice,
{
    let frame = resources.frame_resources[0];
    let mask = create_path_mask_target(
        device,
        "path pixel gate",
        PathMaskTargetDescriptor {
            size: Extent2d::new(16, 16).expect("extent"),
            format: Format::Bgra8Unorm,
            resource_set_layout: resources.path_resource_set_layout,
            frame_buffers: vec![frame.buffers],
            sampler: resources.atlas_sampler,
        },
    )
    .expect("path mask");
    let bounds = bounds(point(px(0.0), px(0.0)), size(px(16.0), px(16.0))).scale(1.0);
    let content_mask = crate::ContentMask {
        bounds,
        corner_bounds: bounds,
        ..Default::default()
    };
    let background = crate::rgb(0xff0000).into();
    let mut bytes = Vec::new();
    for (x, y) in [(2.0, 2.0), (14.0, 2.0), (2.0, 14.0)] {
        write_path_rasterization_vertex(
            &mut bytes,
            &crate::PathVertex_ScaledPixels {
                xy_position: point(crate::ScaledPixels(x), crate::ScaledPixels(y)),
                st_position: point(0.0, 0.0),
                content_mask: 0,
            },
            &background,
            &content_mask,
        );
    }
    device
        .write_buffer(frame.buffers.path_rasterization_vertex_buffer, 0, &bytes)
        .expect("path vertices");
    draw(
        device,
        resources,
        mask.texture_view,
        DrawStepDescriptor {
            pipeline: resources.pipelines.path_rasterization,
            resource_sets: resource_set_list([frame.resource_sets.path_rasterization_resource_set]),
            vertex_count: 3,
            instance_count: 1,
            first_vertex: 0,
            first_instance: 0,
            scissor: None,
        },
        0.0,
    );
    bytes.clear();
    write_path_sprite(&mut bytes, &bounds);
    device
        .write_buffer(frame.buffers.path_sprite_buffer, 0, &bytes)
        .expect("path sprite");
    draw(
        device,
        resources,
        view,
        DrawStepDescriptor {
            pipeline: resources.pipelines.paths,
            resource_sets: resource_set_list([mask.resource_sets[0]]),
            vertex_count: 4,
            instance_count: 1,
            first_vertex: 0,
            first_instance: 0,
            scissor: None,
        },
        1.0,
    );
    let pixels = device.read_texture(target).expect("path readback");
    assert_eq!(
        primitives::pixel(&pixels, 4, 4),
        [0, 0, 255, 255],
        "path fill"
    );
    assert_eq!(
        primitives::pixel(&pixels, 12, 12),
        [0, 0, 0, 255],
        "path exterior"
    );
    println!("production path rasterization and composition: native pixels verified");
}

fn draw<D: BackendPresentationCompat>(
    device: &mut D,
    resources: &RendererResources,
    view: TextureViewId,
    step: DrawStepDescriptor,
    alpha: f32,
) {
    device
        .render_steps_to_texture_compat(
            view,
            resources.render_pass,
            &[RenderStepDescriptor::Draw(step)],
            LoadOp::Clear(ClearColor {
                red: 0.0,
                green: 0.0,
                blue: 0.0,
                alpha,
            }),
            None,
        )
        .expect("path draw");
}
