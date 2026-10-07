//! GPU ownership. Only immutable scenes and typed control work cross this boundary.
//!
//! Every window's device, pipelines, targets and extension renderers are created, consumed and
//! destroyed on the same GPU thread. Keeping one thread also preserves the thread-local device
//! and pipeline registries shared by windows on every supported Nova platform.
//! Thread-affine backend state cannot be scheduled on a work-stealing background executor;
//! application shutdown explicitly drains and joins this platform lifecycle thread.

use parking_lot::Mutex;
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle, ThreadId},
    time::{Duration, Instant},
};

use crate::{
    ActivePresentationFrame, DevicePixels, GpuSpecs, GpuiMemoryTrimLevel, PresentationPacket, Size,
    platform::{NovaRenderer, frame::ActivePresentationTiming},
};
use anyhow::{Result, anyhow};
#[cfg(target_os = "windows")]
use futures::channel::oneshot;

mod queue;
mod worker;

use queue::{Command, Queue};
use worker::{Entry, Worker};

#[derive(Debug)]
pub(crate) struct RenderOwnerFrame {
    pub(crate) submitted: bool,
    pub(crate) pending: bool,
    pub(crate) failed: bool,
    pub(crate) ready_enqueued_at: Option<Instant>,
    pub(crate) completed_animations: smallvec::SmallVec<[crate::SceneAnimationCompletion; 4]>,
}

#[derive(Default)]
struct Status {
    pending: AtomicBool,
    submitted: AtomicBool,
}

type Job = Box<dyn FnOnce(&mut Worker) + Send>;

struct Executor {
    sender: Sender<Job>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

static EXECUTOR: OnceLock<Result<Executor, std::io::Error>> = OnceLock::new();

fn sender() -> Result<&'static Sender<Job>> {
    EXECUTOR
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Job>();
            let worker = thread::Builder::new()
                .name("gpui-gpu-owner".into())
                .spawn(move || {
                    let mut worker = Worker::default();
                    while let Ok(job) = receiver.recv() {
                        job(&mut worker);
                        if worker.is_closing() {
                            break;
                        }
                    }
                })?;
            Ok(Executor {
                sender,
                worker: Mutex::new(Some(worker)),
            })
        })
        .as_ref()
        .map(|executor| &executor.sender)
        .map_err(|error| anyhow!("failed to start GPU owner: {error}"))
}

/// Queues device preparation without waiting for it, overlapping GPU and native/UI startup.
///
/// The job and subsequent window initialization use the same GPU thread and device registry.
/// Device errors are logged here; normal window initialization retries uncached failures.
///
/// # Errors
///
/// Returns an error if the GPU owner cannot start or accept the preparation job.
#[cfg(target_os = "windows")]
pub(crate) fn prepare_device(options: crate::RendererOptions) -> Result<()> {
    sender()?
        .send(Box::new(move |_| {
            let started_at = Instant::now();
            match NovaRenderer::prepare_device(&options) {
                Ok(()) => log::info!(
                    "GPUI device preparation: backend={} elapsed_ms={}",
                    options.backend,
                    started_at.elapsed().as_millis(),
                ),
                Err(error) => log::warn!("GPUI device preparation failed: {error:#}"),
            }
        }))
        .map_err(|_| anyhow!("GPU owner stopped before device preparation"))
}

/// Drains window resources and joins the GPU owner after the native/UI event loops exit.
pub(crate) fn shutdown() {
    let Some(Ok(executor)) = EXECUTOR.get() else {
        return;
    };
    let Some(worker) = executor.worker.lock().take() else {
        return;
    };
    if executor.sender.send(Box::new(Worker::shutdown)).is_err() {
        log::debug!("GPU owner already stopped before shutdown");
    }
    if worker.join().is_err() {
        log::error!("GPU owner panicked during shutdown");
    }
}

