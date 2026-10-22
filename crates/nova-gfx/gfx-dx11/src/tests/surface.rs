use super::*;

#[test]
#[ignore = "requires Windows hardware D3D11 and a desktop session"]
fn resize_retirement_is_scoped_to_native_buffer_generation() {
    let mut device = Dx11Device::new(&DeviceDescriptor::default()).expect("hardware DX11 device");
    for alpha in [
        CompositeAlphaMode::Opaque,
        CompositeAlphaMode::Premultiplied,
    ] {
        verify_resize_retirement(&mut device, alpha);
    }
}

#[expect(
    unsafe_code,
    reason = "the hardware gate queries a live DXGI swapchain descriptor"
)]
fn verify_resize_retirement(device: &mut Dx11Device, alpha: CompositeAlphaMode) {
    let windows = [TestWindow::new(), TestWindow::new()];
    let surfaces = windows.each_ref().map(|window| {
        device
            .create_surface(window, &gfx_core::SurfaceDescriptor { label: None })
            .expect("surface")
    });
    let chains = surfaces.map(|surface| {
        device
            .create_swapchain(
                surface,
                gfx_core::SurfaceConfig {
                    size: Extent2d::new(64, 64).expect("extent"),
                    format: Format::Bgra8Unorm,
                    present_mode: PresentMode::Mailbox,
                    alpha_mode: alpha,
                },
            )
            .expect("swapchain")
    });
    for chain in chains {
        // SAFETY: these swapchains and their owning HWNDs stay live throughout the test.
        let native = unsafe {
            device
                .swapchains
                .get(chain)
                .expect("chain")
                .native
                .GetDesc1()
        }
        .expect("native descriptor");
        assert_eq!(
            native.Scaling,
            windows::Win32::Graphics::Dxgi::DXGI_SCALING_STRETCH,
            "a grown HWND must be covered by scaling the last presented frame"
        );
    }
    let pass = device
        .create_render_pass(&RenderPassDescriptor {
            label: None,
            color_attachment: ColorAttachmentDescriptor {
                format: Format::Bgra8Unorm,
            },
            depth_attachment: None,
        })
        .expect("pass");
    verify_window_retirement(device, chains, pass);
    for chain in chains {
        device.destroy_swapchain(chain).expect("destroy chain");
    }
    for surface in surfaces {
        device.destroy_surface(surface).expect("destroy surface");
    }
    device.destroy_render_pass(pass).expect("destroy pass");
}

fn verify_window_retirement(
    device: &mut Dx11Device,
    chains: [gfx_core::SwapchainId; 2],
    pass: gfx_core::RenderPassId,
) {
    let first = device
        .present_steps(chains[0], pass, &[], LoadOp::Clear(CLEAR), None)
        .expect("first Present");
    device.wait_submission(first).expect("first retirement");
    assert!(device.prepare_swapchain_resize(chains[0]).expect("ready"));
    for _ in 0..8 {
        device
            .present_steps(chains[1], pass, &[], LoadOp::Clear(CLEAR), None)
            .expect("other window Present");
        assert!(
            device
                .prepare_swapchain_resize(chains[0])
                .expect("still ready")
        );
        assert_eq!(
            device
                .swapchains
                .get(chains[0])
                .expect("chain")
                .resize_retirement,
            Some(first),
            "another window must not move this window's completed retirement point"
        );
    }
    verify_new_backbuffer_reader(device, chains[0], pass, first);
}

fn verify_new_backbuffer_reader(
    device: &mut Dx11Device,
    chain: gfx_core::SwapchainId,
    pass: gfx_core::RenderPassId,
    first: gfx_core::SubmissionId,
) {
    // A draw without Present is also a new reader of this backbuffer.
    device
        .render_target(
            gfx_core::RenderTarget::Swapchain {
                swapchain: chain,
                image_index: 0,
            },
            pass,
            &[],
            LoadOp::Clear(CLEAR),
            None,
        )
        .expect("new draw");
    assert!(
        device
            .swapchains
            .get(chain)
            .expect("chain")
            .resize_retirement
            .is_none()
    );
    device
        .prepare_swapchain_resize(chain)
        .expect("new retirement point");
    let next = device
        .swapchains
        .get(chain)
        .expect("chain")
        .resize_retirement
        .expect("marker");
    assert_ne!(next, first);
    device.wait_submission(next).expect("new readers retire");
    let previous = device
        .swapchains
        .get(chain)
        .expect("old chain")
        .native
        .clone();
    device
        .resize_swapchain(chain, 96, 80)
        .expect("native resize");
    let resized = device.swapchains.get(chain).expect("resized chain");
    assert_eq!(resized.generation, 1);
    assert_eq!(resized.source_extent, Extent2d::new(96, 80).unwrap());
    assert_eq!(
        previous, resized.native,
        "size-only resize must retain the native presentation stream"
    );
    assert!(
        resized.resize_retirement.is_none(),
        "old native buffers cannot certify new buffers"
    );
}

#[test]
#[ignore = "requires Windows hardware D3D11 and a desktop session"]
fn native_swapchains_resize_frame_waits_and_foreign_handles() {
    let mut device = Dx11Device::new(&DeviceDescriptor::default()).expect("hardware DX11 device");
    let mut foreign = Dx11Device::new(&DeviceDescriptor::default()).expect("second device");
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
                .render_and_present(&PresentFrameDescriptor {
                    swapchain: chain,
                    render_pass: pass,
                    commands: &[],
                    color_load_op: LoadOp::Clear(CLEAR),
                    depth_attachment: None,
                    present_region: None,
                })
                .expect("present");
        }
        device.resize_swapchain(chain, 96, 80).expect("resize");
        let mut chain = device
            .recreate_swapchain(
                chain,
                gfx_core::SurfaceConfig {
                    size: Extent2d::new(80, 72).expect("recreated size"),
                    ..config
                },
            )
            .expect("same-alpha recreation retains the composition target");
        device
            .render_and_present(&PresentFrameDescriptor {
                swapchain: chain,
                render_pass: pass,
                commands: &[],
                color_load_op: LoadOp::Clear(CLEAR),
                depth_attachment: None,
                present_region: None,
            })
            .expect("resized present");
        let (send, recv) = std::sync::mpsc::channel();
        assert!(
            device
                .request_frame_ready(
                    chain,
                    Box::new(move || send.send(()).expect("frame notification"))
                )
                .expect("arm frame wait")
        );
        recv.recv_timeout(std::time::Duration::from_secs(3))
            .expect("DXGI native wait notification");
        assert!(device.swapchain_frame_ready(chain).expect("frame credit"));
        for next_alpha in [
            CompositeAlphaMode::Opaque,
            CompositeAlphaMode::Premultiplied,
        ] {
            chain = device
                .recreate_swapchain(
                    chain,
                    gfx_core::SurfaceConfig {
                        alpha_mode: next_alpha,
                        ..config
                    },
                )
                .expect("opaque/premultiplied transition");
            assert_eq!(
                device
                    .swapchains
                    .get(chain)
                    .expect("active chain")
                    .config
                    .alpha_mode,
                next_alpha
            );
            device
                .render_and_present(&PresentFrameDescriptor {
                    swapchain: chain,
                    render_pass: pass,
                    commands: &[],
                    color_load_op: LoadOp::Clear(CLEAR),
                    depth_attachment: None,
                    present_region: None,
                })
                .expect("present after alpha transition");
        }
        device.destroy_swapchain(chain).expect("destroy chain");
        device.destroy_surface(surface).expect("destroy surface");
    }
}
