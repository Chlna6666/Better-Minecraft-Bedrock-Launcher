//! Segmented sources exercise dirty complements and first-fill COW on native buffers.
use super::*;

fn quad(renderer: &NovaRenderer, right: bool, color: u32) -> Quad {
    let width = renderer.current_size.width as f32 / 2.0;
    let bounds = crate::bounds(
        crate::point(crate::px(if right { width } else { 0.0 }), crate::px(0.0)),
        crate::size(
            crate::px(width),
            crate::px(renderer.current_size.height as f32),
        ),
    )
    .scale(1.0);
    Quad {
        bounds,
        content_mask: crate::ContentMask {
            bounds,
            corner_bounds: bounds,
            ..Default::default()
        },
        background: crate::rgb(color).into(),
        ..Default::default()
    }
}

fn base_scene(renderer: &NovaRenderer) -> crate::window::retained::PaintFixture {
    let mut scene = crate::window::retained::PaintFixture::default();
    let viewport = crate::bounds(
        crate::point(crate::px(0.0), crate::px(0.0)),
        crate::size(
            crate::px(renderer.current_size.width as f32),
            crate::px(renderer.current_size.height as f32),
        ),
    )
    .scale(1.0);
    for (name, right, color) in [("left", false, 0xff0000), ("right", true, 0x0000ff)] {
        scene.push_layer(viewport);
        let start = scene.len();
        for _ in 0..32 {
            scene.insert_primitive(quad(renderer, right, color));
        }
        scene.record(
            crate::GlobalElementId::from_path(&[name.into()]),
            1,
            start..scene.len(),
        );
        scene.pop_layer();
    }
    scene.finish();
    scene
}

fn replay_with_gap(
    renderer: &NovaRenderer,
    base: &crate::window::retained::PaintFixture,
    color: u32,
) -> crate::window::retained::PaintFixture {
    let mut scene = crate::window::retained::PaintFixture::default();
    scene.replay(0..base.len(), base);
    scene.insert_primitive(quad(renderer, false, color));
    scene.finish();
    for index in [1, 35] {
        assert!(
            std::ptr::eq(
                &scene.paint_operations[index],
                &base.paint_operations[index]
            ),
            "retained operation payload must survive without a per-frame clone"
        );
    }
    scene
}

fn draw_all(renderer: &NovaRenderer, view: TextureViewId, slot: usize) {
    let steps = [RenderCommand::Draw(DrawCommand {
        instance_count: (renderer.frame_upload.quads.len() / PACKED_QUAD_BYTES) as u32,
        bind_groups: bind_group_list([renderer.frame_resources[slot]
            .bind_groups
            .quad_resource_set]),
        ..quad_step(renderer)
    })];
    device!(renderer, device, {
        device
            .render_to_textures(&[RenderPassCommands {
                texture_view: view,
                render_pass: renderer.render_pass,
                commands: &steps,
                color_load_op: LoadOp::Clear(clear_color()),
                clear_region: None,
                depth_attachment: Some(DepthAttachment {
                    target: renderer.depth_texture_view,
                    depth_load_op: LoadOp::Clear(1.0),
                }),
                viewport: None,
            }])
            .expect("segmented draw");
    });
}

fn assert_halves(renderer: &NovaRenderer, texture: TextureId, left: [u8; 4]) {
    device!(renderer, device, {
        let readback = device.read_texture(texture).expect("segmented readback");
        let y = renderer.current_size.height as usize / 2;
        for (x, expected) in [
            (renderer.current_size.width as usize / 4, left),
            (
                renderer.current_size.width as usize * 3 / 4,
                [255, 0, 0, 255],
            ),
        ] {
            let offset = y * readback.bytes_per_row as usize + x * 4;
            assert_eq!(&readback.bytes[offset..offset + 4], &expected);
        }
    });
}

fn submit_old_slot(renderer: &mut NovaRenderer) {
    let steps = vec![RenderCommand::Draw(quad_step(renderer)); 512];
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
            .expect("old chunk submission")
            .and_then(|frame: gfx_core::PresentedFrame| frame.submission)
            .expect("old chunk submission id")
    });
    renderer.pending_submissions.push(PendingSubmission {
        submission,
        frame_resource_index: 0,
        publication: None,
    });
}

