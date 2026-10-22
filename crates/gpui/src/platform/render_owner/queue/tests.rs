use super::*;
use crate::{
    BackdropBlurDamagePlan, DirtyRegion, PartialPresentMode, Point, ScaledPixels, bounds, size,
};
use std::sync::Arc;

fn packet(scene: Arc<crate::window::Paint>, x: f32) -> PresentationPacket {
    let mut dirty = DirtyRegion::empty();
    dirty.push(bounds(
        Point {
            x: ScaledPixels(x),
            y: ScaledPixels(0.0),
        },
        size(ScaledPixels(10.0), ScaledPixels(10.0)),
    ));
    PresentationPacket::new(
        scene,
        [],
        [],
        Instant::now(),
        dirty,
        BackdropBlurDamagePlan::default(),
        PartialPresentMode::FullRedraw,
    )
}

fn draw(packet: PresentationPacket) -> Command {
    Command::Draw {
        packet,
        framebuffer_only: false,
        reply: None,
        resize: None,
    }
}

fn resized_draw(size: Size<DevicePixels>, epoch: u64, packet: PresentationPacket) -> Command {
    Command::Draw {
        packet,
        framebuffer_only: false,
        reply: None,
        resize: Some(DrawableResize {
            size,
            epoch: Some(epoch),
        }),
    }
}

fn resize(size: Size<DevicePixels>) -> Command {
    Command::resize(size)
}

#[test]
fn pending_draw_subsumes_ready_tick_without_crossing_controls() {
    let queue = Queue::default();
    queue
        .enqueue(Command::Continue(Instant::now()), || Ok(()))
        .unwrap();
    queue
        .enqueue(
            draw(packet(Arc::new(crate::window::Paint::default()), 0.)),
            || Ok(()),
        )
        .unwrap();
    assert!(queue.has_pending_draw());
    assert!(matches!(queue.take(), Some(Command::Draw { .. })));
    assert!(!queue.has_pending_draw());
    queue
        .enqueue(Command::Call(Box::new(|_| {})), || Ok(()))
        .unwrap();
    queue
        .enqueue(
            draw(packet(Arc::new(crate::window::Paint::default()), 0.)),
            || Ok(()),
        )
        .unwrap();
    assert!(
        !queue.has_pending_draw(),
        "a tick must not look beyond resource controls"
    );
}

#[test]
fn pending_resize_draw_subsumes_old_tick() {
    let queue = Queue::default();
    let extent = size(DevicePixels(640), DevicePixels(480));
    queue.enqueue(resize(extent), || Ok(())).unwrap();
    queue
        .enqueue(
            draw(packet(Arc::new(crate::window::Paint::default()), 0.)),
            || Ok(()),
        )
        .unwrap();
    assert!(
        queue.has_pending_draw(),
        "the matching new-size draw subsumes an old tick"
    );
    assert!(matches!(queue.take(), Some(Command::Resize { .. })));
    assert!(matches!(queue.take(), Some(Command::Draw { .. })));
    assert!(!queue.has_pending_draw());
}

#[test]
fn replacement_preserves_backlog_age_and_uses_latest_enqueue_time() {
    let queue = Queue::default();
    let first = Instant::now();
    for offset in [0, 10, 20] {
        queue
            .enqueue_at(
                Command::Tick(first, None),
                || Ok(()),
                first + Duration::from_millis(offset),
            )
            .unwrap();
    }
    let queued = queue.take_timed().expect("coalesced tick");
    assert_eq!(queued.enqueued_at, first + Duration::from_millis(20));
    assert_eq!(queued.first_enqueued_at, first);
    assert_eq!(queued.coalesced_count, 2);
    assert!(queue.take_timed().is_none());
}

