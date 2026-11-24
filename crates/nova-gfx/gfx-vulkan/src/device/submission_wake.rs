//! Owner-registered one-shot queue checkpoints. Vulkan has no general fence
//! completion callback: one sleeping worker per active queue waits in the
//! driver. Never wait on a reusable WSI or command-pool fence off-owner.
use super::{Arc, Error, Result, SubmissionId, VulkanError, create_fence, fence_is_complete, vk};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Mutex, PoisonError, mpsc},
    thread::{self, JoinHandle},
};

type Callback = Box<dyn FnOnce() + Send + 'static>;
const MAX_PENDING_WAKES_PER_QUEUE: usize = 8;

#[derive(Default)]
struct WakeState {
    done: bool,
    callback: Option<Callback>,
}

#[derive(Default)]
struct Wake {
    state: Mutex<WakeState>,
}

impl Wake {
    fn arm(&self, callback: Callback) {
        let callback = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.done { Some(callback) } else {
                state.callback = Some(callback);
                None
            }
        };
        if let Some(callback) = callback { callback(); }
    }

    fn cancel(&self) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).callback = None;
    }

    fn done(&self) -> bool {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).done
    }

    fn finish(&self) {
        let callback = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.done = true;
            state.callback.take()
        };
        if let Some(callback) = callback {
            let _ = catch_unwind(AssertUnwindSafe(callback));
        }
    }
}

struct Task {
    fence: vk::Fence,
    wake: Arc<Wake>,
}

struct Worker {
    queue: vk::Queue,
    sender: mpsc::Sender<Task>,
    thread: JoinHandle<()>,
}

impl Worker {
    fn new(device: Arc<ash::Device>, queue: vk::Queue) -> Result<Self> {
        let (sender, receiver) = mpsc::channel::<Task>();
        let thread = thread::Builder::new().name("nova-vulkan-fence".into()).spawn(move || {
            while let Ok(task) = receiver.recv() {
                // SAFETY: The owner retains this dedicated fence until this
                // wait has returned. It is never reset or reused.
                let result = unsafe { device.wait_for_fences(&[task.fence], true, u64::MAX) };
                if let Err(error) = result {
                    log::warn!("Vulkan submission wake wait failed: {error:?}");
                }
                // Wake is a hint even on error; the owner polls real status.
                // No further use of the fence is made after finish begins.
                task.wake.finish();
            }
        }).map_err(|error| Error::Backend(format!("Vulkan wake worker: {error}")))?;
        Ok(Self { queue, sender, thread })
    }

    fn join(self) {
        drop(self.sender);
        let _ = self.thread.join();
    }
}

struct Registration {
    submission: SubmissionId,
    queue: vk::Queue,
    fence: vk::Fence,
    wake: Arc<Wake>,
}

#[derive(Default)]
pub(super) struct SubmissionWakes {
    workers: Vec<Worker>,
    pending: Vec<Registration>,
}

impl SubmissionWakes {
    pub(super) fn register(
        &mut self, device: &Arc<ash::Device>, queue: vk::Queue,
        submission: SubmissionId, callback: Callback,
    ) -> Result<bool> {
        if let Some(record) = self.pending.iter().find(|record| record.submission == submission) {
            record.wake.arm(callback);
            return Ok(true);
        }
        if self.pending.iter().filter(|record| record.queue == queue).count() >= MAX_PENDING_WAKES_PER_QUEUE {
            return Ok(false);
        }
        let worker = if let Some(index) = self.workers.iter().position(|worker| worker.queue == queue) {
            index
        } else {
            self.workers.push(Worker::new(device.clone(), queue)?);
            self.workers.len() - 1
        };
        let fence = create_fence(device, false)?;
        // SAFETY: only the graphics owner submits to this queue. A private,
        // empty checkpoint after the requested submission provides an
        // ordered wake hint without sharing the original fence's lifetime.
        if let Err(error) = unsafe { device.queue_submit(queue, &[vk::SubmitInfo::default()], fence) } {
            unsafe { device.destroy_fence(fence, None) };
            return Err(VulkanError::from(error).into());
        }
        let wake = Arc::new(Wake::default());
        wake.arm(callback);
        self.pending.push(Registration { submission, queue, fence, wake: wake.clone() });
        if self.workers[worker].sender.send(Task { fence, wake: wake.clone() }).is_err() {
            // The checkpoint was submitted; keep its fence alive until the
            // GPU really completes. No callback on failed registration.
            wake.cancel();
            wake.finish();
            return Ok(false);
        }
        Ok(true)
    }

    pub(super) fn cancel(&self, submission: SubmissionId) {
        if let Some(record) = self.pending.iter().find(|record| record.submission == submission) {
            record.wake.cancel();
        }
    }

    pub(super) fn collect(&mut self, device: &ash::Device) {
        self.pending.retain(|record| {
            if record.wake.done() && fence_is_complete(device, record.fence) {
                // SAFETY: GPU and native host wait are both finished.
                unsafe { device.destroy_fence(record.fence, None) };
                false
            } else { true }
        });
    }

    pub(super) fn shutdown(&mut self, device: &ash::Device) {
        for record in &self.pending { record.wake.cancel(); }
        for worker in self.workers.drain(..) { worker.join(); }
        // Caller already drained the device, and all host waits are joined.
        for record in self.pending.drain(..) {
            unsafe { device.destroy_fence(record.fence, None) };
        }
    }
}
