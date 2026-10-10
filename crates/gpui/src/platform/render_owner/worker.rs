use super::{
    RenderOwnerFrame, Status,
    queue::{Command, Queue},
    schedule::Schedule,
};
use crate::diagnostics::gpu_owner::{self, GpuOwnerJobKind, GpuOwnerJobOutcome, GpuOwnerJobSample};
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
        let mut entry = entry;
        entry.id = id;
        gpu_owner::register(id);
        self.entries.insert(id, entry);
    }

    #[cfg(not(target_os = "windows"))]
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.entries
            .values()
            .filter_map(|entry| {
                entry
                    .schedule
                    .deadline()
                    .filter(|_| !entry.queue.has_commands())
            })
            .min()
    }

    #[cfg(not(target_os = "windows"))]
    pub(super) fn present_due(&mut self) {
        let now = Instant::now();
        let due = self
            .entries
            .iter()
            .filter_map(|(id, entry)| {
                entry
                    .schedule
                    .deadline()
                    .filter(|deadline| *deadline <= now && !entry.queue.has_commands())
                    .map(|deadline| (*id, deadline))
            })
            .min_by_key(|(_, deadline)| *deadline);
        // Check the command channel between windows, so overdue animations cannot delay a
        // resize/shutdown barrier behind a burst of submissions for every window.
        if let Some((id, deadline)) = due
            && let Some(entry) = self.entries.get_mut(&id)
        {
            let _dispatch = gpu_owner::Dispatch::start();
            entry.execute_timed(Command::Continue(now), None, Some(deadline));
        }
    }

    pub(super) fn drain(&mut self, id: u64) {
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        if let Some(queued) = entry.queue.take_timed() {
            if let Command::Shutdown(reply) = queued.command {
                self.entries.remove(&id);
                if reply.send(()).is_err() {
                    log::trace!("renderer shutdown caller dropped");
                }
                return;
            }
            entry.execute_timed(
                queued.command,
                Some((
                    queued.enqueued_at,
                    queued.first_enqueued_at,
                    queued.coalesced_count,
                )),
                None,
            );
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
    id: u64,
    renderer: NovaRenderer,
    queue: Arc<Queue>,
    status: Arc<Status>,
    _keep_alive: Box<dyn std::any::Any + Send>,
    report: Arc<dyn Fn(RenderOwnerFrame) + Send + Sync>,
    scene: Option<(PresentationPacket, bool)>,
    ready_registration: Arc<AtomicU64>,
    next_ready_registration: u64,
    schedule: Schedule,
    job_outcome: GpuOwnerJobOutcome,
    #[cfg(not(target_os = "windows"))]
    presentation_clock: Option<Arc<dyn Fn() + Send + Sync>>,
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
            id: 0,
            renderer,
            queue,
            status,
            _keep_alive: keep_alive,
            report,
            scene: None,
            ready_registration: Arc::new(AtomicU64::new(0)),
            next_ready_registration: 0,
            schedule: Schedule::default(),
            job_outcome: GpuOwnerJobOutcome::Control,
            #[cfg(not(target_os = "windows"))]
            presentation_clock: None,
        }
    }

    fn execute_timed(
        &mut self,
        command: Command,
        queued: Option<(Instant, Instant, u64)>,
        deadline: Option<Instant>,
    ) {
        if let Command::Draw { packet, .. } = &command {
            gpu_owner::bind_window(self.id, packet.window_id);
        }
        let kind = match &command {
            Command::Draw { .. } => GpuOwnerJobKind::Draw,
            Command::Tick(..) => GpuOwnerJobKind::Tick,
            Command::Continue(..) => GpuOwnerJobKind::Continue,
            Command::Resize(..) => GpuOwnerJobKind::Resize,
            _ => GpuOwnerJobKind::Control,
        };
        let started_at = Instant::now(); // Command service time, not visual animation time.
        let previous_wait = gpu_owner::blocking_wait();
        self.job_outcome = GpuOwnerJobOutcome::Control;
        self.execute(command);
        let completed_at = Instant::now();
        gpu_owner::record_job(
            GpuOwnerJobSample {
                owner_id: self.id,
                window_id: None,
                job_id: 0,
                kind,
                outcome: if matches!(kind, GpuOwnerJobKind::Control | GpuOwnerJobKind::Resize) {
                    GpuOwnerJobOutcome::Control
                } else {
                    self.job_outcome
                },
                started_at_us: 0,
                completed_at_us: 0,
                queue_wait_us: queued.map(|(time, _, _)| {
                    gpu_owner::micros(started_at.saturating_duration_since(time))
                }),
                pending_age_us: queued.map(|(_, time, _)| {
                    gpu_owner::micros(started_at.saturating_duration_since(time))
                }),
                coalesced_count: queued.map_or(0, |(_, _, count)| count),
                owner_job_duration_us: gpu_owner::micros(
                    completed_at.saturating_duration_since(started_at),
                ),
                owner_blocking_wait_us: gpu_owner::micros(
                    gpu_owner::blocking_wait().saturating_sub(previous_wait),
                ),
                schedule_lateness_us: deadline
                    .map(|time| gpu_owner::micros(started_at.saturating_duration_since(time))),
            },
            started_at,
            completed_at,
        );
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
            Command::Tick(now, timing) => self.tick(now, timing),
            Command::Continue(now) => {
                if self.schedule.is_enabled() {
                    self.tick(now, None);
                }
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
            #[cfg(any(test, not(target_os = "windows")))]
            Command::PresentationInterval(interval) => {
                self.ready_registration.store(0, Ordering::Release);
                self.schedule.set_interval(interval);
                self.report(&Ok(false), smallvec::SmallVec::new());
            }
            #[cfg(not(target_os = "windows"))]
            Command::PresentationClock(callback) => {
                self.presentation_clock = Some(callback);
                self.schedule.set_native_callbacks();
            }
            #[cfg(not(target_os = "windows"))]
            Command::PresentationVisibility(visible) => {
                self.ready_registration.store(0, Ordering::Release);
                self.schedule.set_native_visible(visible);
            }
            Command::Shutdown(_) => unreachable!("shutdown is handled by the worker"),
        }
    }

    fn tick(
        &mut self,
        now: Instant,
        timing: Option<crate::platform::frame::ActivePresentationTiming>,
    ) {
        #[cfg(not(target_os = "windows"))]
        if self.presentation_clock.is_some() && !self.schedule.is_enabled() {
            return;
        }
        let mut completions = smallvec::SmallVec::new();
        let result = if self.scene.is_some() {
            self.present_scene()
        } else {
            (|| {
                if !self.ready()? {
                    return Ok(false);
                }
                let eligibility_time = Instant::now();
                if !self.renderer.active_presentation_is_due(eligibility_time) {
                    return Ok(false);
                }
                #[cfg(not(target_os = "windows"))]
                if let Some(callback) = &self.presentation_clock {
                    callback();
                }
                // One visual timestamp after native arming, shared by sampling, damage and upload.
                #[cfg(not(target_os = "windows"))]
                let frame_time = Instant::now();
                #[cfg(target_os = "windows")]
                let frame_time = eligibility_time;
                let timing = timing.map(|mut timing| {
                    timing.window_dispatch_delay += frame_time.saturating_duration_since(now);
                    timing.frame_started_at = frame_time;
                    timing
                });
                if let Some(frame) = self.renderer.present_active_frame(frame_time, timing)? {
                    completions = frame.completed_animations;
                    Ok(true)
                } else {
                    Ok(false)
                }
            })()
        };
        self.report(&result, completions);
    }

    #[cfg(any(test, not(target_os = "windows")))]
    fn update_schedule(&mut self, retry: bool, submitted: bool) {
        let pending = self.scene.is_some() || self.renderer.has_active_presentation_animations();
        self.schedule.after_frame(
            Instant::now(),
            pending && retry,
            self.ready_registration.load(Ordering::Acquire) != 0,
            self.renderer.presentation_deadline(),
            submitted,
        );
    }

    fn present_scene(&mut self) -> Result<bool> {
        if !self.ready()? {
            return Ok(false);
        }
        let Some((mut packet, framebuffer_only)) = self.scene.take() else {
            return Ok(false);
        };
        #[cfg(not(target_os = "windows"))]
        if let Some(callback) = &self.presentation_clock {
            callback();
        }
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
            let autonomous = self.schedule.is_enabled();
            let queue = self.queue.clone();
            let id = self.id;
            let callback = Box::new(move || {
                if registration
                    .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    return;
                }
                if autonomous {
                    let result = queue.enqueue(Command::Continue(Instant::now()), || {
                        super::sender()?
                            .send(Box::new(move |worker| worker.drain(id)))
                            .map_err(|_| anyhow::anyhow!("GPU owner stopped"))
                    });
                    if let Err(error) = result {
                        log::trace!("discarding GPU readiness after renderer shutdown: {error:#}");
                    }
                    return;
                }
                report(RenderOwnerFrame {
                    submitted: false,
                    pending: true,
                    #[cfg(not(target_os = "windows"))]
                    autonomous: false,
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
        self.job_outcome = match result {
            Ok(true) => GpuOwnerJobOutcome::Submitted,
            Ok(false) => GpuOwnerJobOutcome::Deferred,
            Err(_) => GpuOwnerJobOutcome::Failed,
        };
        let submitted = result.as_ref().is_ok_and(|submitted| *submitted);
        if submitted {
            self.ready_registration.store(0, Ordering::Release);
            self.status.submitted.store(true, Ordering::Release);
        }
        let pending = self.scene.is_some()
            || self.renderer.has_active_presentation_animations()
            || self.queue.has_presentation();
        self.status.pending.store(pending, Ordering::Release);
        #[cfg(any(test, not(target_os = "windows")))]
        self.update_schedule(result.is_ok(), submitted);
        if let Err(error) = result {
            log::error!("GPU owner presentation failed: {error:#}");
        }
        if submitted {
            completed_animations.extend(self.renderer.take_animation_completions());
        }
        (self.report)(RenderOwnerFrame {
            submitted,
            pending,
            #[cfg(not(target_os = "windows"))]
            autonomous: self.schedule.is_enabled() || self.presentation_clock.is_some(),
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
        gpu_owner::unregister(self.id);
    }
}
