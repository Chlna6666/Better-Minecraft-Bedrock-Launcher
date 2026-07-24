#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use crate::{ForegroundTaskQueue, PlatformDispatcher, TaskLabel};
use async_task::Runnable;
use objc::{
    class, msg_send,
    runtime::{BOOL, YES},
    sel, sel_impl,
};
use std::{
    ffi::c_void,
    ptr::{NonNull, addr_of},
    sync::Arc,
    time::Duration,
};

/// All items in the generated file are marked as pub, so we're gonna wrap it in a separate mod to prevent
/// these pub items from leaking into public API.
pub(crate) mod dispatch_sys {
    include!(concat!(env!("OUT_DIR"), "/dispatch_sys.rs"));
}

use dispatch_sys::*;
pub(crate) fn dispatch_get_main_queue() -> dispatch_queue_t {
    addr_of!(_dispatch_main_q) as *const _ as dispatch_queue_t
}

pub(crate) struct MacDispatcher {
    main_queue: Arc<ForegroundTaskQueue>,
}

impl MacDispatcher {
    pub(crate) fn new() -> Self {
        Self {
            main_queue: Arc::new(ForegroundTaskQueue::new()),
        }
    }
}

impl PlatformDispatcher for MacDispatcher {
    fn is_main_thread(&self) -> bool {
        let is_main_thread: BOOL = unsafe { msg_send![class!(NSThread), isMainThread] };
        is_main_thread == YES
    }

    fn dispatch(&self, runnable: Runnable, _: Option<TaskLabel>) {
        unsafe {
            dispatch_async_f(
                dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_HIGH.try_into().unwrap(), 0),
                runnable.into_raw().as_ptr() as *mut c_void,
                Some(trampoline),
            );
        }
    }

    fn dispatch_on_main_thread(&self, runnable: Runnable) {
        if self.main_queue.push(runnable) {
            dispatch_main_queue_drain(self.main_queue.clone());
        }
    }

    fn dispatch_after(&self, duration: Duration, runnable: Runnable) {
        unsafe {
            let queue =
                dispatch_get_global_queue(DISPATCH_QUEUE_PRIORITY_HIGH.try_into().unwrap(), 0);
            let when = dispatch_time(DISPATCH_TIME_NOW as u64, duration.as_nanos() as i64);
            dispatch_after_f(
                when,
                queue,
                runnable.into_raw().as_ptr() as *mut c_void,
                Some(trampoline),
            );
        }
    }
}

extern "C" fn trampoline(runnable: *mut c_void) {
    let task = unsafe { Runnable::<()>::from_raw(NonNull::new_unchecked(runnable as *mut ())) };
    task.run();
}

fn dispatch_main_queue_drain(queue: Arc<ForegroundTaskQueue>) {
    unsafe {
        dispatch_async_f(
            dispatch_get_main_queue(),
            Arc::into_raw(queue) as *mut c_void,
            Some(main_queue_trampoline),
        );
    }
}

extern "C" fn main_queue_trampoline(queue: *mut c_void) {
    let queue = unsafe { Arc::from_raw(queue as *const ForegroundTaskQueue) };
    if queue.drain() {
        dispatch_main_queue_drain(queue.clone());
    }
}
