use super::*;

#[test]
#[ignore = "requires Windows hardware D3D11 and a desktop session"]
fn native_swapchains_resize_frame_waits_and_foreign_handles() {
    let mut device = open_device();
    let mut foreign = open_device();
    let encoder = gfx_core::CommandDevice::create_command_encoder(
        &mut device,
        &CommandEncoderDescriptor { label: None },
    )
    .expect("encoder");
    let submission = device.submit_deferred(encoder).expect("submission");
    assert!(
        foreign.poll_submission(submission).is_err(),
        "foreign submissions must not alias"
    );
    device
        .wait_submission(submission)
        .expect("GPU event completion");
    let resource = buffer(&mut device, 16, BufferUsage::STORAGE);
    assert!(foreign.write_buffer(resource, 0, &[0; 16]).is_err());
    device.destroy_buffer(resource).expect("destroy resource");
    assert!(device.write_buffer(resource, 0, &[0; 16]).is_err());
    for alpha in [
        CompositeAlphaMode::Opaque,
        CompositeAlphaMode::Premultiplied,
    ] {
        let window = TestWindow::new();
        let surface = device
            .create_surface(&window, &gfx_core::SurfaceDescriptor { label: None })
            .expect("surface");
        let config = gfx_core::SurfaceConfig {
            size: Extent2d::new(64, 64).expect("size"),
            format: Format::Bgra8Unorm,
            present_mode: PresentMode::Mailbox,
            alpha_mode: alpha,
        };
        let chain = device
            .create_swapchain(surface, config)
            .expect("native flip chain");
        let pass = device
            .create_render_pass(&RenderPassDescriptor {
                label: None,
                color_attachment: ColorAttachmentDescriptor {
                    format: Format::Bgra8Unorm,
                },
                depth_attachment: None,
            })
            .expect("surface pass");
        for _ in 0..3 {
            device
                .render_step_list_and_present_compat(
                    chain,
                    pass,
                    RenderStepList::Draw(&[]),
                    CLEAR,
                    None,
                )
                .expect("present");
        }
        device.resize_swapchain(chain, 96, 80).expect("resize");
        device
            .render_step_list_and_present_compat(
                chain,
                pass,
                RenderStepList::Draw(&[]),
                CLEAR,
                None,
            )
            .expect("resized present");
        let config = gfx_core::SurfaceConfig {
            size: Extent2d::new(80, 72).expect("size"),
            ..config
        };
        let chain = device
            .recreate_swapchain(chain, config)
            .expect("reconfigure native window");
        device
            .render_step_list_and_present_compat(
                chain,
                pass,
                RenderStepList::Draw(&[]),
                CLEAR,
                None,
            )
            .expect("reconfigured present");
        device.destroy_swapchain(chain).expect("destroy chain");
        device.destroy_surface(surface).expect("destroy surface");
    }
}
