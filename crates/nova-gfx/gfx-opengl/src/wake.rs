//! Owner-created, shared-context GPU completion notifications. No swap or drawing
//! occurs on the waiter. GPU readiness is only a hint; the owner rechecks status.
use glow::HasContext as _;
use glutin::{
    context::NotCurrentContext, display::Display, prelude::*,
    surface::{PbufferSurface, Surface},
};
use std::{
    ffi::CString,
    sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}, mpsc},
    thread::{self, JoinHandle},
};

type Callback = Box<dyn FnOnce() + Send + 'static>;
const MAX_PENDING: usize = 8;
const WAIT_NS: i32 = 16_000_000;

#[derive(Default)]
struct State {
    done: bool,
    callback: Option<Callback>,
}

type SharedState = Arc<Mutex<State>>;

// GL sync objects are shared by contexts in a share group. The graphics
// owner retains this handle until the worker publishes that its wait ended.
struct SharedFence(glow::NativeFence);
// SAFETY: no Rust memory is accessed via the handle; its lifetime is
// protected by the owner registration and the native share group.
unsafe impl Send for SharedFence {}

struct Task {
    fence: SharedFence,
    state: SharedState,
}

struct Registration {
    serial: u32,
    fence: glow::NativeFence,
    state: SharedState,
    retired: bool,
}

struct Worker {
    sender: mpsc::Sender<Task>,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

#[derive(Default)]
pub(super) struct Wakes {
    worker: Option<Worker>,
    disabled: Arc<AtomicBool>,
    registrations: Vec<Registration>,
}

fn finish(state: &SharedState, notify: bool) {
    let callback = {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state.done = true;
        state.callback.take()
    };
    if notify {
        if let Some(callback) = callback {
            // A host callback must not strand the remaining native handles.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback));
        }
    }
}

impl Wakes {
    pub(super) fn initialized(&self) -> bool { self.worker.is_some() }
    pub(super) fn unavailable(&self) -> bool { self.disabled.load(Ordering::Acquire) }
    pub(super) fn disable(&self) { self.disabled.store(true, Ordering::Release); }

    pub(super) fn start(&mut self, context: NotCurrentContext, surface: Surface<PbufferSurface>, display: Display) {
        let (sender, receiver) = mpsc::channel::<Task>();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let disabled = self.disabled.clone();
        let thread = thread::Builder::new().name("nova-gl-wake".into()).spawn(move || {
            let current = context.make_current(&surface);
            if current.is_err() {
                disabled.store(true, Ordering::Release);
                for task in receiver {
                    finish(&task.state, !worker_stop.load(Ordering::Acquire));
                }
                return;
            }
            let current = current.expect("checked wait context");
            // SAFETY: this thread owns its own current shared GL context.
            let gl = unsafe { glow::Context::from_loader_function(|name| {
                display.get_proc_address(&CString::new(name).expect("GL procedure name"))
            }) };
            for task in receiver {
                while !worker_stop.load(Ordering::Acquire) && !disabled.load(Ordering::Acquire) {
                    // The producer already flushed when creating this sync;
                    // a flush from this other context cannot replace that.
                    let status = unsafe { gl.client_wait_sync(task.fence.0, 0, WAIT_NS) };
                    if status == glow::TIMEOUT_EXPIRED {
                        continue;
                    }
                    if status != glow::ALREADY_SIGNALED && status != glow::CONDITION_SATISFIED {
                        disabled.store(true, Ordering::Release);
                    }
                    break;
                }
                // After this publication the worker never references the fence.
                finish(&task.state, !worker_stop.load(Ordering::Acquire));
            }
            let _ = current.make_not_current();
        });
        match thread {
            Ok(thread) => self.worker = Some(Worker { sender, stop, thread }),
            Err(error) => {
                log::debug!("OpenGL wake thread unavailable: {error}");
                self.disable();
            }
        }
    }

    pub(super) fn register(&mut self, serial: u32, fence: glow::NativeFence, callback: Callback) -> bool {
        if self.unavailable() { return false; }
        if let Some(record) = self.registrations.iter().find(|record| record.serial == serial) {
            let mut state = record.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.done { return false; }
            state.callback = Some(callback);
            return true;
        }
        if self.registrations.len() >= MAX_PENDING { return false; }
        let Some(worker) = &self.worker else { return false; };
        let state = Arc::new(Mutex::new(State { done: false, callback: Some(callback) }));
        if worker.sender.send(Task { fence: SharedFence(fence), state: state.clone() }).is_err() {
            self.disable();
            return false;
        }
        self.registrations.push(Registration { serial, fence, state, retired: false });
        true
    }

    pub(super) fn retire(&mut self, serial: u32) -> bool {
        if let Some(record) = self.registrations.iter_mut().find(|record| record.serial == serial) {
            record.retired = true;
            true
        } else { false }
    }

    pub(super) fn collect(&mut self, gl: &glow::Context) {
        self.registrations.retain(|record| {
            if !record.state.lock().unwrap_or_else(|error| error.into_inner()).done {
                return true;
            }
            if record.retired {
                // SAFETY: no native waiter still accesses this owner-retired sync.
                unsafe { gl.delete_sync(record.fence); }
            }
            false
        });
    }

    pub(super) fn shutdown(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.stop.store(true, Ordering::Release);
            drop(worker.sender);
            let _ = worker.thread.join();
        }
        // Even if a driver/loader panic terminated the thread, no native wait
        // remains after join. The owner can reclaim all retired syncs.
        for record in &self.registrations {
            finish(&record.state, false);
        }
    }
}
