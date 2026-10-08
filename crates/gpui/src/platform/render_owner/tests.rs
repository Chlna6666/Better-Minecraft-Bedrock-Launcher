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