#[test]
fn scene_replacement_and_control_barrier_keep_separate_wait_origins() {
    let queue = Queue::default();
    let first = Instant::now();
    for (offset, command) in [
        (
            0,
            draw(packet(Arc::new(crate::window::Paint::default()), 0.0)),
        ),
        (
            5,
            draw(packet(Arc::new(crate::window::Paint::default()), 10.0)),
        ),
        (10, Command::Call(Box::new(|_| {}))),
        (
            15,
            draw(packet(Arc::new(crate::window::Paint::default()), 20.0)),
        ),
    ] {
        queue
            .enqueue_at(command, || Ok(()), first + Duration::from_millis(offset))
            .unwrap();
    }
    let replaced = queue.take_timed().expect("first draw");
    assert_eq!(replaced.first_enqueued_at, first);
    assert_eq!(replaced.coalesced_count, 1);
    let barrier = queue.take_timed().expect("control barrier");
    assert_eq!(barrier.coalesced_count, 0);
    let later = queue.take_timed().expect("later draw");
    assert_eq!(later.first_enqueued_at, first + Duration::from_millis(15));
    assert_eq!(later.coalesced_count, 0);
}

#[test]
fn another_windows_service_is_visible_in_queue_wait() {
    let busy = Queue::default();
    let waiting = Queue::default();
    let enqueued = Instant::now();
    busy.enqueue_at(Command::Tick(enqueued, None), || Ok(()), enqueued)
        .unwrap();
    waiting
        .enqueue_at(Command::Tick(enqueued, None), || Ok(()), enqueued)
        .unwrap();
    busy.take_timed().expect("busy window dispatch");
    // A deterministic owner clock: the first window consumes twenty milliseconds.
    let second_started = enqueued + Duration::from_millis(20);
    let queued = waiting.take_timed().expect("waiting window dispatch");
    assert_eq!(
        second_started.duration_since(queued.enqueued_at),
        Duration::from_millis(20)
    );
    assert_eq!(queued.coalesced_count, 0);
}

#[test]
fn latest_scene_keeps_unsubmitted_damage_and_one_wake() {
    let queue = Queue::default();
    let first = Arc::new(crate::window::Paint::default());
    let latest = Arc::new(crate::window::Paint::default());
    queue.enqueue(draw(packet(first, 0.0)), || Ok(())).unwrap();
    queue
        .enqueue(draw(packet(latest.clone(), 40.0)), || {
            panic!("duplicate wake")
        })
        .unwrap();
    let Some(Command::Draw { packet, .. }) = queue.take() else {
        panic!("expected scene");
    };
    assert!(Arc::ptr_eq(&packet.paint, &latest));
    assert_eq!(
        packet.dirty_region.union_bounds().unwrap().size.width,
        ScaledPixels(50.0)
    );
    assert!(!queue.finish_dispatch());
}

#[test]
fn queued_resize_scene_compaction_keeps_latest_extent_as_damage_authority() {
    let queue = Queue::default();
    let scene = |extent: (u32, u32), epoch| {
        Arc::new(crate::window::Paint::new(
            crate::Scene::default(),
            Arc::default(),
            crate::window::PaintGeometry {
                layout_extent: Some(extent),
                layout_epoch: epoch,
                scale_factor: 1.0,
            },
        ))
    };
    queue
        .enqueue(draw(packet(scene((400, 300), 3), 390.0)), || Ok(()))
        .unwrap();
    queue
        .enqueue(draw(packet(scene((800, 600), 4), 790.0)), || Ok(()))
        .unwrap();
    let Some(Command::Draw { packet, .. }) = queue.take() else {
        panic!("expected latest scene after queue compaction");
    };
    assert_eq!(packet.paint.geometry.layout_extent, Some((800, 600)));
    assert!(packet.dirty_region.is_full());
    assert_eq!(packet.dirty_region.area(), 800.0 * 600.0);
    assert!(packet.force_full_backdrop_blur_refresh);
    assert!(matches!(
        packet.partial_present_mode,
        PartialPresentMode::FullRedraw
    ));
    assert!(queue.take().is_none());
}

