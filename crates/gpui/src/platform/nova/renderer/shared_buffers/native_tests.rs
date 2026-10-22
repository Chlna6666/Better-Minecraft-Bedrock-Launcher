//! Uses production slot selection/rebinding and native pixels, without an application UI loop.
use super::*;
use crate::platform::nova::tests::sprite_gpu::native_window;
use gfx_core::{DiagnosticsDevice, ResourceDevice, SubmissionDevice, TextureTransferDevice};

macro_rules! device {
    ($renderer:expr, $device:ident, $body:block) => {{
        let backend = $renderer.backend.clone();
        let mut backend = lock_backend(&backend);
        match &mut *backend {
            #[cfg(feature = "nova-gfx-dx11")]
            NovaBackend::Dx11($device) => $body,
            #[cfg(feature = "nova-gfx-dx12")]
            NovaBackend::Dx12($device) => $body,
            #[cfg(feature = "nova-gfx-vulkan")]
            NovaBackend::Vulkan($device) => $body,
            #[cfg(feature = "nova-gfx-opengl")]
            NovaBackend::OpenGl($device) => $body,
            #[allow(unreachable_patterns)]
            _ => panic!("unsupported native test backend"),
        }
    }};
}

mod chunks;
mod reactive;
mod resize;
mod retained_color;
mod root_resize;

#[test]
#[cfg(feature = "nova-gfx-dx11")]
#[ignore = "requires Windows hardware D3D11"]
fn dx11_static_buffer_versions_preserve_in_flight_pixels() {
    verify(RendererBackend::NovaDx11);
}
#[test]
#[cfg(feature = "nova-gfx-dx12")]
#[ignore = "requires Windows hardware D3D12"]
fn dx12_static_buffer_versions_preserve_in_flight_pixels() {
    verify(RendererBackend::NovaDx12);
}
#[test]
#[cfg(feature = "nova-gfx-vulkan")]
#[ignore = "requires Windows hardware Vulkan"]
fn vulkan_static_buffer_versions_preserve_in_flight_pixels() {
    verify(RendererBackend::NovaVulkan);
}
#[test]
#[cfg(feature = "nova-gfx-opengl")]
#[ignore = "requires Windows hardware OpenGL 4.5"]
fn opengl_static_buffer_versions_preserve_in_flight_pixels() {
    verify(RendererBackend::NovaOpenGl);
}

fn scene(renderer: &NovaRenderer, color: u32) -> crate::scene::Painter {
    let bounds = crate::bounds(
        crate::point(crate::px(0.0), crate::px(0.0)),
        crate::size(
            crate::px(renderer.current_size.width as f32),
            crate::px(renderer.current_size.height as f32),
        ),
    )
    .scale(1.0);
    let mut scene = crate::scene::Painter::default();
    scene.insert_primitive(Quad {
        bounds,
        content_mask: crate::ContentMask {
            bounds,
            corner_bounds: bounds,
            ..Default::default()
        },
        background: crate::rgb(color).into(),
        ..Default::default()
    });
    scene.finish();
    scene
}

fn upload(
    renderer: &mut NovaRenderer,
    slot: usize,
    scene: &crate::Scene,
    retention: &crate::window::retained::RetainedPaint,
) -> usize {
    renderer
        .activate_frame_resources(slot)
        .expect("activate slot");
    let drawable_size = renderer.current_size;
    renderer.pack_scene(
        scene,
        retention,
        &[],
        drawable_size,
        BackdropBlurQuality::Full,
    );
    renderer
        .prepare_static_buffers()
        .expect("select static versions");
    renderer
        .ensure_stream_capacity()
        .expect("animation table capacity");
    let dirty = renderer.retained_upload.static_upload_mask(slot);
    let plan = renderer
        .retained_upload
        .quad_upload_plan(slot, renderer.frame_upload.quads.len());
    let mono_sprite_plan = renderer
        .retained_upload
        .monochrome_sprite_upload_plan(slot, renderer.frame_upload.mono_sprites.len());
    let full_quad_bytes = if dirty.quad {
        renderer.frame_upload.quads.len()
    } else {
        0
    };
    let bytes = dirty.mapped_upload_bytes(&renderer.frame_upload, false) - full_quad_bytes
        + plan.uploaded_bytes(renderer.frame_upload.quads.len())
        - if dirty.mono_sprite {
            renderer.frame_upload.mono_sprites.len()
        } else {
            0
        }
        + mono_sprite_plan.uploaded_bytes(renderer.frame_upload.mono_sprites.len());
    let buffers = renderer.frame_buffer_targets();
    device!(renderer, device, {
        super::super::buffer_upload::upload_frame_buffers(
            device,
            super::super::buffer_upload::FrameBufferUpload {
                buffers,
                source: &renderer.frame_upload,
                has_compositor_descriptors: false,
                static_uploads: dirty,
                quad_upload_plan: &plan,
                mono_sprite_upload_plan: &mono_sprite_plan,
            },
        )
        .expect("static upload");
    });
    renderer.retained_upload.mark_uploaded(slot);
    bytes
}