/// A window-side producer. It contains no renderer or backend handle.
pub(crate) struct RenderOwner {
    id: u64,
    queue: Arc<Queue>,
    sender: Sender<Job>,
    owner_thread: ThreadId,
    status: Arc<Status>,
    gpu_specs: GpuSpecs,
    #[cfg(not(target_os = "windows"))]
    atlas: Arc<dyn crate::PlatformAtlas>,
    #[cfg(not(target_os = "windows"))]
    viewport_size: Size<DevicePixels>,
}

impl RenderOwner {
    #[cfg(not(target_os = "windows"))]
    pub(crate) fn new(
        create: impl FnOnce() -> Result<NovaRenderer> + Send + 'static,
        report: Arc<dyn Fn(RenderOwnerFrame) + Send + Sync>,
    ) -> Result<Self> {
        let (reply, receiver) = mpsc::sync_channel(1);
        let sender = sender()?.clone();
        let owner_sender = sender.clone();
        sender
            .send(Box::new(move |worker| {
                let result = Self::create(worker, create, Box::new(()), report, owner_sender);
                if reply.send(result).is_err() {
                    log::debug!("GPU owner initialization caller dropped");
                }
            }))
            .map_err(|_| anyhow!("GPU owner stopped before initialization"))?;
        receiver
            .recv()
            .map_err(|_| anyhow!("GPU owner initialization failed"))?
    }

    #[cfg(target_os = "windows")]
    pub(super) fn initialize(
        create: impl FnOnce() -> Result<NovaRenderer> + Send + 'static,
        keep_alive: Box<dyn std::any::Any + Send>,
        report: Arc<dyn Fn(RenderOwnerFrame) + Send + Sync>,
        reply: oneshot::Sender<Result<Self>>,
    ) -> Result<()> {
        let sender = sender()?.clone();
        let owner_sender = sender.clone();
        sender
            .send(Box::new(move |worker| {
                let result = Self::create(worker, create, keep_alive, report, owner_sender);
                if reply.send(result).is_err() {
                    log::debug!("GPU owner initialization receiver was dropped");
                }
            }))
            .map_err(|_| anyhow!("GPU owner stopped before initialization"))
    }

