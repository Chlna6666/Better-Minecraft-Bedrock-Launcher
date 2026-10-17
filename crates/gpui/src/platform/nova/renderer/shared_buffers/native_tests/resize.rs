//! Native resize must converge and rebuild filters from the new scene exactly once.
use super::*;
use std::time::Instant;

#[test]
#[cfg(feature = "nova-gfx-dx11")]
#[ignore = "requires Windows hardware D3D11"]
fn dx11_resize_uses_new_scene_targets() {
    verify(RendererBackend::NovaDx11);
}

#[test]
#[cfg(feature = "nova-gfx-dx12")]
#[ignore = "requires Windows hardware D3D12"]
fn dx12_resize_uses_new_scene_targets() {
    verify(RendererBackend::NovaDx12);
}

#[test]
#[cfg(feature = "nova-gfx-vulkan")]
#[ignore = "requires Windows hardware Vulkan"]
fn vulkan_resize_uses_new_scene_targets() {
    verify(RendererBackend::NovaVulkan);
}

#[test]
#[cfg(feature = "nova-gfx-opengl")]
#[ignore = "requires Windows hardware OpenGL 4.5"]
fn opengl_resize_uses_new_scene_targets() {
    verify(RendererBackend::NovaOpenGl);
}

fn packet(renderer: &NovaRenderer, blur: bool) -> crate::PresentationPacket {
    let mut scene = super::scene(renderer, 0x00ff00);
    let viewport = scene.quads[0].bounds;
    if blur {
        scene.insert_primitive(crate::PaintBackdropBlur {
            order: 0,
            animation_id: None,
            bounds: viewport,
            content_mask: crate::ContentMask::new(viewport),
            radius: crate::ScaledPixels(2.),
            downsample: 2,
            levels: 1,
            opacity: 1.,
            saturation: 1.,
            corner_radii: Default::default(),
            tint: None,
            recompute_overlap: false,
        });
        scene.finish();
    }
    let mut damage = crate::DirtyRegion::empty();
    damage.mark_full(viewport);
    crate::PresentationPacket::new(
        std::sync::Arc::new(crate::window::Paint::new(
            scene.into_scene(),
            std::sync::Arc::default(),
            crate::window::PaintGeometry {
                scale_factor: 1.,
                ..Default::default()
            },
        )),
        [],
        [],
        Instant::now(),
        damage,
        crate::BackdropBlurDamagePlan::default(),
        crate::PartialPresentMode::FullRedraw,
    )
}

