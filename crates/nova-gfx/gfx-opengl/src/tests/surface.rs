use super::*;

#[test]
#[ignore = "requires Windows native OpenGL 4.5 driver"]
fn live_resize_capacity_blit_keeps_native_top_origin() {
    use glow::HasContext as _;
    let mut device = open_device();
    let window = TestWindow::new();
    let surface = device
        .create_surface(&window, &gfx_core::SurfaceDescriptor { label: None })
        .expect("surface");
    let chain = device
        .create_swapchain(
            surface,
            gfx_core::SurfaceConfig {
                size: Extent2d::new(64, 64).expect("capacity"),
                format: Format::Rgba8Unorm,
                present_mode: PresentMode::Mailbox,
                alpha_mode: CompositeAlphaMode::Opaque,
            },
        )
        .expect("capacity swapchain");
    let pass = pass(&mut device, false);
    let layout = device
        .create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None,
            resource_set_layouts: vec![],
        })
        .expect("layout");
    let source = "@vertex fn vs(@builtin(vertex_index) v:u32)->@builtin(position) vec4<f32>{ let p=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));return vec4<f32>(p[v],0.0,1.0); } @fragment fn fs(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32>{if p.y<8.0{return vec4<f32>(1.0,0.0,0.0,1.0);}return vec4<f32>(0.0,0.0,1.0,1.0);}";
    let pipeline = pipeline(
        &mut device,
        source,
        pass,
        layout,
        gfx_core::BlendMode::Replace,
        false,
    );
    let draw = DrawStepDescriptor {
        pipeline,
        resource_sets: resource_set_list([]),
        vertex_count: 3,
        first_vertex: 0,
        instance_count: 1,
        first_instance: 0,
        scissor: None,
    };
    // SAFETY: this test owns the popup HWND; its client is now smaller than the retained image.
    unsafe {
        SetWindowPos(
            window.0,
            None,
            0,
            0,
            64,
            32,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER,
        )
    }
    .expect("native shrink");
    device
        .render_step_list_and_present_compat(
            chain,
            pass,
            RenderStepList::Draw(&[draw]),
            CLEAR,
            None,
        )
        .expect("cropped presentation");
    let swapchain = device.swapchains.get(chain).expect("live swapchain");
    device
        .make_window_current(&swapchain.window)
        .expect("native readback context");
    let mut pixel = [0_u8; 4];
    // SAFETY: the front buffer is the presented native window; the 1x1 output owns four bytes.
    unsafe {
        device.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        device.gl.read_buffer(glow::FRONT);
        device.gl.read_pixels(
            4,
            30,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixel)),
        );
        device.gl.read_buffer(glow::BACK);
    }
    device.check().expect("native pixel readback");
    device.park().expect("park native readback");
    assert_eq!(
        pixel,
        [255, 0, 0, 255],
        "top red stripe must stay at the client's top while retaining 64-pixel capacity"
    );
    device.destroy_swapchain(chain).expect("destroy chain");
    device.destroy_surface(surface).expect("destroy surface");
}

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
