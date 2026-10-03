use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

use anyhow::{Result, anyhow};
use parking_lot::{Condvar, Mutex};

use super::{NovaRenderer, PlatformAtlas, frame::PlatformFrameRequestSender};
use crate::{
    ActivePresentationFrame, DevicePixels, GpuSpecs, GpuiMemoryTrimLevel, PlatformFrameRequest,
    PresentationPacket, SceneAnimationCompletionSender, Size,
    platform::frame::ActivePresentationTiming,
};

enum Command {
    Draw {
        packet: PresentationPacket,
        reply: Option<SyncSender<Result<bool>>>,
    },
    FramebufferOnly {
        packet: PresentationPacket,
        reply: Option<SyncSender<Result<bool>>>,
    },
    Tick(Instant),
    Call(Box<dyn FnOnce(&mut NovaRenderer) + Send>),
    Shutdown,
}

impl Command {
    fn is_barrier(&self) -> bool {
        matches!(self, Self::Call(_) | Self::Shutdown)
    }

    fn is_queued_presentation(&self) -> bool {
        matches!(
            self,
            Self::Draw { reply: None, .. } | Self::FramebufferOnly { reply: None, .. }
        )
    }

    fn merge_damage_from(&mut self, previous: Self) {
        match (self, previous) {
            (
                Self::Draw { packet, .. } | Self::FramebufferOnly { packet, .. },
                Self::Draw {
                    packet: previous, ..
                }
                | Self::FramebufferOnly {
                    packet: previous, ..
                },
            ) => packet.merge_pending_damage_from(&previous),
            _ => unreachable!("only like presentation commands are coalesced"),
        }
    }
}

#[derive(Default)]
struct QueueState {
    commands: VecDeque<Command>,
    closing: bool,
}

struct SharedQueue {
    state: Mutex<QueueState>,
    available: Condvar,
}

pub(crate) struct OwnedNovaRenderer {
    queue: Arc<SharedQueue>,
    worker: Option<JoinHandle<()>>,
    atlas: Arc<dyn PlatformAtlas>,
    viewport_size: Size<DevicePixels>,
    first_frame_presented: bool,
    pending_presentation: Arc<AtomicBool>,
    frame_requests: Arc<Mutex<Option<PlatformFrameRequestSender>>>,
    animation_completions: Arc<Mutex<Option<SceneAnimationCompletionSender>>>,
}

impl OwnedNovaRenderer {
    pub(crate) fn new(mut renderer: NovaRenderer) -> Result<Self> {
        let atlas = renderer.platform_atlas();
        let viewport_size = renderer.viewport_size();
        let queue = Arc::new(SharedQueue {
            state: Mutex::new(QueueState::default()),
            available: Condvar::new(),
        });
        let pending_presentation = Arc::new(AtomicBool::new(false));
        let frame_requests = Arc::new(Mutex::new(None));
        let animation_completions = Arc::new(Mutex::new(None));
        let worker_queue = queue.clone();
        let worker_pending = pending_presentation.clone();
        let worker_requests = frame_requests.clone();
        let worker_completions = animation_completions.clone();
        let worker = thread::Builder::new()
            .name("gpui-presentation".to_string())
            .spawn(move || {
                run_renderer(
                    &mut renderer,
                    worker_queue,
                    worker_pending,
                    worker_requests,
                    worker_completions,
                );
            })?;

        Ok(Self {
            queue,
            worker: Some(worker),
            atlas,
            viewport_size,
            first_frame_presented: false,
            pending_presentation,
            frame_requests,
            animation_completions,
        })
    }

    fn enqueue(&self, mut command: Command) -> Result<()> {
        let mut state = self.queue.state.lock();
        if state.closing {
            return Err(anyhow!("presentation owner is shutting down"));
        }

        let barrier = state
            .commands
            .iter()
            .rposition(Command::is_barrier)
            .map_or(0, |index| index + 1);
        match &command {
            Command::Draw { reply: None, .. } | Command::FramebufferOnly { reply: None, .. } => {
                if let Some(index) = (barrier..state.commands.len())
                    .rev()
                    .find(|index| state.commands[*index].is_queued_presentation())
                {
                    let previous = state.commands.remove(index).expect("index came from queue");
                    command.merge_damage_from(previous);
                }
            }
            Command::Tick(_) => {
                if let Some(index) = (barrier..state.commands.len())
                    .rev()
                    .find(|index| matches!(state.commands[*index], Command::Tick(_)))
                {
                    state.commands.remove(index);
                }
            }
            _ => {}
        }
        state.commands.push_back(command);
        self.queue.available.notify_one();
        Ok(())
    }

    fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut NovaRenderer) -> T + Send + 'static,
    ) -> Result<T> {
        let (reply, receiver) = sync_channel(1);
        self.enqueue(Command::Call(Box::new(move |renderer| {
            if reply.send(operation(renderer)).is_err() {
                log::trace!("discarding renderer call result after its caller dropped");
            }
        })))?;
        receiver
            .recv()
            .map_err(|_| anyhow!("presentation owner stopped before completing a renderer call"))
    }

    fn draw_sync(&self, packet: PresentationPacket, framebuffer_only: bool) -> Result<bool> {
        let (reply, receiver) = sync_channel(1);
        self.enqueue(if framebuffer_only {
            Command::FramebufferOnly {
                packet,
                reply: Some(reply),
            }
        } else {
            Command::Draw {
                packet,
                reply: Some(reply),
            }
        })?;
        receiver
            .recv()
            .map_err(|_| anyhow!("presentation owner stopped before presenting the first frame"))?
    }

    pub(crate) fn draw(&mut self, packet: PresentationPacket) -> crate::PlatformFrameResult {
        if !self.first_frame_presented {
            return match self.draw_sync(packet, false) {
                Ok(true) => {
                    self.first_frame_presented = true;
                    crate::PlatformFrameResult::Submitted
                }
                Ok(false) => crate::PlatformFrameResult::Deferred,
                Err(error) => {
                    log::error!("failed to present first Linux frame: {error:#}");
                    crate::PlatformFrameResult::Deferred
                }
            };
        }

        match self.enqueue(Command::Draw {
            packet,
            reply: None,
        }) {
            Ok(()) => crate::PlatformFrameResult::Queued,
            Err(error) => {
                log::error!("failed to queue Linux scene presentation: {error:#}");
                crate::PlatformFrameResult::Deferred
            }
        }
    }

    pub(crate) fn present_framebuffer_only(
        &mut self,
        packet: PresentationPacket,
    ) -> crate::PlatformFrameResult {
        if !self.first_frame_presented {
            return match self.draw_sync(packet, true) {
                Ok(true) => {
                    self.first_frame_presented = true;
                    crate::PlatformFrameResult::Submitted
                }
                Ok(false) => crate::PlatformFrameResult::Deferred,
                Err(error) => {
                    log::error!("failed to present first Linux framebuffer: {error:#}");
                    crate::PlatformFrameResult::Deferred
                }
            };
        }

        match self.enqueue(Command::FramebufferOnly {
            packet,
            reply: None,
        }) {
            Ok(()) => crate::PlatformFrameResult::Queued,
            Err(error) => {
                log::error!("failed to queue Linux framebuffer presentation: {error:#}");
                crate::PlatformFrameResult::Deferred
            }
        }
    }

    pub(crate) fn present_active_frame(
        &self,
        now: Instant,
    ) -> Result<Option<ActivePresentationFrame>> {
        if !self.pending_presentation.load(Ordering::Acquire) {
            return Ok(None);
        }
        self.enqueue(Command::Tick(now))?;
        // The presentation owner sends completions after successful submission and the native
        // frame callback schedules the next paced sample.
        Ok(Some(ActivePresentationFrame {
            continues: true,
            completed_animations: smallvec::SmallVec::new(),
        }))
    }

    pub(crate) fn has_active_presentation_animations(&self) -> bool {
        self.pending_presentation.load(Ordering::Acquire)
    }

    pub(crate) fn set_frame_request_sender(&self, sender: PlatformFrameRequestSender) {
        *self.frame_requests.lock() = Some(sender);
    }

    pub(crate) fn set_animation_completion_sender(&self, sender: SceneAnimationCompletionSender) {
        *self.animation_completions.lock() = Some(sender);
    }

    pub(crate) fn set_frame_interval(&self, interval: Option<std::time::Duration>) {
        if let Err(error) = self.call(move |renderer| renderer.set_frame_interval(interval)) {
            log::error!("failed to update Linux presentation interval: {error:#}");
        }
    }

    pub(crate) fn platform_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }

    pub(crate) fn viewport_size(&self) -> Size<DevicePixels> {
        self.viewport_size
    }

    pub(crate) fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        if let Err(error) = self.call(move |renderer| renderer.update_drawable_size(size)) {
            log::error!("failed to update Linux drawable size: {error:#}");
        } else {
            self.viewport_size = size;
        }
    }

    pub(crate) fn resize(&mut self, size: Size<crate::Pixels>) -> Result<()> {
        self.call(move |renderer| renderer.resize(size))??;
        Ok(())
    }

    pub(crate) fn update_transparency(&self, transparent: bool) {
        if let Err(error) = self.call(move |renderer| renderer.update_transparency(transparent)) {
            log::error!("failed to update Linux renderer transparency: {error:#}");
        }
    }

    pub(crate) fn gpu_specs(&self) -> Result<GpuSpecs> {
        self.call(|renderer| renderer.gpu_specs())
    }

    pub(crate) fn trim_gpui_memory(&self, level: GpuiMemoryTrimLevel) {
        if let Err(error) = self.call(move |renderer| renderer.trim_gpui_memory(level)) {
            log::error!("failed to trim Linux renderer memory: {error:#}");
        }
    }

    pub(crate) fn destroy(&mut self) {
        if let Some(worker) = self.worker.take() {
            {
                let mut state = self.queue.state.lock();
                state.closing = true;
                state.commands.clear();
                state.commands.push_back(Command::Shutdown);
                self.queue.available.notify_one();
            }
            if worker.join().is_err() {
                log::error!("Linux presentation owner thread panicked during shutdown");
            }
        }
    }
}

