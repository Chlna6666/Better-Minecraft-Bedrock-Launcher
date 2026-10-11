use super::*;
use crate::{
    BackdropBlurDamagePlan, DirtyRegion, PartialPresentMode, Point, ScaledPixels, Scene, bounds,
    size,
};
use std::sync::Arc;

fn packet(scene: Arc<Scene>, x: f32) -> PresentationPacket {
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
        1.0,
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
    }
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
        (0, draw(packet(Arc::new(Scene::default()), 0.0))),
        (5, draw(packet(Arc::new(Scene::default()), 10.0))),
        (10, Command::Call(Box::new(|_| {}))),
        (15, draw(packet(Arc::new(Scene::default()), 20.0))),
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
    let first = Arc::new(Scene::default());
    let latest = Arc::new(Scene::default());
    queue.enqueue(draw(packet(first, 0.0)), || Ok(())).unwrap();
    queue
        .enqueue(draw(packet(latest.clone(), 40.0)), || {
            panic!("duplicate wake")
        })
        .unwrap();
    let Some(Command::Draw { packet, .. }) = queue.take() else {
        panic!("expected scene");
    };
    assert!(Arc::ptr_eq(&packet.scene, &latest));
    assert_eq!(
        packet.dirty_region.union_bounds().unwrap().size.width,
        ScaledPixels(50.0)
    );
    assert!(!queue.finish_dispatch());
}

#[test]
fn presentation_replacement_does_not_cross_control_barrier() {
    let queue = Queue::default();
    let scene = Arc::new(Scene::default());
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
            packet: packet(Arc::new(Scene::default()), 0.0),
            framebuffer_only: false,
            reply: Some(reply),
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
    let scene = Arc::new(Scene::default());
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
        packet.dirty_region.union_bounds().expect("merged damage").size.width,
        ScaledPixels(40.0)
    );
    assert!(queue.take().is_none(), "the owner should draw only once");
}

#[test]
fn compositor_tick_coalescing_must_not_cross_control_barrier() {
    let queue = Queue::default();
    let scene = Arc::new(Scene::default());
    queue.enqueue(Command::Tick(Instant::now(), None), || Ok(())).unwrap();
    queue.enqueue(Command::Call(Box::new(|_| {})), || Ok(())).unwrap();
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
                packet: packet(Arc::new(Scene::default()), 0.0),
                framebuffer_only: false,
                reply: Some(reply),
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
        let mut scene = Scene::default();
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
    let intermediate_plan = intermediate.backdrop_blur_damage_plan(&previous, &[], &[]);
    let latest_plan = latest.backdrop_blur_damage_plan(&intermediate, &[], &[]);
    assert!(intermediate_plan.refresh_required());
    assert!(latest_plan.refresh_required());
    let mut intermediate = packet(Arc::new(intermediate), 40.0);
    intermediate.backdrop_blur_damage_plan = intermediate_plan;
    let mut latest = packet(Arc::new(latest), 80.0);
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
        .source_damage_for_orders(blur_order, blur_order);
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