#[test]
fn presentation_replacement_does_not_cross_control_barrier() {
    let queue = Queue::default();
    let scene = Arc::new(crate::window::Paint::default());
    for command in [
        draw(packet(scene.clone(), 0.0)),
        Command::Call(Box::new(|_| {})),
        draw(packet(scene.clone(), 40.0)),
        draw(packet(scene, 80.0)),
    ] {
        queue.enqueue(command, || Ok(())).unwrap();
    }
    assert!(matches!(queue.take(), Some(Command::Draw { .. })));
    assert!(matches!(queue.take(), Some(Command::Call(_))));
    let Some(Command::Draw { packet, .. }) = queue.take() else {
        panic!("expected scene");
    };
    assert_eq!(
        packet.dirty_region.union_bounds().unwrap().origin.x,
        ScaledPixels(40.0)
    );
    assert_eq!(
        packet.dirty_region.union_bounds().unwrap().size.width,
        ScaledPixels(50.0)
    );
    assert!(!queue.finish_dispatch());
}

#[test]
fn ticks_coalesce_but_first_frame_is_a_barrier() {
    let queue = Queue::default();
    let now = Instant::now();
    let later = now + std::time::Duration::from_millis(10);
    let (reply, _receiver) = std::sync::mpsc::sync_channel(1);
    for command in [
        Command::Tick(now, None),
        Command::Draw {
            packet: packet(Arc::new(crate::window::Paint::default()), 0.0),
            framebuffer_only: false,
            reply: Some(reply),
            resize: None,
        },
        Command::Tick(now, None),
        Command::Tick(later, None),
    ] {
        queue.enqueue(command, || Ok(())).unwrap();
    }
    assert!(matches!(queue.take(), Some(Command::Tick(time, _)) if time == now));
    assert!(matches!(
        queue.take(),
        Some(Command::Draw { reply: Some(_), .. })
    ));
    assert!(matches!(queue.take(), Some(Command::Tick(time, _)) if time == later));
    assert!(!queue.finish_dispatch());
}

#[test]
fn newest_scene_absorbs_obsolete_compositor_ticks_and_retains_damage() {
    let queue = Queue::default();
    let started = Instant::now();
    let scene = Arc::new(crate::window::Paint::default());
    for (millis, command) in [
        (0, Command::Tick(started, None)),
        (3, draw(packet(scene.clone(), 0.0))),
        (6, Command::Tick(started + Duration::from_millis(6), None)),
        (8, Command::Continue(started + Duration::from_millis(8))),
        (12, draw(packet(scene, 30.0))),
    ] {
        queue
            .enqueue_at(command, || Ok(()), started + Duration::from_millis(millis))
            .expect("gpu owner remains open");
    }
    let queued = queue.take_timed().expect("latest frame");
    assert_eq!(queued.first_enqueued_at, started);
    assert_eq!(queued.enqueued_at, started + Duration::from_millis(12));
    assert_eq!(queued.coalesced_count, 4);
    let Command::Draw { packet, .. } = queued.command else {
        panic!("newest committed scene must replace old tick/render requests");
    };
    assert_eq!(
        packet
            .dirty_region
            .union_bounds()
            .expect("merged damage")
            .size
            .width,
        ScaledPixels(40.0)
    );
    assert!(queue.take().is_none(), "the owner should draw only once");
}

#[test]
fn compositor_tick_coalescing_must_not_cross_control_barrier() {
    let queue = Queue::default();
    let scene = Arc::new(crate::window::Paint::default());
    queue
        .enqueue(Command::Tick(Instant::now(), None), || Ok(()))
        .unwrap();
    queue
        .enqueue(Command::Call(Box::new(|_| {})), || Ok(()))
        .unwrap();
    queue.enqueue(draw(packet(scene, 12.0)), || Ok(())).unwrap();
    assert!(matches!(queue.take(), Some(Command::Tick(..))));
    assert!(matches!(queue.take(), Some(Command::Call(_))));
    assert!(matches!(queue.take(), Some(Command::Draw { .. })));
    assert!(queue.take().is_none());
}

