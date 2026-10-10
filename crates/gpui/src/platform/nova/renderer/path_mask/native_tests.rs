//! Read cached production mask pixels through a test-only sampled color target.
use super::*;
use crate::platform::nova::tests::sprite_gpu::native_window;
use gfx_core::{ResourceDevice, TextureTransferDevice};

#[test]
#[ignore = "requires Windows hardware D3D12"]
fn dx12_resident_mask_preserves_pixels_across_repeated_presentations() {
    let window = native_window::Window::renderer_fixture();
    let mut renderer = NovaRenderer::with_atlas(
        &window,
        RendererBackend::NovaDx12,
        &RendererOptions::default(),
        GpuSubmissionMode::Deferred,
        crate::size(DevicePixels(16), DevicePixels(16)),
        false,
        NovaRendererAtlas::new(),
    )
    .expect("native renderer");
    let target = target(&renderer);
    let window_id = u64::MAX - 100;
    let red = scene(0xff0000, 0x0000ff);
    present(&mut renderer, &red, window_id);
    let first = pixels(&renderer, target);
    assert_eq!(&first[(4 * 16 + 4) * 4..][..4], &[0, 0, 255, 255]);
    assert_eq!(&first[(12 * 16 + 12) * 4..][..4], &[0, 0, 0, 0]);
    for _ in 0..5 {
        present(&mut renderer, &red, window_id);
        assert_eq!(pixels(&renderer, target), first, "resident mask pixels");
    }
    let unrelated_update = scene(0xff0000, 0xffff00);
    assert_ne!(red.revision, unrelated_update.revision);
    present(&mut renderer, &unrelated_update, window_id);
    let updated = pixels(&renderer, target);
    assert_eq!(&updated[(4 * 16 + 4) * 4..][..4], &[0, 0, 255, 255]);
    assert_ne!(
        updated, first,
        "quad pixels changed without rerasterizing the mask"
    );
    let metrics = crate::window_metrics_snapshot()
        .into_iter()
        .find(|metrics| metrics.window_id == window_id)
        .expect("window mask metrics");
    assert_eq!(metrics.path_mask.rendered_frames, 1);
    assert_eq!(metrics.path_mask.skipped_frames, 6);

    present(&mut renderer, &scene(0x00ff00, 0xffff00), window_id);
    let changed = pixels(&renderer, target);
    assert_eq!(&changed[(4 * 16 + 4) * 4..][..4], &[0, 255, 0, 255]);
    assert_ne!(changed, first);
    let metrics = crate::window_metrics_snapshot()
        .into_iter()
        .find(|metrics| metrics.window_id == window_id)
        .expect("updated mask metrics");
    assert_eq!(metrics.path_mask.rendered_frames, 2);
    assert_eq!(metrics.path_mask.skipped_frames, 6);
    for (index, variant) in [PathChange::Geometry, PathChange::Clip, PathChange::Scale]
        .into_iter()
        .enumerate()
    {
        let scene = changed_scene(0x00ff00, 0xffff00, variant);
        present(&mut renderer, &scene, window_id);
        assert_ne!(pixels(&renderer, target), changed, "path pixels changed");
        let metrics = crate::window_metrics_snapshot()
            .into_iter()
            .find(|metrics| metrics.window_id == window_id)
            .expect("changed path metrics");
        assert_eq!(metrics.path_mask.rendered_frames, 3 + index as u64);
        assert_eq!(metrics.path_mask.skipped_frames, 6);
    }
    let mut backend = lock_backend(&renderer.backend);
    let NovaBackend::Dx12(device) = &mut *backend else {
        panic!("DX12 fixture");
    };
    device
        .destroy_texture_view(target.1)
        .expect("destroy scratch view");
    device
        .destroy_texture(target.0)
        .expect("destroy scratch texture");
}

fn scene(color: u32, quad_color: u32) -> Arc<crate::Scene> {
    changed_scene(color, quad_color, PathChange::None)
}

enum PathChange {
    None,
    Geometry,
    Clip,
    Scale,
}

