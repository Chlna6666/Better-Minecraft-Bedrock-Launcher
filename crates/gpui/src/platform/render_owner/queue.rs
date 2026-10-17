#[cfg(any(test, not(target_os = "windows")))]
use std::time::Duration;
use std::{collections::VecDeque, sync::mpsc::SyncSender, time::Instant};

use crate::{
    DevicePixels, PresentationPacket, Size,
    platform::{NovaRenderer, frame::ActivePresentationTiming},
};
use anyhow::{Result, anyhow};
use parking_lot::Mutex;

#[cfg(test)]
mod tests;

pub(super) enum Command {
    Draw {
        packet: PresentationPacket,
        framebuffer_only: bool,
        reply: Option<SyncSender<Result<bool>>>,
        resize: Option<DrawableResize>,
    },
    Tick(Instant, Option<ActivePresentationTiming>),
    Continue(Instant),
    Resize {
        size: Size<DevicePixels>,
        /// Native client-geometry generation, or `None` for an unversioned resize.
        epoch: Option<u64>,
    },
    #[cfg(any(test, target_os = "windows"))]
    ContentStretch(Size<DevicePixels>),
    Transparency(bool),
    #[cfg(any(test, not(target_os = "windows")))]
    PresentationInterval(Option<Duration>),
    #[cfg(not(target_os = "windows"))]
    PresentationClock(std::sync::Arc<dyn Fn() + Send + Sync>),
    #[cfg(not(target_os = "windows"))]
    PresentationVisibility(bool),
    Call(Box<dyn FnOnce(&mut NovaRenderer) + Send>),
    Shutdown(SyncSender<()>),
}

#[derive(Clone, Copy)]
pub(super) struct DrawableResize {
    pub(super) size: Size<DevicePixels>,
    pub(super) epoch: Option<u64>,
}

impl Command {
    #[cfg(test)]
    fn resize(size: Size<DevicePixels>) -> Self {
        Self::Resize { size, epoch: None }
    }

    /// Hard ordering boundaries that a newer exact-size draw must never
    /// overtake while compacting obsolete *unexecuted* native resize frames.
    #[cfg(any(test, target_os = "windows"))]
    fn is_resize_compaction_barrier(&self) -> bool {
        if matches!(
            self,
            Self::Draw {
                resize: Some(_),
                ..
            }
        ) {
            return true;
        }
        !matches!(
            self,
            Self::Resize { .. }
                | Self::Draw { reply: None, .. }
                | Self::Tick(..)
                | Self::Continue(..)
                | Self::ContentStretch(..)
        )
    }

    #[cfg(any(test, target_os = "windows"))]
    fn is_live_resize_compaction_barrier(&self) -> bool {
        match self {
            Self::Draw { reply: Some(_), .. }
            | Self::Transparency(_)
            | Self::Call(_)
            | Self::Shutdown(_) => true,
            #[cfg(any(test, not(target_os = "windows")))]
            Self::PresentationInterval(_) => true,
            #[cfg(not(target_os = "windows"))]
            Self::PresentationClock(_) | Self::PresentationVisibility(_) => true,
            _ => false,
        }
    }

    fn is_barrier(&self) -> bool {
        match self {
            Self::Resize { .. }
            | Self::Transparency(_)
            | Self::Call(_)
            | Self::Shutdown(_)
            | Self::Draw { reply: Some(_), .. }
            | Self::Draw {
                resize: Some(_), ..
            } => true,
            #[cfg(any(test, not(target_os = "windows")))]
            Self::PresentationInterval(_) => true,
            #[cfg(not(target_os = "windows"))]
            Self::PresentationClock(_) => true,
            #[cfg(not(target_os = "windows"))]
            Self::PresentationVisibility(_) => true,
            _ => false,
        }
    }
}

#[derive(Default)]
struct State {
    commands: VecDeque<QueuedCommand>,
    scheduled: bool,
    closing: bool,
}

#[derive(Default)]
pub(super) struct Queue(Mutex<State>);

pub(super) struct QueuedCommand {
    pub(super) command: Command,
    pub(super) enqueued_at: Instant,
    pub(super) first_enqueued_at: Instant,
    pub(super) coalesced_count: u64,
}

impl Queue {
    pub(super) fn enqueue(
        &self,
        command: Command,
        wake: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        self.enqueue_at(command, wake, Instant::now())
    }