fn target(renderer: &NovaRenderer) -> (TextureId, TextureViewId) {
    device!(renderer, device, {
        let texture = device
            .create_texture(&TextureDescriptor {
                label: Some("P2-A slot pixels".into()),
                size: renderer.surface_config.size,
                mip_level_count: 1,
                format: Format::Bgra8Unorm,
                usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
                memory_location: MemoryLocation::GpuOnly,
                dimension: TextureDimension::D2,
            })
            .expect("target");
        let view = device
            .create_texture_view(&TextureViewDescriptor {
                label: None,
                texture,
                format: Format::Bgra8Unorm,
                base_mip_level: 0,
                mip_level_count: 1,
            })
            .expect("target view");
        (texture, view)
    })
}

fn quad_step(renderer: &NovaRenderer) -> DrawCommand {
    DrawCommand {
        pipeline: renderer.pipelines.alpha.quads,
        bind_groups: bind_group_list([renderer.quad_resource_set]),
        vertex_count: 4,
        instance_count: 1,
        first_vertex: 0,
        first_instance: 0,
        scissor: None,
    }
}

fn draw(renderer: &NovaRenderer, view: TextureViewId, slot: usize) {
    let step = RenderCommand::Draw(DrawCommand {
        bind_groups: bind_group_list([renderer.frame_resources[slot]
            .bind_groups
            .quad_resource_set]),
        ..quad_step(renderer)
    });
    device!(renderer, device, {
        device
            .render_to_textures(&[RenderPassCommands {
                texture_view: view,
                render_pass: renderer.render_pass,
                commands: std::slice::from_ref(&step),
                color_load_op: LoadOp::Clear(clear_color()),
                clear_region: None,
                depth_attachment: Some(DepthAttachment {
                    target: renderer.depth_texture_view,
                    depth_load_op: LoadOp::Clear(1.0),
                }),
                viewport: None,
            }])
            .expect("slot draw");
    });
}

fn pixels(renderer: &NovaRenderer, texture: TextureId, expected: [u8; 4]) {
    device!(renderer, device, {
        let readback = device.read_texture(texture).expect("native readback");
        let offset = (renderer.current_size.height / 2) as usize * readback.bytes_per_row as usize
            + (renderer.current_size.width / 2) as usize * 4;
        assert_eq!(&readback.bytes[offset..offset + 4], &expected);
    });
}

