#![expect(
    unsafe_code,
    reason = "Windows thread-pool scheduling and timer resolution use audited Win32 FFI"
)]

use std::{
    ffi::c_void,
    ptr::NonNull,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{ThreadId, current},
    time::Duration,
};

use async_task::Runnable;
use flume::Sender;
use windows::Win32::{
    Foundation::FILETIME,
    Media::{timeBeginPeriod, timeEndPeriod},
    System::Threading::{
        CloseThreadpoolTimer, CreateThreadpoolTimer, PTP_CALLBACK_INSTANCE, PTP_TIMER,
        SetThreadpoolTimer, TP_CALLBACK_ENVIRON_V3, TP_CALLBACK_PRIORITY_NORMAL,
        TrySubmitThreadpoolCallback,
    },
};
use winit::event_loop::EventLoopProxy;

use super::WindowsUserEvent;
use crate::{PlatformDispatcher, TaskLabel, TimerResolutionGuard};

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
        let environment = TP_CALLBACK_ENVIRON_V3 {
            Version: 3,
            CallbackPriority: TP_CALLBACK_PRIORITY_NORMAL,
            Size: std::mem::size_of::<TP_CALLBACK_ENVIRON_V3>() as u32,
            ..Default::default()
        };

        // Transfer ownership to the native callback. If the OS refuses submission we
        // intentionally leak the scheduled runnable: dropping it would cancel the task and a
        // later poll of an awaiter can panic with "Task polled after completion". Submission
        // failure is expected only during shutdown or extreme resource exhaustion.
        let context = runnable.into_raw().as_ptr() as *mut c_void;
        // SAFETY: context is an async-task Runnable raw pointer consumed exactly once by
        // run_work_callback when Windows executes the submitted callback. The callback
        // environment is stack-owned only for the duration of submission, as required by Win32.
        if let Err(error) = unsafe {
            TrySubmitThreadpoolCallback(
                Some(run_work_callback),
                Some(context),
                Some(&environment),
            )
        } {
            log::error!(
                "WindowsDispatcher::dispatch_on_threadpool failed: {:?}",
                error
            );
        }
    }

    fn dispatch_on_threadpool_after(&self, runnable: Runnable, duration: Duration) {
        // See dispatch_on_threadpool: raw ownership stays with the native callback. On creation
        // failure the runnable is intentionally leaked instead of being cancelled under an
        // awaiter.
        let context = runnable.into_raw().as_ptr() as *mut c_void;

        // SAFETY: context is a valid async-task Runnable raw pointer. The timer callback consumes
        // it exactly once and closes the one-shot thread-pool timer after running the task.
        let timer = match unsafe { CreateThreadpoolTimer(Some(run_timer_callback), Some(context), None) } {
            Ok(timer) => timer,
            Err(error) => {
                log::error!(
                    "WindowsDispatcher::dispatch_on_threadpool_after failed duration={:?}: {:?}",
                    duration,
                    error
                );
                return;
            }
        };

        // Negative FILETIME values are relative delays expressed in 100ns ticks.
        let ticks = (duration.as_nanos() / 100).min(i64::MAX as u128) as i64;
        let due = (-ticks) as u64;
        let due_time = FILETIME {
            dwLowDateTime: due as u32,
            dwHighDateTime: (due >> 32) as u32,
        };

        // SAFETY: timer was created above and remains valid until run_timer_callback closes it.
        unsafe {
            SetThreadpoolTimer(timer, Some(&due_time), 0, None);
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

    fn increase_timer_resolution(&self) -> TimerResolutionGuard {
        const TIMER_PERIOD_MS: u32 = 1;

        // SAFETY: timeBeginPeriod accepts a period in milliseconds and carries no pointer
        // invariants. A successful request is paired with timeEndPeriod by the returned guard.
        let result = unsafe { timeBeginPeriod(TIMER_PERIOD_MS) };
        if result != 0 {
            log::debug!(
                "WindowsDispatcher::increase_timer_resolution failed period={}ms result={}",
                TIMER_PERIOD_MS,
                result
            );
            return TimerResolutionGuard::noop();
        }

        TimerResolutionGuard::new(|| {
            // SAFETY: paired with the successful timeBeginPeriod call above using the same period.
            let _ = unsafe { timeEndPeriod(TIMER_PERIOD_MS) };
        })
    }
}

unsafe extern "system" fn run_work_callback(
    _instance: PTP_CALLBACK_INSTANCE,
    context: *mut c_void,
) {
    // SAFETY: context was produced by Runnable::into_raw in dispatch_on_threadpool and this
    // callback is the unique consumer installed for that submission.
    let runnable =
        unsafe { Runnable::<()>::from_raw(NonNull::new_unchecked(context as *mut ())) };
    runnable.run();
}

unsafe extern "system" fn run_timer_callback(
    _instance: PTP_CALLBACK_INSTANCE,
    context: *mut c_void,
    timer: PTP_TIMER,
) {
    // SAFETY: context was produced by Runnable::into_raw in dispatch_on_threadpool_after and this
    // one-shot callback is the unique consumer.
    let runnable =
        unsafe { Runnable::<()>::from_raw(NonNull::new_unchecked(context as *mut ())) };
    runnable.run();

    // SAFETY: timer is the callback's valid PTP_TIMER and is no longer armed after this one-shot
    // callback completes.
    unsafe {
        CloseThreadpoolTimer(timer);
    }
}
