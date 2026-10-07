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
    Resize(Size<DevicePixels>),
    Transparency(bool),
    Call(Box<dyn FnOnce(&mut NovaRenderer) + Send>),
    Shutdown(SyncSender<()>),
}

impl Command {
    fn is_barrier(&self) -> bool {
        matches!(
            self,
            Self::Resize(_)
                | Self::Transparency(_)
                | Self::Call(_)
                | Self::Shutdown(_)
                | Self::Draw { reply: Some(_), .. }
        )
    }
}

#[derive(Default)]
struct State {
    commands: VecDeque<Command>,
    scheduled: bool,
    closing: bool,
}

#[derive(Default)]
pub(super) struct Queue(Mutex<State>);

impl Queue {
    pub(super) fn enqueue(
        &self,
        mut command: Command,
        wake: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
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
            .rposition(Command::is_barrier)
            .map_or(0, |index| index + 1);
        match &mut command {
            Command::Draw {
                packet,
                reply: None,
                ..
            } => {
                if let Some(index) = (barrier..state.commands.len()).rev().find(|index| {
                    matches!(state.commands[*index], Command::Draw { reply: None, .. })
                }) {
                    let Command::Draw {
                        packet: previous, ..
                    } = state
                        .commands
                        .remove(index)
                        .expect("index was obtained from the queue")
                    else {
                        unreachable!()
                    };
                    packet.merge_pending_damage_from(&previous);
                }
            }
            Command::Tick(..) => {
                if let Some(index) = (barrier..state.commands.len())
                    .rev()
                    .find(|index| matches!(state.commands[*index], Command::Tick(..)))
                {
                    state.commands.remove(index);
                }
            }
            _ => {}
        }
        state.commands.push_back(command);
        if !state.scheduled {
            wake()?;
            state.scheduled = true;
        }
        Ok(())
    }

    pub(super) fn take(&self) -> Option<Command> {
        self.0.lock().commands.pop_front()
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
        self.0
            .lock()
            .commands
            .iter()
            .any(|command| matches!(command, Command::Draw { .. } | Command::Tick(..)))
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