fn verify(backend: RendererBackend) {
    let window = native_window::Window::renderer_fixture();
    let mut renderer = NovaRenderer::with_atlas(
        &window,
        backend,
        &RendererOptions::default(),
        GpuSubmissionMode::Deferred,
        crate::size(DevicePixels(16), DevicePixels(16)),
        false,
        NovaRendererAtlas::new(),
    )
    .expect("native renderer");
    let initial: FxHashSet<_> = renderer
        .frame_resources
        .iter()
        .flat_map(|frame| frame.buffers.ids())
        .collect();
    assert_eq!(initial.len(), 14, "ten static plus four slot-local buffers");
    let initial_bytes: u64 = renderer
        .buffer_memory_profile()
        .iter()
        .map(|stream| stream.gpu_capacity_bytes)
        .sum();
    assert_eq!(initial_bytes, 1_445_456);
    let red = scene(&renderer, 0xff0000);
    let green = scene(&renderer, 0x00ff00);
    let first_bytes = upload(
        &mut renderer,
        0,
        &red,
        &crate::window::retained::RetainedPaint::default(),
    );
    let second_bytes = upload(
        &mut renderer,
        1,
        &red,
        &crate::window::retained::RetainedPaint::default(),
    );
    assert_eq!(second_bytes, GLOBAL_UPLOAD_BYTES);
    assert!(first_bytes > second_bytes);
    for stream in BufferStream::ALL {
        assert_eq!(
            stream.allocation(renderer.frame_resources[0].buffers).0,
            stream.allocation(renderer.frame_resources[1].buffers).0
        );
    }
    let old = renderer.quad_buffer;
    let old_target = target(&renderer);
    let new_target = target(&renderer);
    draw(&renderer, old_target.1, 1);
    // A tracked queue marker follows the old draw. Keep its real SubmissionId attached to slot 1
    // until explicitly waiting, even on a GPU that completes before the following CPU inspection.
    let steps = vec![RenderCommand::Draw(quad_step(&renderer)); 512];
    let submission = device!(renderer, device, {
        device
            .render_and_present(&PresentFrameDescriptor {
                swapchain: renderer.swapchain,
                render_pass: renderer.render_pass,
                commands: &steps,
                color_load_op: LoadOp::Clear(clear_color()),
                depth_attachment: Some(DepthAttachment {
                    target: renderer.depth_texture_view,
                    depth_load_op: LoadOp::Clear(1.0),
                }),
                present_region: None,
            })
            .expect("old slot submission")
            .and_then(|frame: gfx_core::PresentedFrame| frame.submission)
            .expect("old slot submission id")
    });
    renderer.pending_submissions.push(PendingSubmission {
        submission,
        frame_resource_index: 1,
        publication: None,
    });
    let pending_at_switch = device!(renderer, device, {
        matches!(
            SubmissionDevice::poll_submission(device, submission).expect("submission status"),
            SubmissionStatus::Pending
        )
    });
    assert!(
        pending_at_switch,
        "native gate requires an unfinished old submission before switching versions"
    );
    assert!(
        renderer.prepare_static_buffers().is_err(),
        "cannot mutate the pending slot"
    );
    renderer.activate_frame_resources(0).expect("free slot");
    let drawable_size = renderer.current_size;
    renderer.pack_scene(
        &green,
        &crate::window::retained::RetainedPaint::default(),
        &[],
        drawable_size,
        BackdropBlurQuality::Full,
    );
    let old_ids = renderer.frame_resources[0].buffers.ids();
    let count = device!(renderer, device, { device.resource_stats().buffers });
    let layout = renderer.quad_resource_set_layout;
    renderer.quad_resource_set_layout = BindGroupLayoutId::from_parts(u32::MAX, u32::MAX);
    assert!(
        renderer.prepare_static_buffers().is_err(),
        "rebind failure is reported"
    );
    renderer.quad_resource_set_layout = layout;
    assert_eq!(renderer.frame_resources[0].buffers.ids(), old_ids);
    assert_eq!(
        device!(renderer, device, { device.resource_stats().buffers }),
        count,
        "failed transaction retires all new buffers"
    );
    let changed_bytes = upload(
        &mut renderer,
        0,
        &green,
        &crate::window::retained::RetainedPaint::default(),
    );
    assert_eq!(changed_bytes, PACKED_QUAD_BYTES);
    assert_ne!(renderer.quad_buffer, old);
    assert_eq!(renderer.frame_resources[1].buffers.quad_buffer, old);
    draw(&renderer, new_target.1, 0);
    draw(&renderer, old_target.1, 1);
    renderer
        .coalesce_idle_static_buffers()
        .expect("keep pending version");
    assert_eq!(renderer.frame_resources[1].buffers.quad_buffer, old);
    pixels(&renderer, old_target.0, [0, 0, 255, 255]);
    pixels(&renderer, new_target.0, [0, 255, 0, 255]);
    renderer
        .wait_pending_submissions()
        .expect("old fence completion");
    renderer
        .coalesce_idle_static_buffers()
        .expect("coalesce idle version without another frame");
    assert_eq!(renderer.current_frame_resource_index, 0);
    assert_eq!(
        upload(
            &mut renderer,
            1,
            &green,
            &crate::window::retained::RetainedPaint::default()
        ),
        0,
        "converged content inherits without uploads"
    );
    assert_eq!(
        renderer.frame_resources[0].buffers.quad_buffer,
        renderer.frame_resources[1].buffers.quad_buffer
    );
    assert_eq!(
        device!(renderer, device, { device.resource_stats().buffers }),
        count
    );
    assert_eq!(
        renderer
            .buffer_memory_profile()
            .iter()
            .map(|stream| stream.gpu_capacity_bytes)
            .sum::<u64>(),
        initial_bytes
    );
    let shared = renderer.quad_buffer;
    assert_eq!(
        upload(
            &mut renderer,
            0,
            &red,
            &crate::window::retained::RetainedPaint::default()
        ),
        PACKED_QUAD_BYTES
    );
    assert_eq!(renderer.quad_buffer, shared, "idle aliases update in place");
    assert_eq!(renderer.frame_resources[1].buffers.quad_buffer, shared);
    assert!(
        renderer
            .retained_upload
            .stream_state(1, BufferStream::Quad)
            .2
    );
    // Returning to the other slot's old scene must upload again, never inherit its stale token.
    assert_eq!(
        upload(
            &mut renderer,
            1,
            &green,
            &crate::window::retained::RetainedPaint::default()
        ),
        PACKED_QUAD_BYTES
    );
    assert_eq!(renderer.quad_buffer, shared);
    draw(&renderer, new_target.1, 1);
    pixels(&renderer, new_target.0, [0, 255, 0, 255]);
    assert_eq!(
        device!(renderer, device, { device.resource_stats().buffers }),
        count
    );
    println!(
        "P2-A {backend}: initial={initial_bytes} B unique_buffers=14 static_upload={first_bytes}->{second_bytes} B change={changed_bytes} B pending_at_switch={pending_at_switch}; old/new native pixels, rollback and idle in-place updates verified"
    );
    device!(renderer, device, {
        for (texture, view) in [old_target, new_target] {
            device.destroy_texture_view(view).expect("view cleanup");
            device.destroy_texture(texture).expect("texture cleanup");
        }
    });
    chunks::verify(&mut renderer);
    renderer.destroy();
}
