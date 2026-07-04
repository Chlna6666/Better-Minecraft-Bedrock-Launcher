use crate::{ForegroundTaskQueue, PlatformDispatcher, TaskLabel};
use async_task::Runnable;
use calloop::{
    EventLoop,
    channel::{self, Sender},
    timer::TimeoutAction,
};
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use util::ResultExt;

struct TimerAfter {
    duration: Duration,
    runnable: Runnable,
}

pub(crate) struct LinuxDispatcher {
    main_sender: Sender<()>,
    main_queue: Arc<ForegroundTaskQueue>,
    timer_sender: Sender<TimerAfter>,
    background_sender: flume::Sender<Runnable>,
    _background_threads: Vec<thread::JoinHandle<()>>,
    main_thread_id: thread::ThreadId,
}

impl LinuxDispatcher {
    pub fn new(main_sender: Sender<()>, main_queue: Arc<ForegroundTaskQueue>) -> Self {
        let (background_sender, background_receiver) = flume::unbounded::<Runnable>();
        let thread_count = std::thread::available_parallelism()
            .map(|i| i.get())
            .unwrap_or(1);

        let mut background_threads = (0..thread_count)
            .map(|i| {
                let receiver = background_receiver.clone();
                std::thread::Builder::new()
                    .name(format!("Worker-{i}"))
                    .spawn(move || {
                        for runnable in receiver {
                            let start = Instant::now();

                            runnable.run();

                            log::trace!(
                                "background thread {}: ran runnable. took: {:?}",
                                i,
                                start.elapsed()
                            );
                        }
                    })
                    .unwrap()
            })
            .collect::<Vec<_>>();

        let (timer_sender, timer_channel) = calloop::channel::channel::<TimerAfter>();
        let timer_thread = std::thread::Builder::new()
            .name("Timer".to_owned())
            .spawn(|| {
                let mut event_loop: EventLoop<()> =
                    EventLoop::try_new().expect("Failed to initialize timer loop!");

                let handle = event_loop.handle();
                let timer_handle = event_loop.handle();
                handle
                    .insert_source(timer_channel, move |e, _, _| {
                        if let channel::Event::Msg(timer) = e {
                            // This has to be in an option to satisfy the borrow checker. The callback below should only be scheduled once.
                            let mut runnable = Some(timer.runnable);
                            timer_handle
                                .insert_source(
                                    calloop::timer::Timer::from_duration(timer.duration),
                                    move |_, _, _| {
                                        if let Some(runnable) = runnable.take() {
                                            runnable.run();
                                        }
                                        TimeoutAction::Drop
                                    },
                                )
                                .expect("Failed to start timer");
                        }
                    })
                    .expect("Failed to start timer thread");

                event_loop.run(None, &mut (), |_| {}).log_err();
            })
            .unwrap();

        background_threads.push(timer_thread);

        Self {
            main_sender,
            main_queue,
            timer_sender,
            background_sender,
            _background_threads: background_threads,
            main_thread_id: thread::current().id(),
        }
    }
}

impl PlatformDispatcher for LinuxDispatcher {
    fn is_main_thread(&self) -> bool {
        thread::current().id() == self.main_thread_id
    }

    fn dispatch(&self, runnable: Runnable, _: Option<TaskLabel>) {
        self.background_sender.send(runnable).unwrap();
    }

    fn dispatch_on_main_thread(&self, runnable: Runnable) {
        if self.main_queue.push(runnable) && self.main_sender.send(()).is_err() {
            // NOTE: Runnable may wrap a Future that is !Send.
            //
            // This is usually safe because we only poll it on the main thread.
            // However if the send fails, we know that main_receiver has been
            // dropped and the app is shutting down. It is not safe to drop
            // pending !Send futures on this thread, so leak the queued runnables.
            self.main_queue.forget_pending();
        }
    }

    fn dispatch_after(&self, duration: Duration, runnable: Runnable) {
        self.timer_sender
            .send(TimerAfter { duration, runnable })
            .ok();
    }
}
