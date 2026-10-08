use super::*;

pub(super) fn verify_pixels<D>(
    device: &mut D,
    resources: &RendererResources,
    target: TextureId,
    view: TextureViewId,
) where
    D: BackendResources + BackendPresentationCompat + TextureTransferDevice,
{
    let bounds = bounds(point(px(2.0), px(2.0)), size(px(12.0), px(12.0))).scale(1.0);
    let content_mask = crate::ContentMask {
        bounds,
        corner_bounds: bounds,
        ..Default::default()
    };
    let frame = resources.frame_resources[0];
    let pipelines = resources.pipelines.alpha;
    let mut bytes = Vec::new();
    write_quad(
        &mut bytes,
        &Quad {
            bounds,
            content_mask,
            background: crate::rgb(0xff0000).into(),
            corner_radii: crate::Corners::all(crate::ScaledPixels(4.0)),
            ..Default::default()
        },
    );
    device
        .write_buffer(frame.buffers.quad_buffer, 0, &bytes)
        .expect("rounded quad upload");
    draw(
        device,
        resources,
        view,
        pipelines.quads,
        frame.resource_sets.quad_resource_set,
    );
    let pixels = device.read_texture(target).expect("quad readback");
    assert_eq!(pixel(&pixels, 8, 8), [0, 0, 255, 255], "quad interior");
    assert_eq!(pixel(&pixels, 2, 2), [0, 0, 0, 255], "rounded corner");
    bytes.clear();
    write_shadow(
        &mut bytes,
        &Shadow {
            order: 0,
            blur_radius: crate::ScaledPixels(2.0),
            animation_id: None,
            bounds,
            content_mask,
            corner_radii: Default::default(),
            color: crate::rgb(0xff0000).into(),
        },
    );
    device
        .write_buffer(frame.buffers.shadow_buffer, 0, &bytes)
        .expect("shadow upload");
    draw(
        device,
        resources,
        view,
        pipelines.shadows,
        frame.resource_sets.shadow_resource_set,
    );
    let pixels = device.read_texture(target).expect("shadow readback");
    assert!(pixel(&pixels, 8, 8)[2] > 200, "shadow center");
    assert_eq!(pixel(&pixels, 0, 0), [0, 0, 0, 255], "shadow content mask");
    bytes.clear();
    write_underline(
        &mut bytes,
        &Underline {
            order: 0,
            pad: 0,
            animation_id: None,
            bounds,
            content_mask,
            color: crate::rgb(0xff0000).into(),
            thickness: crate::ScaledPixels(12.0),
            wavy: 0,
        },
    );
    device
        .write_buffer(frame.buffers.underline_buffer, 0, &bytes)
        .expect("underline upload");
    draw(
        device,
        resources,
        view,
        pipelines.underlines,
        frame.resource_sets.underline_resource_set,
    );
    let pixels = device.read_texture(target).expect("underline readback");
    assert_eq!(pixel(&pixels, 8, 8), [0, 0, 255, 255], "underline interior");
    assert_eq!(pixel(&pixels, 0, 0), [0, 0, 0, 255], "underline clipping");
    println!("production rounded quad, shadow, underline and clipping: native pixels verified");
}

fn draw<D: BackendPresentationCompat>(
    device: &mut D,
    resources: &RendererResources,
    view: TextureViewId,
    pipeline: RenderPipelineId,
    set: ResourceSetId,
) {
    device
        .render_steps_to_texture_compat(
            view,
            resources.render_pass,
            &[RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline,
                resource_sets: resource_set_list([set]),
                vertex_count: 4,
                instance_count: 1,
                first_vertex: 0,
                first_instance: 0,
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
        .expect("production primitive draw");
}

pub(super) fn pixel(pixels: &gfx_core::TextureReadback, x: usize, y: usize) -> [u8; 4] {
    pixels.bytes
        [y * pixels.bytes_per_row as usize + x * 4..y * pixels.bytes_per_row as usize + x * 4 + 4]
        .try_into()
        .expect("BGRA pixel")
}