    fn create(
        worker: &mut Worker,
        create: impl FnOnce() -> Result<NovaRenderer>,
        keep_alive: Box<dyn std::any::Any + Send>,
        report: Arc<dyn Fn(RenderOwnerFrame) + Send + Sync>,
        sender: Sender<Job>,
    ) -> Result<Self> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let renderer = create()?;
        let gpu_specs = renderer.gpu_specs();
        #[cfg(not(target_os = "windows"))]
        let atlas = renderer.platform_atlas();
        #[cfg(not(target_os = "windows"))]
        let viewport_size = renderer.viewport_size();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let queue = Arc::new(Queue::default());
        let status = Arc::new(Status::default());
        worker.insert(
            id,
            Entry::new(renderer, queue.clone(), status.clone(), keep_alive, report),
        );
        Ok(Self {
            id,
            queue,
            sender,
            owner_thread: thread::current().id(),
            status,
            gpu_specs,
            #[cfg(not(target_os = "windows"))]
            atlas,
            #[cfg(not(target_os = "windows"))]
            viewport_size,
        })
    }

    fn enqueue(&self, command: Command) -> Result<()> {
        let id = self.id;
        let sender = self.sender.clone();
        self.queue.enqueue(command, move || {
            sender
                .send(Box::new(move |worker| worker.drain(id)))
                .map_err(|_| anyhow!("GPU owner has stopped"))
        })
    }

    fn control(&self, operation: impl FnOnce(&mut NovaRenderer) + Send + 'static) -> Result<()> {
        self.enqueue(Command::Call(Box::new(operation)))
    }

    fn submit(&self, packet: PresentationPacket, framebuffer_only: bool) -> Result<bool> {
        // Only the initial visibility handshake waits for GPU work. Subsequent commits are
        // acknowledged as queued; successful submission is reported by the owner separately.
        let first = !self.status.submitted.load(Ordering::Acquire);
        let handshake = first.then(|| mpsc::sync_channel(1));
        let (reply, receiver) = match handshake {
            Some((reply, receiver)) => (Some(reply), Some(receiver)),
            None => (None, None),
        };
        self.status.pending.store(true, Ordering::Release);
        self.enqueue(Command::Draw {
            packet,
            framebuffer_only,
            reply,
        })?;
        if let Some(receiver) = receiver {
            receiver
                .recv()
                .map_err(|_| anyhow!("GPU owner stopped before first frame"))?
        } else {
            Ok(false)
        }
    }

    pub(super) fn draw(&self, packet: PresentationPacket) -> Result<bool> {
        self.submit(packet, false)
    }

    pub(super) fn present_framebuffer_only(&self, packet: PresentationPacket) -> Result<bool> {
        self.submit(packet, true)
    }

    pub(super) fn present_active_frame(
        &self,
        now: Instant,
        timing: Option<ActivePresentationTiming>,
    ) -> Result<Option<ActivePresentationFrame>> {
        if !self.has_active_presentation_animations() {
            return Ok(None);
        }
        self.enqueue(Command::Tick(now, timing))?;
        Ok(Some(ActivePresentationFrame {
            continues: true,
            completed_animations: smallvec::SmallVec::new(),
        }))
    }

    pub(super) fn has_active_presentation_animations(&self) -> bool {
        self.status.pending.load(Ordering::Acquire)
    }

    #[cfg(target_os = "windows")]
    pub(super) fn stretch_for_pending_resize(&self, size: Size<DevicePixels>) {
        if let Err(error) =
            self.control(move |renderer| renderer.stretch_surface_for_pending_resize(size))
        {
            log::error!("failed to queue GPU resize stretch: {error:#}");
        }
    }

    /// Accepts a resize in command order; target recreation runs on the next GPU frame.
    pub(super) fn update_drawable_size(&mut self, size: Size<DevicePixels>) -> Result<()> {
        self.enqueue(Command::Resize(size))?;
        #[cfg(not(target_os = "windows"))]
        {
            self.viewport_size = size;
        }
        Ok(())
    }

    pub(super) fn set_frame_interval(&self, interval: Option<Duration>) {
        if let Err(error) = self.control(move |renderer| renderer.set_frame_interval(interval)) {
            log::error!("failed to queue GPU frame interval: {error:#}");
        }
    }

    pub(super) fn update_transparency(&self, transparent: bool) {
        if let Err(error) = self.enqueue(Command::Transparency(transparent)) {
            log::error!("failed to queue GPU transparency: {error:#}");
        }
    }

    pub(super) fn trim_gpui_memory(&self, level: GpuiMemoryTrimLevel) {
        if let Err(error) = self.control(move |renderer| renderer.trim_gpui_memory(level)) {
            log::error!("failed to queue GPU memory trim: {error:#}");
        }
    }

    pub(super) fn gpu_specs(&self) -> GpuSpecs {
        self.gpu_specs.clone()
    }

    #[cfg(not(target_os = "windows"))]
    pub(super) fn platform_atlas(&self) -> Arc<dyn crate::PlatformAtlas> {
        self.atlas.clone()
    }

    #[cfg(not(target_os = "windows"))]
    pub(super) fn viewport_size(&self) -> Size<DevicePixels> {
        self.viewport_size
    }

    pub(super) fn has_submitted_frame(&self) -> bool {
        self.status.submitted.load(Ordering::Acquire)
    }
}

impl Drop for RenderOwner {
    fn drop(&mut self) {
        if self.queue.is_closed() {
            return;
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        if let Err(error) = self.enqueue(Command::Shutdown(reply)) {
            if self.queue.is_closed() {
                log::trace!("GPU renderer was closed by application shutdown");
            } else {
                log::error!("failed to shut down GPU renderer: {error:#}");
            }
            return;
        }
        // A dropped initialization receiver can release this proxy on the GPU owner itself.
        // In that case its queued shutdown runs after initialization returns, without self-wait.
        if thread::current().id() != self.owner_thread && receiver.recv().is_err() {
            log::error!("GPU owner stopped before renderer shutdown completed");
        }
    }
}
