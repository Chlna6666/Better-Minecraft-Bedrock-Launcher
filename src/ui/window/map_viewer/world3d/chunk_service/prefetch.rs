//! Waits for prefetch results while allowing view changes and shutdown to cancel the batch.

use super::{ChunkPos, ChunkServiceCommand, LoadedChunkBatch};
use bedrock_world::surface::CancelFlag;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;

pub(super) struct PendingChunkBatch {
    pub(super) receiver: Receiver<(LoadedChunkBatch, Duration)>,
    pub(super) cancel: CancelFlag,
    pub(super) positions: Vec<ChunkPos>,
}

pub(super) enum BatchWait {
    Loaded(LoadedChunkBatch, Duration),
    Command(ChunkServiceCommand),
    Disconnected,
}

impl PendingChunkBatch {
    pub(super) fn wait(&self, commands: &Receiver<ChunkServiceCommand>) -> BatchWait {
        loop {
            match commands.try_recv() {
                Ok(command) => return BatchWait::Command(command),
                Err(TryRecvError::Disconnected) => {
                    return BatchWait::Command(ChunkServiceCommand::Shutdown);
                }
                Err(TryRecvError::Empty) => {}
            }
            match self.receiver.recv_timeout(Duration::from_millis(10)) {
                Ok((loaded, elapsed)) => return BatchWait::Loaded(loaded, elapsed),
                Err(RecvTimeoutError::Disconnected) => return BatchWait::Disconnected,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

impl Drop for PendingChunkBatch {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, sync_channel};

    #[test]
    fn pending_batch_yields_to_commands_before_loading_finishes() {
        let (_result_tx, receiver) = sync_channel(1);
        let (command_tx, commands) = channel();
        let cancel = CancelFlag::new();
        let pending = PendingChunkBatch {
            receiver,
            cancel: cancel.clone(),
            positions: Vec::new(),
        };
        let (started_tx, started_rx) = channel();
        let (done_tx, done_rx) = channel();
        let waiter = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let interrupted = matches!(
                pending.wait(&commands),
                BatchWait::Command(ChunkServiceCommand::Shutdown)
            );
            drop(pending);
            done_tx.send(interrupted).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        command_tx.send(ChunkServiceCommand::Shutdown).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        waiter.join().unwrap();
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn finished_batch_is_returned_when_no_command_is_pending() {
        let (result_tx, receiver) = sync_channel(1);
        let (_command_tx, commands) = channel();
        result_tx
            .send((
                LoadedChunkBatch {
                    chunks: Vec::new(),
                    stats: Default::default(),
                    query_time: Duration::ZERO,
                    collect_time: Duration::ZERO,
                },
                Duration::from_millis(7),
            ))
            .unwrap();
        let pending = PendingChunkBatch {
            receiver,
            cancel: CancelFlag::new(),
            positions: Vec::new(),
        };
        assert!(
            matches!(pending.wait(&commands), BatchWait::Loaded(_, elapsed) if elapsed == Duration::from_millis(7))
        );
    }
}