#[test]
fn shutdown_discards_pending_work_and_rejects_later_commits() {
    let queue = Queue::default();
    queue
        .enqueue(Command::Tick(Instant::now(), None), || Ok(()))
        .unwrap();
    let (reply, _receiver) = std::sync::mpsc::sync_channel(1);
    queue.enqueue(Command::Shutdown(reply), || Ok(())).unwrap();
    assert!(
        queue
            .enqueue(Command::Tick(Instant::now(), None), || Ok(()))
            .is_err()
    );
    assert!(matches!(queue.take(), Some(Command::Shutdown(_))));
    assert!(!queue.finish_dispatch());
}

#[test]
fn shutting_down_dx12_child_queue_does_not_invalidate_main_window_queue() {
    // Backend-independent ownership contract needed for DX12 child-window
    // teardown: each Entry owns its own Queue; Shutdown only closes that
    // window's queue and cannot clear a peer window's resize transaction.
    let child = Queue::default();
    let main = Queue::default();
    let scene = Arc::new(crate::window::Paint::default());
    main.enqueue(
        resized_draw(
            size(DevicePixels(900), DevicePixels(600)),
            31,
            packet(scene, 0.0),
        ),
        || Ok(()),
    )
    .unwrap();
    child
        .enqueue(Command::Tick(Instant::now(), None), || Ok(()))
        .unwrap();
    let (reply, receiver) = std::sync::mpsc::sync_channel(1);
    child.enqueue(Command::Shutdown(reply), || Ok(())).unwrap();
    assert!(matches!(child.take(), Some(Command::Shutdown(_))));
    assert!(
        receiver.recv().is_err(),
        "dropped child receiver must not block"
    );
    assert!(child.is_closed());
    assert!(
        !main.is_closed(),
        "child closure must never close the main queue"
    );
    assert!(
        matches!(main.take(), Some(Command::Draw { resize: Some(resize), .. }) if resize.epoch == Some(31))
    );
    main.enqueue(Command::Tick(Instant::now(), None), || Ok(()))
        .unwrap();
    assert!(matches!(main.take(), Some(Command::Tick(..))));
}

#[test]
fn draining_rearms_only_after_queue_becomes_empty() {
    let queue = Queue::default();
    queue
        .enqueue(Command::Tick(Instant::now(), None), || Ok(()))
        .unwrap();
    assert!(matches!(queue.take(), Some(Command::Tick(..))));
    queue
        .enqueue(Command::Call(Box::new(|_| {})), || {
            panic!("already scheduled")
        })
        .unwrap();
    assert!(queue.finish_dispatch());
    assert!(matches!(queue.take(), Some(Command::Call(_))));
    assert!(!queue.finish_dispatch());
    let mut woke = false;
    queue
        .enqueue(Command::Tick(Instant::now(), None), || {
            woke = true;
            Ok(())
        })
        .unwrap();
    assert!(woke);
}

#[test]
fn readiness_continuation_cannot_replace_a_tick_across_a_pause_barrier() {
    let queue = Queue::default();
    let now = Instant::now();
    queue.enqueue(Command::Continue(now), || Ok(())).unwrap();
    queue
        .enqueue(Command::PresentationInterval(None), || {
            panic!("duplicate wake")
        })
        .unwrap();
    queue
        .enqueue(Command::Tick(now, None), || panic!("duplicate wake"))
        .unwrap();
    queue
        .enqueue(Command::Continue(now), || panic!("duplicate wake"))
        .unwrap();
    assert!(matches!(queue.take(), Some(Command::Continue(..))));
    assert!(matches!(
        queue.take(),
        Some(Command::PresentationInterval(None))
    ));
    assert!(matches!(queue.take(), Some(Command::Continue(..))));
    assert!(!queue.finish_dispatch());
}

