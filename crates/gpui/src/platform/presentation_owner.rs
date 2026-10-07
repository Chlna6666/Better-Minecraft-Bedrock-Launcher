//! Native frame/completion bridge to the shared, backend-neutral GPU owner.
//! Wayland/X11 protocol objects remain on their native thread.
use super::{
    NovaRenderer, PlatformAtlas,
    frame::PlatformFrameRequestSender,
    render_owner::{RenderOwner, RenderOwnerFrame},
};
use crate::{
    ActivePresentationFrame, DevicePixels, GpuSpecs, GpuiMemoryTrimLevel, PlatformFrameRequest,
    PlatformFrameResult, PresentationPacket, SceneAnimationCompletionSender, Size,
};
use anyhow::Result;
use parking_lot::Mutex;
use std::{sync::Arc, time::Instant};

pub(crate) struct OwnedNovaRenderer {
    owner: Option<RenderOwner>,
    frame_requests: Arc<Mutex<Option<PlatformFrameRequestSender>>>,
    animation_completions: Arc<Mutex<Option<SceneAnimationCompletionSender>>>,
}

impl OwnedNovaRenderer {
    pub(crate) fn new(
        create: impl FnOnce() -> Result<NovaRenderer> + Send + 'static,
    ) -> Result<Self> {
        let frame_requests = Arc::new(Mutex::new(None::<PlatformFrameRequestSender>));
        let animation_completions = Arc::new(Mutex::new(None::<SceneAnimationCompletionSender>));
        let requests = frame_requests.clone();
        let completions = animation_completions.clone();
        let report = Arc::new(move |frame: RenderOwnerFrame| {
            if frame.submitted {
                if let Some(sender) = completions.lock().clone() {
                    for completion in frame.completed_animations {
                        if sender.unbounded_send(completion).is_err() {
                            log::trace!("GPU animation completion receiver closed");
                            break;
                        }
                    }
                }
            }
            if frame.pending || frame.failed {
                if let Some(sender) = requests.lock().clone()
                    && !sender.request(if frame.failed {
                        PlatformFrameRequest::ui_commit()
                    } else {
                        PlatformFrameRequest::presentation()
                    })
                {
                    log::trace!("GPU frame request receiver closed");
                }
            }
        });
        let owner = RenderOwner::new(create, report)?;
        Ok(Self {
            owner: Some(owner),
            frame_requests,
            animation_completions,
        })
    }

    fn owner(&self) -> &RenderOwner {
        self.owner
            .as_ref()
            .expect("native window does not render after destroying its GPU owner")
    }

    pub(crate) fn draw(&mut self, packet: PresentationPacket) -> PlatformFrameResult {
        self.submitted(self.owner().draw(packet))
    }

    pub(crate) fn present_framebuffer_only(
        &mut self,
        packet: PresentationPacket,
    ) -> PlatformFrameResult {
        self.submitted(self.owner().present_framebuffer_only(packet))
    }

    fn submitted(&self, result: Result<bool>) -> PlatformFrameResult {
        match result {
            Ok(true) => PlatformFrameResult::Submitted,
            Ok(false) if self.owner().has_submitted_frame() => PlatformFrameResult::Queued,
            Ok(false) => PlatformFrameResult::Deferred,
            Err(error) => {
                log::error!("failed to submit GPU presentation: {error:#}");
                PlatformFrameResult::Deferred
            }
        }
    }

    pub(crate) fn present_active_frame(
        &self,
        now: Instant,
    ) -> Result<Option<ActivePresentationFrame>> {
        self.owner().present_active_frame(now, None)
    }

    pub(crate) fn has_active_presentation_animations(&self) -> bool {
        self.owner().has_active_presentation_animations()
    }

    pub(crate) fn set_frame_request_sender(&self, sender: PlatformFrameRequestSender) {
        *self.frame_requests.lock() = Some(sender);
    }

    pub(crate) fn set_animation_completion_sender(&self, sender: SceneAnimationCompletionSender) {
        *self.animation_completions.lock() = Some(sender);
    }

    pub(crate) fn set_frame_interval(&self, interval: Option<std::time::Duration>) {
        self.owner().set_frame_interval(interval);
    }

    pub(crate) fn platform_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.owner().platform_atlas()
    }

    pub(crate) fn viewport_size(&self) -> Size<DevicePixels> {
        self.owner().viewport_size()
    }

    pub(crate) fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        if let Err(error) = self.resize(size) {
            log::error!("failed to queue GPU resize: {error:#}");
        }
    }

    /// Queues the new drawable extent; GPU target recreation is deferred to presentation.
    pub(crate) fn resize(&mut self, size: Size<DevicePixels>) -> Result<()> {
        self.owner
            .as_mut()
            .expect("live GPU owner")
            .update_drawable_size(size)
    }

    pub(crate) fn update_transparency(&self, transparent: bool) {
        self.owner().update_transparency(transparent);
    }

    pub(crate) fn gpu_specs(&self) -> Result<GpuSpecs> {
        Ok(self.owner().gpu_specs())
    }

    pub(crate) fn trim_gpui_memory(&self, level: GpuiMemoryTrimLevel) {
        self.owner().trim_gpui_memory(level);
    }

    pub(crate) fn destroy(&mut self) {
        // RenderOwner waits for its destruction barrier before the native surface is released.
        self.owner.take();
    }
}
