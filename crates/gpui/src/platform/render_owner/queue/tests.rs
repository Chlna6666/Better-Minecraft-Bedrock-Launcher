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
