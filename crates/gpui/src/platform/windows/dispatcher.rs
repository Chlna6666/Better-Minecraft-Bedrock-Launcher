use std::{
    cell::RefCell,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{ThreadId, current},
    time::Duration,
};

use async_task::Runnable;
use flume::Sender;
use windows::System::Threading::{
    ThreadPool, ThreadPoolTimer, TimerElapsedHandler, WorkItemHandler, WorkItemPriority,
};
use winit::event_loop::EventLoopProxy;

use super::WindowsUserEvent;
use crate::{PlatformDispatcher, TaskLabel};

pub(crate) struct WindowsDispatcher {
    main_sender: Sender<Runnable>,
    main_thread_wakeup_pending: Arc<AtomicBool>,
    main_thread_id: ThreadId,
    event_loop_proxy: Arc<Mutex<Option<EventLoopProxy<WindowsUserEvent>>>>,
}

impl WindowsDispatcher {
    pub(crate) fn new(
        main_sender: Sender<Runnable>,
        main_thread_wakeup_pending: Arc<AtomicBool>,
        event_loop_proxy: Arc<Mutex<Option<EventLoopProxy<WindowsUserEvent>>>>,
    ) -> Self {
        let main_thread_id = current().id();

        WindowsDispatcher {
            main_sender,
            main_thread_wakeup_pending,
            main_thread_id,
            event_loop_proxy,
        }
    }

    fn dispatch_on_threadpool(&self, runnable: Runnable) {
        let handler = {
            let task_wrapper = RefCell::new(Some(runnable));
            WorkItemHandler::new(move |_| {
                if let Some(task) = task_wrapper.borrow_mut().take() {
                    task.run();
                }
                Ok(())
            })
        };
        if let Err(error) = ThreadPool::RunWithPriorityAsync(&handler, WorkItemPriority::High) {
            log::error!(
                "WindowsDispatcher::dispatch_on_threadpool failed: {:?}",
                error
            );
        }
    }

    fn dispatch_on_threadpool_after(&self, runnable: Runnable, duration: Duration) {
        let handler = {
            let task_wrapper = RefCell::new(Some(runnable));
            TimerElapsedHandler::new(move |_| {
                if let Some(task) = task_wrapper.borrow_mut().take() {
                    task.run();
                }
                Ok(())
            })
        };
        if let Err(error) = ThreadPoolTimer::CreateTimer(&handler, duration.into()) {
            log::error!(
                "WindowsDispatcher::dispatch_on_threadpool_after failed duration={:?}: {:?}",
                duration,
                error
            );
        }
    }
}

impl PlatformDispatcher for WindowsDispatcher {
    fn is_main_thread(&self) -> bool {
        current().id() == self.main_thread_id
    }

    fn dispatch(&self, runnable: Runnable, label: Option<TaskLabel>) {
        self.dispatch_on_threadpool(runnable);
        if let Some(label) = label {
            log::debug!("TaskLabel: {label:?}");
        }
    }

    fn dispatch_on_main_thread(&self, runnable: Runnable) {
        match self.main_sender.send(runnable) {
            Ok(_) => {
                if !self.main_thread_wakeup_pending.swap(true, Ordering::AcqRel) {
                    let event_loop_proxy = self.event_loop_proxy.lock().unwrap().clone();
                    if let Some(event_loop_proxy) = event_loop_proxy {
                        if let Err(error) =
                            event_loop_proxy.send_event(WindowsUserEvent::RunMainThreadTasks)
                        {
                            self.main_thread_wakeup_pending
                                .store(false, Ordering::Release);
                            log::error!(
                                "WindowsDispatcher::dispatch_on_main_thread send failed: {:?}",
                                error
                            );
                        }
                    } else {
                        self.main_thread_wakeup_pending
                            .store(false, Ordering::Release);
                        log::warn!(
                            "WindowsDispatcher::dispatch_on_main_thread dropped wakeup before event loop initialization"
                        );
                    }
                }
            }
            Err(runnable) => {
                // NOTE: Runnable may wrap a Future that is !Send.
                //
                // This is usually safe because we only poll it on the main thread.
                // However if the send fails, we know that:
                // 1. main_receiver has been dropped (which implies the app is shutting down)
                // 2. we are on a background thread.
                // It is not safe to drop something !Send on the wrong thread, and
                // the app will exit soon anyway, so we must forget the runnable.
                std::mem::forget(runnable);
            }
        }
    }

    fn dispatch_after(&self, duration: Duration, runnable: Runnable) {
        self.dispatch_on_threadpool_after(runnable, duration);
    }
}
