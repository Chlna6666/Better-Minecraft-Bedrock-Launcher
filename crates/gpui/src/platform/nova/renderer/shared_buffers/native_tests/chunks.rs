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

fn base_scene(renderer: &NovaRenderer) -> crate::Scene {
    let mut scene = crate::Scene::default();
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
        scene.record_retained_chunk(
            crate::GlobalElementId::from_path(&[name.into()]),
            1,
            start..scene.len(),
        );
        scene.pop_layer();
    }
    scene.finish();
    scene
}

fn replay_with_gap(renderer: &NovaRenderer, base: &crate::Scene, color: u32) -> crate::Scene {
    let mut scene = crate::Scene::default();
    scene.replay(0..base.len(), base);
    scene.insert_primitive(quad(renderer, false, color));
    scene.finish();
    scene
}

fn draw_all(renderer: &NovaRenderer, view: TextureViewId, slot: usize) {
    let steps = [DrawStepDescriptor {
        instance_count: (renderer.frame_upload.quads.len() / PACKED_QUAD_BYTES) as u32,
        resource_sets: resource_set_list([renderer.frame_resources[slot]
            .resource_sets
            .quad_resource_set]),
        ..quad_step(renderer)
    }];
    device!(renderer, device, {
        device
            .render_step_list_to_texture_compat(
                view,
                renderer.render_pass,
                RenderStepList::Draw(&steps),
                LoadOp::Clear(clear_color()),
                Some(RenderPassDepthAttachment {
                    target: renderer.depth_texture_view,
                    depth_load_op: LoadOp::Clear(1.0),
                }),
            )
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
    let steps = vec![RenderStepDescriptor::Draw(quad_step(renderer)); 512];
    let submission = device!(renderer, device, {
        device
            .render_steps_and_present_deferred(
                renderer.swapchain,
                renderer.render_pass,
                &steps,
                clear_color(),
                Some(RenderPassDepthAttachment {
                    target: renderer.depth_texture_view,
                    depth_load_op: LoadOp::Clear(1.0),
                }),
            )
            .expect("old chunk submission")
    });
    renderer.pending_submissions.push(PendingSubmission {
        submission,
        frame_resource_index: 0,
    });
}

fn verify_replay(renderer: &mut NovaRenderer, base: &crate::Scene) -> (usize, usize) {
    let first = upload(renderer, 0, base);
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
    let delta = upload(renderer, 0, &green);
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
        renderer.frame_upload.retained_quad_memory().used_bytes,
        (64 * PACKED_QUAD_BYTES) as u64
    );
    assert_eq!(
        upload(renderer, 1, &green),
        0,
        "second slot adopts identical segments"
    );
    (first, delta)
}

fn verify_cow(renderer: &mut NovaRenderer, base: &crate::Scene) -> usize {
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
    let fresh = upload(renderer, 1, &yellow);
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
    renderer
        .wait_for_pending_submissions()
        .expect("chunk fence");
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
        "P2-B {}: shared_payload=12288 B owned_gap=192 B owned_capacity={} B replay_copy=0 B quad_upload={first}->{delta} B cow_full={fresh} B; chunk backing, dirty upload, old/new pixels and COW full refill verified",
        renderer.backend_info.label(),
        renderer.frame_upload.quads.capacity()
    );
}
