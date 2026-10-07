use super::{
    RenderOwnerFrame, Status,
    queue::{Command, Queue},
};
use crate::{PresentationPacket, platform::NovaRenderer};
use anyhow::Result;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

#[derive(Default)]
pub(super) struct Worker {
    entries: HashMap<u64, Entry>,
    closing: bool,
}

impl Worker {
    pub(super) fn is_closing(&self) -> bool {
        self.closing
    }

    pub(super) fn shutdown(&mut self) {
        self.entries.clear();
        self.closing = true;
    }
    pub(super) fn insert(&mut self, id: u64, entry: Entry) {
        self.entries.insert(id, entry);
    }

    pub(super) fn drain(&mut self, id: u64) {
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        if let Some(command) = entry.queue.take() {
            if let Command::Shutdown(reply) = command {
                self.entries.remove(&id);
                if reply.send(()).is_err() {
                    log::trace!("renderer shutdown caller dropped");
                }
                return;
            }
            entry.execute(command);
        }
        // One command per dispatch lets another window initialize/present even under a steady
        // stream of commits. There is at most one global wake queued for each window.
        if entry.queue.finish_dispatch() {
            match super::sender().and_then(|sender| {
                sender
                    .send(Box::new(move |worker| worker.drain(id)))
                    .map_err(|_| anyhow::anyhow!("GPU owner stopped"))
            }) {
                Ok(()) => {}
                Err(error) => log::error!("failed to reschedule GPU work: {error:#}"),
            }
        }
    }
}

pub(super) struct Entry {
    renderer: NovaRenderer,
    queue: Arc<Queue>,
    status: Arc<Status>,
    _keep_alive: Box<dyn std::any::Any + Send>,
    report: Arc<dyn Fn(RenderOwnerFrame) + Send + Sync>,
    scene: Option<(PresentationPacket, bool)>,
    ready_registration: Arc<AtomicU64>,
    next_ready_registration: u64,
}

impl Entry {
    pub(super) fn new(
        renderer: NovaRenderer,
        queue: Arc<Queue>,
        status: Arc<Status>,
        keep_alive: Box<dyn std::any::Any + Send>,
        report: Arc<dyn Fn(RenderOwnerFrame) + Send + Sync>,
    ) -> Self {
        Self {
            renderer,
            queue,
            status,
            _keep_alive: keep_alive,
            report,
            scene: None,
            ready_registration: Arc::new(AtomicU64::new(0)),
            next_ready_registration: 0,
        }
    }

    fn execute(&mut self, command: Command) {
        match command {
            Command::Draw {
                mut packet,
                framebuffer_only,
                reply,
            } => {
                if let Some((previous, _)) = self.scene.take() {
                    packet.merge_pending_damage_from(&previous);
                }
                self.scene = Some((packet, framebuffer_only));
                let result = self.present_scene();
                self.report(&result, smallvec::SmallVec::new());
                if let Some(reply) = reply
                    && reply.send(result).is_err()
                {
                    log::trace!("first-frame caller dropped");
                }
            }
            Command::Tick(now, timing) => {
                let mut completions = smallvec::SmallVec::new();
                let result = if self.scene.is_some() {
                    self.present_scene()
                } else {
                    (|| {
                        if !self.ready()? {
                            return Ok(false);
                        }
                        let frame_time = Instant::now();
                        if !self.renderer.active_presentation_is_due(frame_time) {
                            return Ok(false);
                        }
                        let timing = timing.map(|mut timing| {
                            timing.window_dispatch_delay +=
                                frame_time.saturating_duration_since(now);
                            timing.frame_started_at = frame_time;
                            timing
                        });
                        if let Some(frame) =
                            self.renderer.present_active_frame(frame_time, timing)?
                        {
                            completions = frame.completed_animations;
                            Ok(true)
                        } else {
                            Ok(false)
                        }
                    })()
                };
                self.report(&result, completions);
            }
            Command::Resize(size) => {
                // Target recreation may cancel the backend's old readiness registration.
                // Old callbacks must not consume a later registration or wake a closed window.
                self.ready_registration.store(0, Ordering::Release);
                self.renderer.update_drawable_size(size);
            }
            Command::Call(operation) => operation(&mut self.renderer),
            Command::Transparency(transparent) => {
                self.ready_registration.store(0, Ordering::Release);
                self.renderer.update_transparency(transparent);
            }
            Command::Shutdown(_) => unreachable!("shutdown is handled by the worker"),
        }
    }

    fn present_scene(&mut self) -> Result<bool> {
        if !self.ready()? {
            return Ok(false);
        }
        let Some((mut packet, framebuffer_only)) = self.scene.take() else {
            return Ok(false);
        };
        // This is the GPU frame's single visual sample, after waiting in the producer queue.
        // UI layout stays immutable; retained timelines are sampled by Nova using this timestamp.
        packet.frame_time = Instant::now();
        if framebuffer_only {
            self.renderer.present_framebuffer_only(packet)
        } else {
            self.renderer.draw(packet)
        }
    }

    fn ready(&mut self) -> Result<bool> {
        if self.renderer.can_present_without_wait()? {
            return Ok(true);
        }
        if !self
            .renderer
            .presentation_capabilities()
            .frame_ready_notification
        {
            return Ok(false);
        }
        if self.ready_registration.load(Ordering::Acquire) == 0 {
            self.next_ready_registration += 1;
            let generation = self.next_ready_registration;
            self.ready_registration.store(generation, Ordering::Release);
            let registration = self.ready_registration.clone();
            let report = self.report.clone();
            let callback = Box::new(move || {
                if registration
                    .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    return;
                }
                report(RenderOwnerFrame {
                    submitted: false,
                    pending: true,
                    failed: false,
                    ready_enqueued_at: Some(Instant::now()),
                    completed_animations: smallvec::SmallVec::new(),
                });
            });
            match self.renderer.arm_swapchain_frame_ready(callback) {
                Ok(true) => {}
                Ok(false) => {
                    self.ready_registration.store(0, Ordering::Release);
                }
                Err(error) => {
                    self.ready_registration.store(0, Ordering::Release);
                    return Err(error);
                }
            }
        }
        Ok(false)
    }

    fn report(
        &mut self,
        result: &Result<bool>,
        mut completed_animations: smallvec::SmallVec<[crate::SceneAnimationCompletion; 4]>,
    ) {
        let submitted = result.as_ref().is_ok_and(|submitted| *submitted);
        if submitted {
            self.ready_registration.store(0, Ordering::Release);
            self.status.submitted.store(true, Ordering::Release);
        }
        let pending = self.scene.is_some()
            || self.renderer.has_active_presentation_animations()
            || self.queue.has_presentation();
        self.status.pending.store(pending, Ordering::Release);
        if let Err(error) = result {
            log::error!("GPU owner presentation failed: {error:#}");
        }
        if submitted {
            completed_animations.extend(self.renderer.take_animation_completions());
        }
        (self.report)(RenderOwnerFrame {
            submitted,
            pending,
            failed: result.is_err(),
            ready_enqueued_at: None,
            completed_animations,
        });
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.queue.close();
        self.ready_registration.store(0, Ordering::Release);
        self.renderer.destroy();
    }
}