fn present(renderer: &mut NovaRenderer, blur: bool) {
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while !renderer
        .draw(packet(renderer, blur))
        .expect("native resize draw")
    {
        assert!(
            Instant::now() < deadline,
            "resize must not strand the final frame"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    renderer
        .wait_for_pending_submissions()
        .expect("frame retirement");
}

fn verify(backend: RendererBackend) {
    let window = native_window::Window::renderer_fixture();
    window.resize(512, 512);
    let mut renderer = NovaRenderer::with_atlas(
        &window,
        backend,
        &RendererOptions::default(),
        GpuSubmissionMode::Deferred,
        crate::size(DevicePixels(512), DevicePixels(512)),
        false,
        NovaRendererAtlas::new(),
    )
    .expect("native resize renderer");
    present(&mut renderer, true);
    assert!(renderer.filters.targets.is_some());
    for (width, height, blur) in [(640, 480, false), (400, 360, true), (512, 512, true)] {
        verify_extent(&window, &mut renderer, (width, height), blur);
    }
    verify_live_resize(&window, &mut renderer);
    renderer.destroy();
}

fn geometry_packet(extent: (u32, u32)) -> crate::PresentationPacket {
    let viewport = crate::bounds(
        crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(0.)),
        crate::size(
            crate::ScaledPixels(extent.0 as f32),
            crate::ScaledPixels(extent.1 as f32),
        ),
    );
    let marker = crate::bounds(
        crate::point(
            crate::ScaledPixels(extent.0 as f32 - 24.),
            crate::ScaledPixels(extent.1 as f32 - 24.),
        ),
        crate::size(crate::ScaledPixels(8.), crate::ScaledPixels(8.)),
    );
    let mut scene = crate::scene::Painter::default();
    for (bounds, color) in [(viewport, 0x00ff00), (marker, 0xff0000)] {
        scene.insert_primitive(crate::Quad {
            bounds,
            content_mask: crate::ContentMask::new(viewport),
            background: crate::rgb(color).into(),
            ..Default::default()
        });
    }
    scene.finish();
    crate::PresentationPacket::new(
        std::sync::Arc::new(crate::window::Paint::new(
            scene.into_scene(),
            std::sync::Arc::default(),
            crate::window::PaintGeometry {
                scale_factor: 1.,
                layout_extent: Some(extent),
                ..Default::default()
            },
        )),
        [],
        [],
        Instant::now(),
        crate::DirtyRegion::full(viewport),
        crate::BackdropBlurDamagePlan::default(),
        crate::PartialPresentMode::FullRedraw,
    )
}

fn present_geometry(renderer: &mut NovaRenderer, extent: (u32, u32)) {
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while !renderer
        .draw(geometry_packet(extent))
        .expect("live geometry draw")
    {
        assert!(
            Instant::now() < deadline,
            "live frame must not stop until pointer release"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    renderer
        .wait_for_pending_submissions()
        .expect("live frame retirement");
    verify_marker(renderer, extent);
}

fn verify_marker(renderer: &NovaRenderer, extent: (u32, u32)) {
    let output = super::target(renderer);
    device!(renderer, device, {
        device
            .render_step_lists_to_textures_compat(&[gfx_core::TextureRenderStepList {
                texture_view: output.1,
                render_pass: renderer.render_pass,
                steps: RenderStepList::from_render_steps(renderer.draw_step_scratch.steps()),
                color_load_op: LoadOp::Clear(clear_color()),
                clear_region: None,
                depth_attachment: Some(RenderPassDepthAttachment {
                    target: renderer.depth_texture_view,
                    depth_load_op: LoadOp::Clear(1.),
                }),
                viewport: Some(gfx_core::RenderViewport {
                    extent: Extent2d::new(extent.0, extent.1).expect("client viewport"),
                    reference_extent: Extent2d::new(
                        renderer.current_size.width,
                        renderer.current_size.height,
                    )
                    .expect("retained target extent"),
                }),
            }])
            .expect("live geometry pixel draw");
        let pixels = device.read_texture(output.0).expect("live geometry pixels");
        let target_width = renderer.current_size.width;
        let target_height = renderer.current_size.height;
        for (x, y, expected) in [
            (extent.0 - 20, extent.1 - 20, [0, 0, 255, 255]),
            (8, 8, [0, 255, 0, 255]),
            (extent.0 - 2, extent.1 - 2, [0, 255, 0, 255]),
        ] {
            let readback_y = if renderer.backend_info.label() == "nova-opengl" {
                target_height - extent.1 + y
            } else {
                y
            };
            let offset = readback_y as usize * pixels.bytes_per_row as usize + x as usize * 4;
            assert_eq!(
                &pixels.bytes[offset..offset + 4],
                &expected,
                "the current scene must retain native client-pixel geometry at ({x},{y})"
            );
        }
        assert!(target_width >= extent.0 && target_height >= extent.1);
        device
            .destroy_texture_view(output.1)
            .expect("live output view");
        device
            .destroy_texture(output.0)
            .expect("live output texture");
    });
}

fn verify_live_resize(window: &native_window::Window, renderer: &mut NovaRenderer) {
    // A live client and the actual swapchain must use a single native-pixel
    // extent. In particular no geometric headroom may silently cause DXGI
    // to crop an enlarged backing buffer to a smaller source region.
    renderer.set_live_resize_active(true);
    for extent in [(520, 520), (525, 522), (530, 528)] {
        window.resize(extent.0, extent.1);
        renderer.update_drawable_size(
            crate::size(DevicePixels(extent.0 as i32), DevicePixels(extent.1 as i32)),
            None,
        );
        present_geometry(renderer, extent);
        assert_eq!(
            renderer.committed_drawable_extent(),
            extent,
            "every rendered Scene must target the exact active client extent"
        );
    }
    window.resize(500, 500);
    renderer.update_drawable_size(crate::size(DevicePixels(500), DevicePixels(500)), None);
    present_geometry(renderer, (500, 500));
    assert_eq!(
        renderer.committed_drawable_extent(),
        (500, 500),
        "native shrink must not leave a larger DXGI backbuffer"
    );
    renderer.set_live_resize_active(false);
    present_geometry(renderer, (500, 500));
    assert_eq!(
        renderer.committed_drawable_extent(),
        (500, 500),
        "pointer release must settle to the exact last client size"
    );
}

fn verify_extent(
    window: &native_window::Window,
    renderer: &mut NovaRenderer,
    extent: (u32, u32),
    blur: bool,
) {
    let (width, height) = extent;
    window.resize(width, height);
    let size = crate::size(DevicePixels(width as i32), DevicePixels(height as i32));
    let started = Instant::now();
    renderer.update_drawable_size(size, None);
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while !renderer
        .apply_pending_drawable_size(None, None)
        .expect("exact resize")
    {
        assert!(Instant::now() < deadline, "pending resize must converge");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        renderer.filters.targets.is_none(),
        "resize must not allocate stale blur topology"
    );
    present(renderer, blur);
    assert_eq!(renderer.committed_drawable_extent(), extent);
    assert_eq!(renderer.filters.targets.is_some(), blur);
    verify_pixels(renderer);
    println!(
        "resize backend={} extent={width}x{height} blur={blur} resize_draw_readback_us={}",
        renderer.backend_info.label(),
        started.elapsed().as_micros()
    );
    let depth = renderer.depth_texture;
    let filters = renderer
        .filters
        .targets
        .as_ref()
        .map(BackdropBlurTargets::byte_size);
    renderer.resize(size).expect("same-size resize");
    assert_eq!(
        renderer.depth_texture, depth,
        "same-size resize reuses resources"
    );
    assert_eq!(
        renderer
            .filters
            .targets
            .as_ref()
            .map(BackdropBlurTargets::byte_size),
        filters
    );
}

fn verify_pixels(renderer: &NovaRenderer) {
    let output = super::target(renderer);
    device!(renderer, device, {
        device
            .render_step_list_to_texture_compat(
                output.1,
                renderer.render_pass,
                RenderStepList::from_render_steps(renderer.draw_step_scratch.steps()),
                LoadOp::Clear(clear_color()),
                Some(RenderPassDepthAttachment {
                    target: renderer.depth_texture_view,
                    depth_load_op: LoadOp::Clear(1.),
                }),
            )
            .expect("resize pixel reference");
    });
    super::pixels(renderer, output.0, [0, 255, 0, 255]);
    device!(renderer, device, {
        device
            .destroy_texture_view(output.1)
            .expect("output view retirement");
        device.destroy_texture(output.0).expect("output retirement");
    });
}
