//! An in-progress draw must present even while the native client and producer run ahead.
use super::*;
use crate::{
    BackdropBlurDamagePlan, DevicePixels, DirtyRegion, GpuSubmissionMode, PartialPresentMode,
    RendererBackend, RendererOptions, ScaledPixels, bounds,
    platform::nova::{NovaRendererAtlas, native_window},
    point, size,
};
use std::time::Duration;

#[test]
#[cfg(feature = "nova-gfx-dx11")]
#[ignore = "requires Windows hardware D3D11 and a desktop session"]
fn dx11_resize_draw_progresses_with_newer_frame_queued() {
    verify_progress(RendererBackend::NovaDx11);
}

#[test]
#[cfg(feature = "nova-gfx-dx12")]
#[ignore = "requires Windows hardware D3D12 and a desktop session"]
fn dx12_resize_draw_progresses_with_newer_frame_queued() {
    verify_progress(RendererBackend::NovaDx12);
}

#[test]
#[cfg(feature = "nova-gfx-vulkan")]
#[ignore = "requires Windows hardware Vulkan and a desktop session"]
fn vulkan_resize_draw_progresses_with_newer_frame_queued() {
    verify_progress(RendererBackend::NovaVulkan);
}

#[test]
#[cfg(feature = "nova-gfx-opengl")]
#[ignore = "requires Windows hardware OpenGL 4.5 and a desktop session"]
fn opengl_resize_draw_progresses_with_newer_frame_queued() {
    verify_progress(RendererBackend::NovaOpenGl);
}

fn packet(extent: (u32, u32)) -> PresentationPacket {
    let viewport = bounds(
        point(ScaledPixels(0.), ScaledPixels(0.)),
        size(ScaledPixels(extent.0 as f32), ScaledPixels(extent.1 as f32)),
    );
    let mut painter = crate::scene::Painter::default();
    painter.insert_primitive(crate::Quad {
        bounds: viewport,
        content_mask: crate::ContentMask::new(viewport),
        background: crate::rgb(0x00ff00).into(),
        ..Default::default()
    });
    painter.finish();
    PresentationPacket::new(
        Arc::new(crate::window::Paint::new(
            painter.into_scene(),
            Arc::default(),
            crate::window::PaintGeometry {
                layout_extent: Some(extent),
                layout_epoch: u64::from(extent.0),
                scale_factor: 1.,
                ..Default::default()
            },
        )),
        [],
        [],
        Instant::now(),
        DirtyRegion::full(viewport),
        BackdropBlurDamagePlan::default(),
        PartialPresentMode::FullRedraw,
    )
}

fn verify_progress(backend: RendererBackend) {
    let window = native_window::Window::renderer_fixture();
    let renderer = NovaRenderer::with_atlas(
        &window,
        backend,
        &RendererOptions::default(),
        GpuSubmissionMode::Deferred,
        size(DevicePixels(512), DevicePixels(512)),
        false,
        NovaRendererAtlas::new(),
    )
    .expect("native renderer");
    let queue = Arc::new(Queue::default());
    let mut entry = Entry::new(
        renderer,
        queue.clone(),
        Arc::new(Status::default()),
        Box::new(()),
        Arc::new(|_| {}),
    );
    entry.scene = Some((packet((512, 512)), false));
    // The HWND and newest unstarted Scene are already ahead of this in-progress draw.
    window.resize(640, 480);
    queue
        .enqueue(
            Command::Draw {
                packet: packet((640, 480)),
                framebuffer_only: false,
                reply: None,
                resize: Some(super::super::queue::DrawableResize {
                    size: size(DevicePixels(640), DevicePixels(480)),
                    epoch: Some(640),
                }),
            },
            || Ok(()),
        )
        .expect("queue next geometry");
    present_scene(&mut entry, (512, 512));
    assert!(queue.has_pending_resize_draw(), "newer work stays queued");
    let next = queue.take().expect("next geometry");
    entry.execute(next);
    if entry.scene.is_some() {
        present_scene(&mut entry, (640, 480));
    }
    assert_eq!(entry.renderer.committed_drawable_extent(), (640, 480));
}

fn present_scene(entry: &mut Entry, extent: (u32, u32)) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !entry
        .ready(Some(u64::from(extent.0)), Some(extent))
        .expect("frame readiness")
    {
        assert!(Instant::now() < deadline, "native frame must become ready");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        entry.present_scene().expect("present current scene"),
        "a ready in-progress Scene must not be cancelled by newer queued work"
    );
}