#[test]
fn closing_releases_a_waiting_first_frame_handshake() {
    let queue = Queue::default();
    let (reply, receiver) = std::sync::mpsc::sync_channel(1);
    queue
        .enqueue(
            Command::Draw {
                packet: packet(Arc::new(crate::window::Paint::default()), 0.0),
                framebuffer_only: false,
                reply: Some(reply),
                resize: None,
            },
            || Ok(()),
        )
        .unwrap();
    queue.close();
    assert!(receiver.recv().is_err());
    assert!(!queue.has_presentation());
    assert!(
        queue
            .enqueue(Command::Tick(Instant::now(), None), || Ok(()))
            .is_err()
    );
}

#[test]
fn replaced_scene_keeps_earlier_backdrop_source_damage() {
    let viewport = bounds(
        Point::default(),
        size(ScaledPixels(200.0), ScaledPixels(100.0)),
    );
    let scene = |x| {
        let mut scene = crate::scene::Painter::default();
        let source = bounds(
            Point {
                x: ScaledPixels(x),
                y: ScaledPixels(20.0),
            },
            size(ScaledPixels(10.0), ScaledPixels(10.0)),
        );
        scene.insert_primitive(crate::Quad {
            bounds: source,
            content_mask: crate::ContentMask::new(viewport),
            background: crate::rgb(0xff0000).into(),
            ..Default::default()
        });
        scene.insert_primitive(crate::PaintBackdropBlur {
            order: 1,
            animation_id: None,
            bounds: viewport,
            content_mask: crate::ContentMask::new(viewport),
            corner_radii: Default::default(),
            radius: ScaledPixels(8.0),
            downsample: 2,
            levels: 3,
            recompute_overlap: false,
            saturation: 1.0,
            opacity: 1.0,
            tint: None,
        });
        scene.finish();
        scene
    };
    let previous = scene(10.0);
    let intermediate = scene(40.0);
    let latest = scene(80.0);
    let blur_order = latest.backdrop_blurs[0].order;
    let intermediate_plan =
        crate::paint::backdrop_blur_damage_plan(&intermediate, &previous, &[], &[]);
    let latest_plan = crate::paint::backdrop_blur_damage_plan(&latest, &intermediate, &[], &[]);
    assert!(intermediate_plan.refresh_required());
    assert!(latest_plan.refresh_required());
    let mut intermediate = packet(
        Arc::new(crate::window::Paint::new(
            intermediate.into_scene(),
            Arc::default(),
            Default::default(),
        )),
        40.0,
    );
    intermediate.backdrop_blur_damage_plan = intermediate_plan;
    let mut latest = packet(
        Arc::new(crate::window::Paint::new(
            latest.into_scene(),
            Arc::default(),
            Default::default(),
        )),
        80.0,
    );
    latest.backdrop_blur_damage_plan = latest_plan;
    let queue = Queue::default();
    queue.enqueue(draw(intermediate), || Ok(())).unwrap();
    queue
        .enqueue(draw(latest), || panic!("duplicate wake"))
        .unwrap();
    let Some(Command::Draw { packet, .. }) = queue.take() else {
        panic!("expected scene");
    };
    let (full_refresh, damage) = packet
        .backdrop_blur_damage_plan
        .order_source_damage(blur_order, blur_order);
    let damage: Vec<_> = damage.collect();
    assert!(!full_refresh);
    assert!(
        damage
            .iter()
            .any(|damage| damage.origin.x == ScaledPixels(10.0))
    );
    assert!(
        damage
            .iter()
            .any(|damage| damage.origin.x == ScaledPixels(80.0))
    );
}