fn verify_replay(
    renderer: &mut NovaRenderer,
    base: &crate::window::retained::PaintFixture,
) -> (usize, usize) {
    let first = upload(renderer, 0, base, &base.retention);
    assert_eq!(renderer.frame_upload.resident_quad_spans.len(), 2);
    assert_eq!(renderer.frame_upload.quads.owned_len(), 0);
    assert_eq!(first, 64 * PACKED_QUAD_BYTES);
    let payloads: Vec<_> = renderer
        .frame_upload
        .quads
        .shared_bytes()
        .map(Arc::clone)
        .collect();
    let green = replay_with_gap(renderer, base, 0x00ff00);
    for index in [0, 32] {
        assert_eq!(
            &green.quads[index] as *const Quad, &base.quads[index] as *const Quad,
            "Window replay must share CPU Paint as well as packed GPU bytes",
        );
    }
    let delta = upload(renderer, 0, &green, &green.retention);
    assert_eq!(delta, PACKED_QUAD_BYTES, "only the new gap uploads");
    assert_eq!(renderer.frame_upload.quads.owned_len(), PACKED_QUAD_BYTES);
    assert_eq!(
        renderer.buffer_memory_profile()[2].cpu.used_bytes,
        PACKED_QUAD_BYTES as u64
    );
    for (current, cached) in renderer.frame_upload.quads.shared_bytes().zip(&payloads) {
        assert!(
            Arc::ptr_eq(current, cached),
            "replay must borrow the cached backing"
        );
    }
    assert_eq!(
        renderer
            .frame_upload
            .retained_chunk_memory(&renderer.retained_upload.retained_chunks)
            .used_bytes,
        (64 * PACKED_QUAD_BYTES) as u64
    );
    assert_eq!(
        upload(renderer, 1, &green, &green.retention),
        0,
        "second slot adopts identical segments"
    );
    (first, delta)
}

fn verify_cow(renderer: &mut NovaRenderer, base: &crate::window::retained::PaintFixture) -> usize {
    renderer
        .activate_frame_resources(0)
        .expect("old chunk slot");
    let old = renderer.quad_buffer;
    let old_target = target(renderer);
    let new_target = target(renderer);
    draw_all(renderer, old_target.1, 0);
    assert_halves(renderer, old_target.0, [0, 255, 0, 255]);
    submit_old_slot(renderer);
    let yellow = replay_with_gap(renderer, base, 0xffff00);
    let fresh = upload(renderer, 1, &yellow, &yellow.retention);
    assert_ne!(renderer.quad_buffer, old);
    assert_eq!(
        fresh,
        65 * PACKED_QUAD_BYTES,
        "new COW buffer fills every segment and gap"
    );
    draw_all(renderer, new_target.1, 1);
    draw_all(renderer, old_target.1, 0);
    assert_halves(renderer, new_target.0, [0, 255, 255, 255]);
    assert_halves(renderer, old_target.0, [0, 255, 0, 255]);
    renderer.wait_pending_submissions().expect("chunk fence");
    renderer
        .coalesce_idle_static_buffers()
        .expect("merge chunk versions");
    assert_eq!(
        renderer.frame_resources[0].buffers.quad_buffer,
        renderer.quad_buffer
    );
    device!(renderer, device, {
        for (texture, view) in [old_target, new_target] {
            device
                .destroy_texture_view(view)
                .expect("chunk view cleanup");
            device
                .destroy_texture(texture)
                .expect("chunk texture cleanup");
        }
    });
    fresh
}

pub(super) fn verify(renderer: &mut NovaRenderer) {
    let base = base_scene(renderer);
    let (first, delta) = verify_replay(renderer, &base);
    let fresh = verify_cow(renderer, &base);
    println!(
        "P2-B {}: cpu_paint_shared_quads=64 cpu_paint_shared_operations=64 shared_payload=12288 B owned_gap=192 B owned_capacity={} B replay_copy=0 B quad_upload={first}->{delta} B cow_full={fresh} B; chunk backing, dirty upload, old/new pixels and COW full refill verified",
        renderer.backend_info.label(),
        renderer.frame_upload.quads.capacity()
    );
}
