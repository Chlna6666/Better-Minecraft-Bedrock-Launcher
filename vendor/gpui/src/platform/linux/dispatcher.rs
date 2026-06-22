use crate::{PlatformDispatcher, Priority, PriorityQueueSender, TaskLabel};
use async_task::Runnable;
use calloop::{
    EventLoop,
    channel::{self, Sender},
    timer::TimeoutAction,
};
use parking_lot::{Condvar, Mutex};
use std::{
    collections::VecDeque,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use util::ResultExt;

const MIN_BACKGROUND_THREADS: usize = 2;

struct TimerAfter {
    duration: Duration,
    runnable: Runnable,
}

struct PrioritizedRunnable {
    priority: Priority,
    runnable: Runnable,
}

#[derive(Default)]
struct BackgroundQueueState {
    high: VecDeque<Runnable>,
    medium: VecDeque<Runnable>,
    low: VecDeque<Runnable>,
    closed: bool,
}

#[derive(Clone, Default)]
struct BackgroundQueue {
    state: Arc<Mutex<BackgroundQueueState>>,
    available: Arc<Condvar>,
}

impl BackgroundQueue {
    fn push(&self, runnable: Runnable, priority: Priority) {
        {
            let mut state = self.state.lock();
            match priority {
                Priority::RealtimeAudio => {
                    unreachable!("RealtimeAudio priority should use spawn_realtime, not dispatch")
                }
                Priority::High => state.high.push_back(runnable),
                Priority::Medium => state.medium.push_back(runnable),
                Priority::Low => state.low.push_back(runnable),
            }
        }
        self.available.notify_one();
    }

    fn pop(&self) -> Option<PrioritizedRunnable> {
        let mut state = self.state.lock();
        loop {
            if let Some(runnable) = state.high.pop_front() {
                return Some(PrioritizedRunnable {
                    priority: Priority::High,
                    runnable,
                });
            }
            if let Some(runnable) = state.medium.pop_front() {
                return Some(PrioritizedRunnable {
                    priority: Priority::Medium,
                    runnable,
                });
            }
            if let Some(runnable) = state.low.pop_front() {
                return Some(PrioritizedRunnable {
                    priority: Priority::Low,
                    runnable,
                });
            }
            if state.closed {
                return None;
            }
            self.available.wait(&mut state);
        }
    }

    fn close(&self) {
        self.state.lock().closed = true;
        self.available.notify_all();
    }
}

pub(crate) struct LinuxDispatcher {
    main_sender: Sender<()>,
    main_queue_sender: PriorityQueueSender<Runnable>,
    timer_sender: Sender<TimerAfter>,
    background_queue: BackgroundQueue,
    _background_threads: Vec<thread::JoinHandle<()>>,
    main_thread_id: thread::ThreadId,
}

impl LinuxDispatcher {
    pub fn new(main_sender: Sender<()>, main_queue_sender: PriorityQueueSender<Runnable>) -> Self {
        let background_queue = BackgroundQueue::default();
        let thread_count = std::thread::available_parallelism()
            .map(|i| i.get().max(MIN_BACKGROUND_THREADS))
            .unwrap_or(MIN_BACKGROUND_THREADS);

        let mut background_threads = (0..thread_count)
            .map(|i| {
                let background_queue = background_queue.clone();
                match std::thread::Builder::new()
                    .name(format!("Worker-{i}"))
                    .spawn(move || {
                        while let Some(PrioritizedRunnable { priority, runnable }) =
                            background_queue.pop()
                        {
                            let start = Instant::now();

                            runnable.run();

                            log::trace!(
                                "background thread {}: ran {priority:?} runnable. took: {:?}",
                                i,
                                start.elapsed()
                            );
                        }
                    }) {
                    Ok(thread) => thread,
                    Err(error) => panic!("failed to spawn GPUI background worker {i}: {error}"),
                }
            })
            .collect::<Vec<_>>();

        let (timer_sender, timer_channel) = calloop::channel::channel::<TimerAfter>();
        let timer_thread = match std::thread::Builder::new()
            .name("Timer".to_owned())
            .spawn(|| {
                let Ok(mut event_loop) = EventLoop::<()>::try_new() else {
                    log::error!("failed to initialize GPUI timer loop");
                    return;
                };

                let handle = event_loop.handle();
                let timer_handle = event_loop.handle();
                if let Err(error) = handle.insert_source(timer_channel, move |e, _, _| {
                    if let channel::Event::Msg(timer) = e {
                        // This has to be in an option to satisfy the borrow checker. The callback below should only be scheduled once.
                        let mut runnable = Some(timer.runnable);
                        if let Err(error) = timer_handle.insert_source(
                            calloop::timer::Timer::from_duration(timer.duration),
                            move |_, _, _| {
                                if let Some(runnable) = runnable.take() {
                                    runnable.run();
                                }
                                TimeoutAction::Drop
                            },
                        ) {
                            log::error!("failed to start GPUI timer: {error}");
                        }
                    }
                }) {
                    log::error!("failed to start GPUI timer thread: {error}");
                    return;
                }

                event_loop.run(None, &mut (), |_| {}).log_err();
            }) {
            Ok(thread) => thread,
            Err(error) => panic!("failed to spawn GPUI timer thread: {error}"),
        };

        background_threads.push(timer_thread);

        Self {
            main_sender,
            main_queue_sender,
            timer_sender,
            background_queue,
            _background_threads: background_threads,
            main_thread_id: thread::current().id(),
        }
    }
}

impl Drop for LinuxDispatcher {
    fn drop(&mut self) {
        self.background_queue.close();
    }
}

impl PlatformDispatcher for LinuxDispatcher {
    fn is_main_thread(&self) -> bool {
        thread::current().id() == self.main_thread_id
    }

    fn dispatch(&self, runnable: Runnable, _: Option<TaskLabel>) {
        self.dispatch_with_priority(runnable, Priority::Medium, None);
    }

    fn dispatch_with_priority(
        &self,
        runnable: Runnable,
        priority: Priority,
        _label: Option<TaskLabel>,
    ) {
        self.background_queue.push(runnable, priority);
    }

    fn dispatch_on_main_thread(&self, runnable: Runnable) {
        self.dispatch_on_main_thread_with_priority(runnable, Priority::Medium);
    }

    fn dispatch_on_main_thread_with_priority(&self, runnable: Runnable, priority: Priority) {
        match self.main_queue_sender.send(priority, runnable) {
            Ok(()) => {
                if let Err(()) = self.main_sender.send(()) {
                    log::warn!("LinuxDispatcher::dispatch_on_main_thread dropped wakeup");
                }
            }
            Err(error) => {
                // NOTE: Runnable may wrap a Future that is !Send.
                //
                // This is usually safe because we only poll it on the main thread.
                // However if the send fails, we know that:
                // 1. main_receiver has been dropped (which implies the app is shutting down)
                // 2. we are on a background thread.
                // It is not safe to drop something !Send on the wrong thread, and
                // the app will exit soon anyway, so we must forget the runnable.
                std::mem::forget(error.0);
            }
        }
    }

    fn dispatch_after(&self, duration: Duration, runnable: Runnable) {
        self.timer_sender
            .send(TimerAfter { duration, runnable })
            .ok();
    }
}