#[test]
fn adjacent_resize_commands_coalesce_without_crossing_draw_barriers() {
    use crate::{DevicePixels, size};
    let queue = Queue::default();
    let started = Instant::now();
    let extent = |side| size(DevicePixels(side), DevicePixels(side));

    queue
        .enqueue_at(resize(extent(900)), || Ok(()), started)
        .unwrap();
    queue
        .enqueue_at(
            resize(extent(1000)),
            || panic!("adjacent resize should reuse the pending wake"),
            started + Duration::from_millis(1),
        )
        .unwrap();
    let first = queue.take_timed().expect("coalesced resize");
    assert_eq!(first.coalesced_count, 1);
    assert_eq!(first.first_enqueued_at, started);
    let Command::Resize { size, epoch } = first.command else {
        panic!("expected resize")
    };
    assert_eq!(epoch, None);
    assert_eq!(size, extent(1000));
    assert!(queue.take_timed().is_none());

    queue.enqueue(resize(extent(1200)), || Ok(())).unwrap();
    queue
        .enqueue(
            draw(packet(Arc::new(crate::window::Paint::default()), 0.0)),
            || Ok(()),
        )
        .unwrap();
    queue.enqueue(resize(extent(1300)), || Ok(())).unwrap();
    let Some(Command::Resize {
        size: before_draw, ..
    }) = queue.take()
    else {
        panic!("first resize lost")
    };
    assert_eq!(before_draw, extent(1200));
    assert!(matches!(queue.take(), Some(Command::Draw { .. })));
    let Some(Command::Resize {
        size: after_draw, ..
    }) = queue.take()
    else {
        panic!("last resize lost")
    };
    assert_eq!(after_draw, extent(1300));
}

#[test]
fn live_resize_queue_keeps_only_latest_unexecuted_extent_and_scene() {
    use crate::{DevicePixels, size};

    let queue = Queue::default();
    let extent = |side| size(DevicePixels(side), DevicePixels(side));
    let scene = Arc::new(crate::window::Paint::default());
    let start = Instant::now();
    for (offset, command) in [
        (0, resize(extent(800))),
        (1, draw(packet(scene.clone(), 0.0))),
        (2, resize(extent(900))),
        (3, draw(packet(scene.clone(), 40.0))),
        (4, resize(extent(1000))),
        (5, draw(packet(scene, 80.0))),
    ] {
        queue
            .enqueue_at(command, || Ok(()), start + Duration::from_millis(offset))
            .unwrap();
    }

    let Some(Command::Resize {
        size: resized,
        epoch,
    }) = queue.take()
    else {
        panic!("latest resize must be retained");
    };
    assert_eq!(resized, extent(1000));
    assert_eq!(epoch, None);
    let Some(QueuedCommand {
        command: Command::Draw { packet, .. },
        first_enqueued_at,
        ..
    }) = queue.take_timed()
    else {
        panic!("only latest full-damage scene must remain");
    };
    assert_eq!(first_enqueued_at, start);
    assert_eq!(
        packet.dirty_region.union_bounds().unwrap().size.width,
        ScaledPixels(90.0)
    );
    assert!(queue.take().is_none());
}

#[test]
fn live_resize_transaction_keeps_latest_extent_paired_with_latest_scene() {
    use crate::{DevicePixels, size};

    let queue = Queue::default();
    let extent = |side| size(DevicePixels(side), DevicePixels(side));
    let scene = Arc::new(crate::window::Paint::default());
    let start = Instant::now();
    for (offset, command) in [
        (0, resized_draw(extent(900), 7, packet(scene.clone(), 0.0))),
        (
            1,
            resized_draw(extent(1000), 8, packet(scene.clone(), 20.0)),
        ),
        (2, resized_draw(extent(1100), 9, packet(scene, 40.0))),
    ] {
        queue
            .enqueue_at(command, || Ok(()), start + Duration::from_millis(offset))
            .unwrap();
    }
    assert!(queue.has_pending_resize_draw());

    let Some(QueuedCommand {
        command:
            Command::Draw {
                packet,
                resize: Some(resize),
                ..
            },
        first_enqueued_at,
        ..
    }) = queue.take_timed()
    else {
        panic!("latest resize and its Scene must remain one queued transaction");
    };
    assert_eq!(resize.size, extent(1100));
    assert_eq!(resize.epoch, Some(9));
    assert_eq!(first_enqueued_at, start);
    assert_eq!(
        packet.dirty_region.union_bounds().unwrap().size.width,
        ScaledPixels(50.0)
    );
    assert!(queue.take().is_none());
    assert!(!queue.has_pending_resize_draw());
}