    fn enqueue_at(
        &self,
        mut command: Command,
        wake: impl FnOnce() -> Result<()>,
        enqueued_at: Instant,
    ) -> Result<()> {
        // Profiling includes producer lock contention, while replacement keeps backlog age.
        let mut first_enqueued_at = enqueued_at;
        let mut coalesced_count = 0_u64;
        let mut state = self.0.lock();
        if state.closing {
            return Err(anyhow!("GPU renderer is closing"));
        }
        if matches!(command, Command::Shutdown(_)) {
            state.closing = true;
            state.commands.clear();
        }
        // A live resize frame owns both its geometry and the Scene laid out for
        // that geometry. Replace queued presentation work as one transaction so
        // a newer WM_SIZE cannot be separated from its matching Scene.
        #[cfg(any(test, target_os = "windows"))]
        if let Command::Draw {
            packet,
            reply: None,
            resize: Some(_),
            ..
        } = &mut command
        {
            let start = state
                .commands
                .iter()
                .rposition(|queued| queued.command.is_live_resize_compaction_barrier())
                .map_or(0, |index| index + 1);
            for obsolete in state.commands.drain(start..) {
                first_enqueued_at = first_enqueued_at.min(obsolete.first_enqueued_at);
                coalesced_count =
                    coalesced_count.saturating_add(obsolete.coalesced_count.saturating_add(1));
                match obsolete.command {
                    Command::Resize { .. }
                    | Command::Tick(..)
                    | Command::Continue(..)
                    | Command::ContentStretch(..) => {}
                    Command::Draw {
                        packet: previous,
                        reply: None,
                        ..
                    } => packet.merge_pending_damage_from(&previous),
                    _ => unreachable!("live resize compaction must not cross a control barrier"),
                }
            }
        }
        // Several WM_SIZE generations can arrive while the shared GPU owner
        // is busy. Each obsolete Resize + Draw used to be a barrier, forcing
        // costly intermediate swapchain/blur/depth rebuilds on every backend.
        // Once a NEW Draw is available, only its newest queued extent and
        // complete accumulated damage need to reach the GPU. Never cross a
        // resource-control command or a synchronous first-frame handshake.
        #[cfg(any(test, target_os = "windows"))]
        if let Command::Draw {
            packet,
            reply: None,
            ..
        } = &mut command
        {
            let start = state
                .commands
                .iter()
                .rposition(|queued| queued.command.is_resize_compaction_barrier())
                .map_or(0, |index| index + 1);
            let resize_count = state
                .commands
                .iter()
                .skip(start)
                .filter(|queued| matches!(queued.command, Command::Resize { .. }))
                .count();
            if resize_count >= 2 {
                let mut newest_resize = None;
                for obsolete in state.commands.drain(start..) {
                    first_enqueued_at = first_enqueued_at.min(obsolete.first_enqueued_at);
                    coalesced_count =
                        coalesced_count.saturating_add(obsolete.coalesced_count.saturating_add(1));
                    match obsolete.command {
                        Command::Resize { size, epoch } => newest_resize = Some((size, epoch)),
                        Command::Draw {
                            packet: previous, ..
                        } => {
                            packet.merge_pending_damage_from(&previous);
                        }
                        Command::Tick(..)
                        | Command::Continue(..)
                        | Command::ContentStretch(..) => {}
                        _ => unreachable!("resize compaction must not cross a control barrier"),
                    }
                }
                let (size, epoch) = newest_resize.expect("at least two pending resizes");
                state.commands.push_back(QueuedCommand {
                    command: Command::Resize { size, epoch },
                    enqueued_at,
                    first_enqueued_at,
                    coalesced_count: 0,
                });
            }
        }
        let barrier = state
            .commands
            .iter()
            .rposition(|queued| queued.command.is_barrier())
            .map_or(0, |index| index + 1);
        match &mut command {
            Command::Resize { .. } => {
                // Collapse adjacent unconsumed resizes only. Never cross a Draw,
                // fence-bearing Call, or resource ownership barrier.
                if state
                    .commands
                    .back()
                    .is_some_and(|last| matches!(last.command, Command::Resize { .. }))
                {
                    let previous = state
                        .commands
                        .pop_back()
                        .expect("the previous command was checked");
                    first_enqueued_at = first_enqueued_at.min(previous.first_enqueued_at);
                    coalesced_count = previous.coalesced_count.saturating_add(1);
                }
            }
            #[cfg(any(test, target_os = "windows"))]
            Command::ContentStretch(_) => {
                // Resize bursts keep only the latest temporary compositor scale.
                if state
                    .commands
                    .back()
                    .is_some_and(|last| matches!(last.command, Command::ContentStretch(_)))
                {
                    let previous = state
                        .commands
                        .pop_back()
                        .expect("the previous command was checked");
                    first_enqueued_at = first_enqueued_at.min(previous.first_enqueued_at);
                    coalesced_count = previous.coalesced_count.saturating_add(1);
                }
            }
            Command::Draw {
                packet,
                reply: None,
                ..
            } => {
                // A newly committed immutable Scene subsumes older presentation
                // ticks in the same barrier-free segment. The Draw samples the
                // current visual timeline on the GPU owner; running an old Tick
                // first would render obsolete pixels only to replace them.
                // Never absorb a Tick across Resize, Call, or a first-frame
                // synchronous Draw barrier.
                for index in (barrier..state.commands.len()).rev() {
                    if !matches!(
                        state.commands[index].command,
                        Command::Tick(..) | Command::Continue(..)
                    ) {
                        continue;
                    }
                    let obsolete = state
                        .commands
                        .remove(index)
                        .expect("index was validated against the queue");
                    first_enqueued_at = first_enqueued_at.min(obsolete.first_enqueued_at);
                    coalesced_count =
                        coalesced_count.saturating_add(obsolete.coalesced_count.saturating_add(1));
                }
                if let Some(index) = (barrier..state.commands.len()).rev().find(|index| {
                    matches!(
                        state.commands[*index].command,
                        Command::Draw { reply: None, .. }
                    )
                }) {
                    let previous = state
                        .commands
                        .remove(index)
                        .expect("index was obtained from the queue");
                    first_enqueued_at = first_enqueued_at.min(previous.first_enqueued_at);
                    coalesced_count =
                        coalesced_count.saturating_add(previous.coalesced_count.saturating_add(1));
                    let Command::Draw {
                        packet: previous, ..
                    } = previous.command
                    else {
                        unreachable!()
                    };
                    packet.merge_pending_damage_from(&previous);
                }
            }
            Command::Tick(..) | Command::Continue(..) => {
                if let Some(index) = (barrier..state.commands.len()).rev().find(|index| {
                    matches!(
                        state.commands[*index].command,
                        Command::Tick(..) | Command::Continue(..)
                    )
                }) {
                    let previous = state
                        .commands
                        .remove(index)
                        .expect("index was obtained from the queue");
                    first_enqueued_at = previous.first_enqueued_at;
                    coalesced_count = previous.coalesced_count.saturating_add(1);
                }
            }
            _ => {}
        }
        state.commands.push_back(QueuedCommand {
            command,
            enqueued_at,
            first_enqueued_at,
            coalesced_count,
        });
        if !state.scheduled {
            wake()?;
            state.scheduled = true;
        }
        Ok(())
    }

