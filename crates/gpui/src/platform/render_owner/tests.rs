use super::*;

fn owner(pending: bool) -> (RenderOwner, mpsc::Receiver<Job>) {
    let (sender, receiver) = mpsc::channel();
    let owner = RenderOwner {
        id: 1,
        queue: Arc::new(Queue::default()),
        sender,
        owner_thread: thread::current().id(),
        status: Arc::new(Status {

            pending: AtomicBool::new(pending),
            submitted: AtomicBool::new(true),
            waiting_for_frame_ready: AtomicBool::new(false),
        }),
        gpu_specs: GpuSpecs::default(),
    };
    (owner, receiver)
}

#[test]
fn idle_owner_does_not_enqueue_presentation() {
    let (owner, receiver) = owner(false);
    let result = owner.present_active_frame(Instant::now(), None);
    let queued = owner.queue.take();
    // No worker consumes this fixture: close before assertions so Drop cannot wait for a reply.
    owner.queue.close();

    assert!(result.unwrap().is_none());
    assert!(queued.is_none());
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn queued_ticks_wait_for_gpu_report_to_continue() {
    let (owner, receiver) = owner(true);
    let first_time = Instant::now();
    let latest_time = first_time + Duration::from_millis(16);
    let first = owner.present_active_frame(first_time, None);
    let latest = owner.present_active_frame(latest_time, None);
    let queued = owner.queue.take();
    let another = owner.queue.take();
    owner.queue.close();

    for result in [first, latest] {
        let frame = result.unwrap().expect("active work must enqueue a tick");
        assert!(
            !frame.continues,
            "only the GPU report may schedule continuation"
        );
        assert!(frame.completed_animations.is_empty());
    }
    assert!(matches!(queued, Some(Command::Tick(time, _)) if time == latest_time));
    assert!(another.is_none());
    assert!(receiver.try_recv().is_ok());
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn first_windows_frame_enqueues_without_waiting_for_gpu_completion() {
    use crate::{BackdropBlurDamagePlan, DirtyRegion, PartialPresentMode};

    // The GPU worker deliberately does not consume this queue. The producer must still
    // return immediately, even though this window has never presented a frame.
    let (owner, receiver) = owner(false);
    owner.status.submitted.store(false, Ordering::Release);
    let packet = PresentationPacket::new(
        std::sync::Arc::new(crate::window::Paint::new(
            crate::Scene::default(),
            std::sync::Arc::default(),
            crate::window::PaintGeometry {
                scale_factor: 1.0,
                ..Default::default()
            },
        )),
        [],
        [],
        Instant::now(),
        DirtyRegion::empty(),
        BackdropBlurDamagePlan::default(),
        PartialPresentMode::FullRedraw,
    );
    assert!(!owner.draw(packet).expect("first packet is queued"));
    assert!(owner.has_pending_gpu_work());
    assert!(!owner.has_submitted_frame());
    assert!(matches!(
        owner.queue.take(),
        Some(Command::Draw { reply: None, .. })
    ));
    // Close before Drop because no Worker exists in this fixture.
    owner.queue.close();
    assert!(receiver.try_recv().is_ok());
}

#[test]
fn competing_windows_have_independent_coalesced_queues() {
    let (busy, busy_wake) = owner(false);
    let (interactive, interactive_wake) = owner(false);
    busy.status.pending.store(true, Ordering::Release);
    interactive.status.pending.store(true, Ordering::Release);
    let first = Instant::now();

    for count in 0..24 {
        busy.present_active_frame(first + Duration::from_millis(count), None)
            .unwrap();
    }
    interactive.present_active_frame(first, None).unwrap();

    // A frequently ticking window cannot fill the shared job channel with 24
    // obsolete presentations or suppress another window's wake.
    assert!(matches!(busy.queue.take(), Some(Command::Tick(_, _))));
    assert!(busy.queue.take().is_none());
    assert!(matches!(
        interactive.queue.take(),
        Some(Command::Tick(_, _))
    ));
    assert!(busy_wake.try_recv().is_ok());
    assert!(interactive_wake.try_recv().is_ok());
    busy.queue.close();
    interactive.queue.close();
}

#[test]
fn blocked_swapchain_suppresses_only_its_own_speculative_ticks() {
    let (blocked, blocked_jobs) = owner(true);
    let (responsive, responsive_jobs) = owner(true);
    blocked
        .status
        .waiting_for_frame_ready
        .store(true, Ordering::Release);

    assert!(
        blocked.present_active_frame(Instant::now(), None).unwrap().is_none(),
        "blocked window must await its GPU-ready callback rather than polling DWM"
    );
    assert!(blocked.queue.take().is_none());
    assert!(matches!(
        blocked_jobs.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    assert!(
        responsive.present_active_frame(Instant::now(), None).unwrap().is_some(),
        "an independent window must remain schedulable"
    );
    assert!(matches!(responsive.queue.take(), Some(Command::Tick(_, _))));
    assert!(responsive_jobs.try_recv().is_ok());

    blocked
        .status
        .waiting_for_frame_ready
        .store(false, Ordering::Release);
    assert!(blocked.present_active_frame(Instant::now(), None).unwrap().is_some());
    assert!(matches!(blocked.queue.take(), Some(Command::Tick(_, _))));
    blocked.queue.close();
    responsive.queue.close();
}