impl Drop for OwnedNovaRenderer {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn run_renderer(
    renderer: &mut NovaRenderer,
    queue: Arc<SharedQueue>,
    pending_presentation: Arc<AtomicBool>,
    frame_requests: Arc<Mutex<Option<PlatformFrameRequestSender>>>,
    animation_completions: Arc<Mutex<Option<SceneAnimationCompletionSender>>>,
) {
    loop {
        let command = {
            let mut state = queue.state.lock();
            while state.commands.is_empty() {
                queue.available.wait(&mut state);
            }
            state.commands.pop_front().expect("queue is nonempty")
        };

        match command {
            Command::Draw { packet, reply } => {
                let result = renderer.draw(packet);
                report_draw(
                    renderer,
                    &result,
                    &pending_presentation,
                    &frame_requests,
                    &animation_completions,
                );
                if let Some(reply) = reply {
                    if reply.send(result).is_err() {
                        log::trace!("discarding first scene result after its caller dropped");
                    }
                }
            }
            Command::FramebufferOnly { packet, reply } => {
                let result = renderer.present_framebuffer_only(packet);
                report_draw(
                    renderer,
                    &result,
                    &pending_presentation,
                    &frame_requests,
                    &animation_completions,
                );
                if let Some(reply) = reply {
                    if reply.send(result).is_err() {
                        log::trace!("discarding first framebuffer result after its caller dropped");
                    }
                }
            }
            Command::Tick(now) => match renderer.present_active_frame(now, None) {
                Ok(Some(frame)) => {
                    send_completions(frame.completed_animations, &animation_completions);
                    pending_presentation.store(
                        renderer.has_active_presentation_animations(),
                        Ordering::Release,
                    );
                }
                Ok(None) => {
                    let pending = renderer.has_active_presentation_animations();
                    pending_presentation.store(pending, Ordering::Release);
                    if pending {
                        request_frame(&frame_requests, PlatformFrameRequest::presentation());
                    }
                }
                Err(error) => {
                    log::error!("failed to advance Linux scene animation: {error:#}");
                    pending_presentation.store(
                        renderer.has_active_presentation_animations(),
                        Ordering::Release,
                    );
                    request_frame(&frame_requests, PlatformFrameRequest::presentation());
                }
            },
            Command::Call(operation) => operation(renderer),
            Command::Shutdown => {
                renderer.destroy();
                return;
            }
        }
    }
}

fn report_draw(
    renderer: &mut NovaRenderer,
    result: &Result<bool>,
    pending_presentation: &AtomicBool,
    frame_requests: &Mutex<Option<PlatformFrameRequestSender>>,
    animation_completions: &Mutex<Option<SceneAnimationCompletionSender>>,
) {
    let pending = renderer.has_active_presentation_animations();
    pending_presentation.store(pending, Ordering::Release);
    match result {
        Ok(true) => {
            send_completions(renderer.take_animation_completions(), animation_completions);
            if pending {
                request_frame(frame_requests, PlatformFrameRequest::presentation());
            }
        }
        Ok(false) => {
            request_frame(frame_requests, PlatformFrameRequest::presentation());
        }
        Err(error) => {
            log::error!("failed to draw Linux presentation packet: {error:#}");
            request_frame(frame_requests, PlatformFrameRequest::ui_commit());
        }
    }
}

fn request_frame(
    frame_requests: &Mutex<Option<PlatformFrameRequestSender>>,
    request: PlatformFrameRequest,
) {
    let sender = frame_requests.lock().clone();
    if let Some(sender) = sender
        && !sender.request(request)
    {
        log::trace!("discarding presentation request after its UI receiver closed");
    }
}

fn send_completions(
    completions: impl IntoIterator<Item = crate::SceneAnimationCompletion>,
    animation_completions: &Mutex<Option<SceneAnimationCompletionSender>>,
) {
    let sender = animation_completions.lock().clone();
    if let Some(sender) = sender {
        for completion in completions {
            if sender.unbounded_send(completion).is_err() {
                log::trace!("discarding presentation completion after its UI receiver closed");
                break;
            }
        }
    }
}
