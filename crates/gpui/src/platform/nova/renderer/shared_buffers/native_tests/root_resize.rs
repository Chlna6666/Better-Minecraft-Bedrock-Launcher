//! Verify a retained root's final RGBA across native resizes.
use super::*;
use std::{sync::Arc, time::Instant};

#[test]
#[cfg(feature = "nova-gfx-dx11")]
#[ignore = "requires Windows hardware D3D11"]
fn dx11_resize_preserves_root_alpha() {
    verify(RendererBackend::NovaDx11);
}

#[test]
#[cfg(feature = "nova-gfx-dx12")]
#[ignore = "requires Windows hardware D3D12"]
fn dx12_resize_preserves_root_alpha() {
    verify(RendererBackend::NovaDx12);
}

#[test]
#[cfg(feature = "nova-gfx-vulkan")]
#[ignore = "requires Windows hardware Vulkan"]
fn vulkan_resize_preserves_root_alpha() {
    verify(RendererBackend::NovaVulkan);
}

#[test]
#[cfg(feature = "nova-gfx-opengl")]
#[ignore = "requires Windows hardware OpenGL 4.5"]
fn opengl_resize_preserves_root_alpha() {
    verify(RendererBackend::NovaOpenGl);
}

fn packet(extent: (u32, u32)) -> crate::PresentationPacket {
    let bounds = crate::bounds(
        crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(0.)),
        crate::size(
            crate::ScaledPixels(extent.0 as f32),
            crate::ScaledPixels(extent.1 as f32),
        ),
    );
    let mut scene = crate::scene::Painter::default();
    // Cross the production complex-scene retention threshold used by the debug window.
    for _ in 0..400 {
        scene.insert_primitive(Quad {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            background: crate::rgb(0x00ff00).into(),
            ..Default::default()
        });
    }
    scene.finish();
    crate::PresentationPacket::new(
        Arc::new(crate::window::Paint::new(
            scene.into_scene(),
            Arc::default(),
            crate::window::PaintGeometry {
                scale_factor: 1.,
                layout_extent: Some(extent),
                ..Default::default()
            },
        )),
        [],
        [],
        Instant::now(),
        crate::DirtyRegion::full(bounds),
        crate::BackdropBlurDamagePlan::default(),
        crate::PartialPresentMode::FullRedraw,
    )
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
        matches!(
            backend,
            RendererBackend::NovaDx11 | RendererBackend::NovaDx12
        ),
        NovaRendererAtlas::new(),
    )
    .expect("retained resize renderer");
    renderer.set_live_resize_active(true);
    for extent in [(512, 512), (520, 500), (530, 480), (520, 460), (500, 512)] {
        window.resize(extent.0, extent.1);
        renderer.update_drawable_size(
            crate::size(DevicePixels(extent.0 as i32), DevicePixels(extent.1 as i32)),
            None,
        );
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while renderer
            .presentation_readiness(None, Some(extent))
            .expect("root resize readiness")
            != PresentReadiness::Ready
            || !renderer.draw(packet(extent)).expect("root resize draw")
        {
            assert!(Instant::now() < deadline, "root resize must present");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        renderer
            .wait_pending_submissions()
            .expect("root retirement");
        assert!(renderer.root_color.target.is_some(), "retained root");
        verify_composite(&renderer);
    }
    renderer.destroy();
}

fn verify_composite(renderer: &NovaRenderer) {
    let output = super::target(renderer);
    device!(renderer, device, {
        device
            .render_to_textures(&[RenderPassCommands {
                texture_view: output.1,
                render_pass: renderer.render_pass,
                commands: renderer.command_scratch.commands(),
                color_load_op: LoadOp::Clear(clear_color()),
                clear_region: None,
                depth_attachment: Some(DepthAttachment {
                    target: renderer.depth_texture_view,
                    depth_load_op: LoadOp::Clear(1.),
                }),
                viewport: None,
            }])
            .expect("root composite reference");
    });
    super::pixels(renderer, output.0, [0, 255, 0, 255]);
    device!(renderer, device, {
        device
            .destroy_texture_view(output.1)
            .expect("composite view");
        device.destroy_texture(output.0).expect("composite texture");
    });
}