#[test]
fn live_resize_compaction_stops_at_control_and_first_frame_barriers() {
    use crate::{DevicePixels, size};
    let queue = Queue::default();
    let extent = |side| size(DevicePixels(side), DevicePixels(side));
    let scene = Arc::new(crate::window::Paint::default());

    queue
        .enqueue(
            resized_draw(extent(800), 1, packet(scene.clone(), 0.0)),
            || Ok(()),
        )
        .unwrap();
    queue
        .enqueue(Command::Call(Box::new(|_| {})), || Ok(()))
        .unwrap();
    queue
        .enqueue(
            resized_draw(extent(900), 2, packet(scene.clone(), 20.0)),
            || Ok(()),
        )
        .unwrap();
    queue
        .enqueue(
            resized_draw(extent(1000), 3, packet(scene, 40.0)),
            || Ok(()),
        )
        .unwrap();

    assert!(matches!(
        queue.take(),
        Some(Command::Draw {
            resize: Some(DrawableResize { size, epoch: Some(1) }),
            ..
        }) if size == extent(800)
    ));
    assert!(matches!(queue.take(), Some(Command::Call(_))));
    let Some(Command::Draw {
        packet,
        resize: Some(resize),
        ..
    }) = queue.take()
    else {
        panic!("latest resize transaction must remain behind the control barrier");
    };
    assert_eq!(resize.size, extent(1000));
    assert_eq!(resize.epoch, Some(3));
    assert_eq!(
        packet.dirty_region.union_bounds().unwrap().size.width,
        ScaledPixels(30.0)
    );
    assert!(queue.take().is_none());
}

#[test]
fn live_resize_compaction_preserves_synchronous_first_frame_handshake() {
    use crate::{DevicePixels, size};
    let queue = Queue::default();
    let extent = |side| size(DevicePixels(side), DevicePixels(side));
    let scene = Arc::new(crate::window::Paint::default());
    let (reply, receiver) = std::sync::mpsc::sync_channel(1);
    queue
        .enqueue(
            Command::Draw {
                packet: packet(scene.clone(), 0.0),
                framebuffer_only: false,
                reply: Some(reply),
                resize: None,
            },
            || Ok(()),
        )
        .unwrap();
    queue
        .enqueue(
            resized_draw(extent(900), 2, packet(scene.clone(), 20.0)),
            || Ok(()),
        )
        .unwrap();
    queue
        .enqueue(
            resized_draw(extent(1000), 3, packet(scene, 40.0)),
            || Ok(()),
        )
        .unwrap();
    let Some(Command::Draw {
        reply: Some(sender),
        ..
    }) = queue.take()
    else {
        panic!("synchronous first-frame handshake cannot be removed");
    };
    sender.send(Ok(true)).unwrap();
    assert!(receiver.recv().unwrap().unwrap());
    let Some(Command::Draw {
        packet,
        resize: Some(resize),
        ..
    }) = queue.take()
    else {
        panic!("latest resize transaction after handshake is required");
    };
    assert_eq!(resize.size, extent(1000));
    assert_eq!(resize.epoch, Some(3));
    assert_eq!(
        packet.dirty_region.union_bounds().unwrap().size.width,
        ScaledPixels(30.0)
    );
    assert!(queue.take().is_none());
}