    pub(super) fn take_timed(&self) -> Option<QueuedCommand> {
        self.0.lock().commands.pop_front()
    }

    #[cfg(test)]
    pub(super) fn take(&self) -> Option<Command> {
        self.take_timed().map(|queued| queued.command)
    }

    pub(super) fn finish_dispatch(&self) -> bool {
        let mut state = self.0.lock();
        if state.commands.is_empty() {
            state.scheduled = false;
            false
        } else {
            true
        }
    }

    pub(super) fn has_presentation(&self) -> bool {
        self.0.lock().commands.iter().any(|command| {
            matches!(
                command.command,
                Command::Draw { .. } | Command::Tick(..) | Command::Continue(..)
            )
        })
    }

    /// A committed scene subsumes optional ticks before resource-control barriers.
    pub(super) fn has_pending_draw(&self) -> bool {
        for queued in &self.0.lock().commands {
            match &queued.command {
                Command::Draw { .. } => return true,
                Command::Resize { .. } => {}
                command if command.is_barrier() => break,
                _ => {}
            }
        }
        false
    }

    pub(super) fn has_pending_resize_draw(&self) -> bool {
        self.0.lock().commands.iter().any(|queued| {
            matches!(
                queued.command,
                Command::Draw {
                    resize: Some(_),
                    reply: None,
                    ..
                }
            )
        })
    }

    pub(super) fn has_commands(&self) -> bool {
        !self.0.lock().commands.is_empty()
    }

    pub(super) fn close(&self) {
        let mut state = self.0.lock();
        state.closing = true;
        state.commands.clear();
    }

    pub(super) fn is_closed(&self) -> bool {
        self.0.lock().closing
    }
}
