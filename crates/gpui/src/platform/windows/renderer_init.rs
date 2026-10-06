//! Serial execution of renderer initialization.
//!
//! Renderer initialization creates the GPU device and compiles the render pipelines. Running it
//! on the shared background executor spreads those creations across arbitrary pool threads, and a
//! device is only shareable between windows that were initialized on the same thread. Initializing
//! on one dedicated thread for the whole process therefore lets a second window reuse the device
//! and the compiled pipelines the first window already paid for.
//!
//! The finished renderer is handed to the window's own thread afterwards, exactly as it was when
//! initialization ran on the pool.

use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

/// Work performed on the renderer initialization thread.
type Job = Box<dyn FnOnce() + Send + 'static>;

fn queue() -> &'static Mutex<Sender<Job>> {
    static QUEUE: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("gpui-renderer-init".to_string())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    job();
                }
            });
        if let Err(error) = spawned {
            log::error!("failed to start the GPUI renderer initialization thread: {error}");
        }
        Mutex::new(sender)
    })
}

/// Runs `job` on the renderer initialization thread, in submission order.
pub(super) fn spawn(job: impl FnOnce() + Send + 'static) {
    let queue = queue().lock().unwrap_or_else(|poison| poison.into_inner());
    if queue.send(Box::new(job)).is_err() {
        log::error!("GPUI renderer initialization thread is unavailable; renderer will not start");
    }
}