#[test]
fn repeated_live_resize_commands_compact_without_compositor_stretch() {
    let queue = Queue::default();
    let extent = |side| size(DevicePixels(side), DevicePixels(side));
    let scene = Arc::new(crate::window::Paint::default());
    for (side, epoch) in [(800, 1), (900, 2), (1100, 3)] {
        queue
            .enqueue(
                resized_draw(
                    extent(side),
                    epoch,
                    packet(scene.clone(), (epoch * 10) as f32),
                ),
                || Ok(()),
            )
            .unwrap();
    }
    let Some(Command::Draw {
        packet,
        resize: Some(resize),
        ..
    }) = queue.take()
    else {
        panic!("must retain latest matching Resize and Scene");
    };
    assert_eq!(resize.size, extent(1100));
    assert_eq!(resize.epoch, Some(3));
    assert_eq!(
        packet.dirty_region.union_bounds().unwrap().size.width,
        ScaledPixels(30.0)
    );
    assert!(queue.take().is_none());
}

#[test]
fn resize_compaction_keeps_renderer_control_barrier_and_latest_scene() {
    let queue = Queue::default();
    let extent = |side| size(DevicePixels(side), DevicePixels(side));
    let scene = Arc::new(crate::window::Paint::default());
    queue
        .enqueue(
            resized_draw(extent(800), 1, packet(scene.clone(), 0.0)),
            || Ok(()),
        )
        .unwrap();
    queue
        .enqueue(Command::Call(Box::new(|_| {})), || Ok(()))
        .unwrap();
    queue
        .enqueue(
            resized_draw(extent(900), 2, packet(scene.clone(), 10.0)),
            || Ok(()),
        )
        .unwrap();
    queue
        .enqueue(
            resized_draw(extent(1000), 3, packet(scene, 20.0)),
            || Ok(()),
        )
        .unwrap();
    assert!(
        matches!(queue.take(), Some(Command::Draw { resize: Some(resize), .. }) if resize.size == extent(800))
    );
    assert!(matches!(queue.take(), Some(Command::Call(_))));
    assert!(
        matches!(queue.take(), Some(Command::Draw { resize: Some(resize), .. }) if resize.size == extent(1000) && resize.epoch == Some(3))
    );
    assert!(queue.take().is_none());
}

#[test]
fn repeated_native_covers_keep_only_the_newest_extent() {
    use crate::{DevicePixels, size};

    let queue = Queue::default();
    queue
        .enqueue(
            Command::Cover {
                size: size(DevicePixels(1200), DevicePixels(800)),
            },
            || Ok(()),
        )
        .unwrap();
    queue
        .enqueue(Command::Continue(Instant::now()), || Ok(()))
        .unwrap();
    queue
        .enqueue(
            Command::Cover {
                size: size(DevicePixels(1240), DevicePixels(820)),
            },
            || Ok(()),
        )
        .unwrap();
    assert!(matches!(queue.take(), Some(Command::Continue(..))));
    let Some(Command::Cover { size: covered }) = queue.take() else {
        panic!("the newest native cover must survive coalescing");
    };
    assert_eq!(covered, size(DevicePixels(1240), DevicePixels(820)));
    assert!(queue.take().is_none());
}

#[test]
fn live_resize_compaction_preserves_latest_native_cover() {
    use crate::{DevicePixels, size};

    let queue = Queue::default();
    let extent = |side| size(DevicePixels(side), DevicePixels(side));
    let scene = Arc::new(crate::window::Paint::default());
    queue
        .enqueue(Command::Cover { size: extent(900) }, || Ok(()))
        .unwrap();
    // Keep the last-frame cover until its replacement is presented. It must not
    // become an ordering barrier or discard the matching resize transaction.
    queue
        .enqueue(resized_draw(extent(1000), 2, packet(scene, 0.0)), || Ok(()))
        .unwrap();
    assert!(matches!(
        queue.take(),
        Some(Command::Cover { size }) if size == extent(900)
    ));
    assert!(matches!(
        queue.take(),
        Some(Command::Draw { resize: Some(resize), .. }) if resize.size == extent(1000)
    ));
    assert!(queue.take().is_none());
}