fn changed_scene(color: u32, quad_color: u32, change: PathChange) -> Arc<crate::Scene> {
    let mut path = crate::Path::new(crate::point(crate::px(2.0), crate::px(2.0)));
    path.line_to(crate::point(crate::px(14.0), crate::px(2.0)));
    let end_x = if matches!(change, PathChange::Geometry) {
        10.0
    } else {
        2.0
    };
    path.line_to(crate::point(crate::px(end_x), crate::px(14.0)));
    path.color = crate::rgb(color).into();
    let bounds = crate::bounds(
        crate::point(crate::px(0.0), crate::px(0.0)),
        crate::size(crate::px(16.0), crate::px(16.0)),
    );
    path.content_mask = crate::ContentMask {
        bounds,
        corner_bounds: bounds,
        ..Default::default()
    };
    if matches!(change, PathChange::Clip) {
        path.content_mask.bounds.size.width = crate::px(4.0);
    }
    let mut scene = crate::Scene::default();
    scene.push_layer(bounds.scale(1.0));
    let scale = if matches!(change, PathChange::Scale) {
        0.5
    } else {
        1.0
    };
    scene.insert_primitive(path.scale(scale));
    let quad_bounds = crate::bounds(
        crate::point(crate::px(0.0), crate::px(0.0)),
        crate::size(crate::px(1.0), crate::px(1.0)),
    )
    .scale(1.0);
    scene.insert_primitive(crate::Quad {
        bounds: quad_bounds,
        content_mask: crate::ContentMask {
            bounds: quad_bounds,
            ..Default::default()
        },
        background: crate::rgb(quad_color).into(),
        ..Default::default()
    });
    scene.finish();
    Arc::new(scene)
}

fn present(renderer: &mut NovaRenderer, scene: &Arc<crate::Scene>, window_id: u64) {
    let mut packet = PresentationPacket::new(
        Arc::clone(scene),
        [],
        [],
        Instant::now(),
        1.0,
        crate::DirtyRegion::empty(),
        crate::BackdropBlurDamagePlan::default(),
        PartialPresentMode::FullRedraw,
    );
    packet.window_id = window_id;
    assert!(renderer.present_framebuffer_only(packet).expect("present"));
}

fn target(renderer: &NovaRenderer) -> (TextureId, TextureViewId) {
    let mut backend = lock_backend(&renderer.backend);
    let NovaBackend::Dx12(device) = &mut *backend else {
        panic!("DX12 fixture");
    };
    let texture = device
        .create_texture(&TextureDescriptor {
            label: Some("resident mask readback scratch".into()),
            size: renderer.surface_config.size,
            mip_level_count: 1,
            format: Format::Bgra8Unorm,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        })
        .expect("scratch texture");
    let view = device
        .create_texture_view(&TextureViewDescriptor {
            label: None,
            texture,
            format: Format::Bgra8Unorm,
            base_mip_level: 0,
            mip_level_count: 1,
        })
        .expect("scratch view");
    (texture, view)
}

fn pixels(renderer: &NovaRenderer, target: (TextureId, TextureViewId)) -> Vec<u8> {
    let mut backend = lock_backend(&renderer.backend);
    let NovaBackend::Dx12(device) = &mut *backend else {
        panic!("DX12 fixture");
    };
    device
        .render_step_list_to_texture_compat(
            target.1,
            renderer.render_pass,
            RenderStepList::from_render_steps(renderer.draw_step_scratch.steps()),
            LoadOp::Clear(clear_color()),
            Some(RenderPassDepthAttachment {
                target: renderer.depth_texture_view,
                depth_load_op: LoadOp::Clear(1.0),
            }),
        )
        .expect("sample cached mask");
    let readback = device.read_texture(target.0).expect("scratch readback");
    let mut pixels = Vec::with_capacity(16 * 16 * 4);
    for row in readback
        .bytes
        .chunks_exact(readback.bytes_per_row as usize)
        .take(16)
    {
        pixels.extend_from_slice(&row[..16 * 4]);
    }
    pixels
}
