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
    },
    Tick(Instant, Option<ActivePresentationTiming>),
    Continue(Instant),
    Resize(Size<DevicePixels>),
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

impl Command {
    fn is_barrier(&self) -> bool {
        match self {
            Self::Resize(_)
            | Self::Transparency(_)
            | Self::Call(_)
            | Self::Shutdown(_)
            | Self::Draw { reply: Some(_), .. } => true,
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
        let mut coalesced_count = 0;
        let mut state = self.0.lock();
        if state.closing {
            return Err(anyhow!("GPU renderer is closing"));
        }
        if matches!(command, Command::Shutdown(_)) {
            state.closing = true;
            state.commands.clear();
        }
        let barrier = state
            .commands
            .iter()
            .rposition(|queued| queued.command.is_barrier())
            .map_or(0, |index| index + 1);
        match &mut command {
            Command::Draw {
                packet,
                reply: None,
                ..
            } => {
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
                    first_enqueued_at = previous.first_enqueued_at;
                    coalesced_count = previous.coalesced_count.saturating_add(1);
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

    #[cfg(not(target_os = "windows"))]
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
