use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{ThreadId, current},
    time::Duration,
};

use async_task::Runnable;
use flume::Sender;
use windows::{
    System::Threading::{
        ThreadPool, ThreadPoolTimer, TimerElapsedHandler, WorkItemHandler, WorkItemPriority,
    },
    Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::PostMessageW,
    },
};

use crate::{
    HWND, PlatformDispatcher, SafeHwnd, TaskLabel, WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD,
};

pub(crate) struct WindowsDispatcher {
    main_sender: Sender<Runnable>,
    main_thread_wakeup_pending: Arc<AtomicBool>,
    main_thread_id: ThreadId,
    platform_window_handle: SafeHwnd,
    validation_number: usize,
}

impl WindowsDispatcher {
    pub(crate) fn new(
        main_sender: Sender<Runnable>,
        main_thread_wakeup_pending: Arc<AtomicBool>,
        platform_window_handle: HWND,
        validation_number: usize,
    ) -> Self {
        let main_thread_id = current().id();
        let platform_window_handle = platform_window_handle.into();

        WindowsDispatcher {
            main_sender,
            main_thread_wakeup_pending,
            main_thread_id,
            platform_window_handle,
            validation_number,
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
                    unsafe {
                        if let Err(error) = PostMessageW(
                            Some(self.platform_window_handle.as_raw()),
                            WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD,
                            WPARAM(self.validation_number),
                            LPARAM(0),
                        ) {
                            self.main_thread_wakeup_pending
                                .store(false, Ordering::Release);
                            log::error!(
                                "WindowsDispatcher::dispatch_on_main_thread post failed: {:?}",
                                error
                            );
                        }
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
