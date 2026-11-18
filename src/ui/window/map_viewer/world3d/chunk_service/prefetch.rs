//! Waits for prefetch results while allowing view changes and shutdown to cancel the batch.

use super::{
    ChunkCursor, ChunkPos, ChunkServiceCommand, LoadedChunkBatch, TERRAIN_REFERENCE_Y, ViewState,
    inside_view,
};
use bedrock_world::chunk::{ChunkVersion, SubChunk};
use bedrock_world::surface::{
    CancelFlag, ChunkDataRequest, ChunkLoadOptions, ReadSubchunks, WorldThreadingOptions,
};
use bedrock_world::world::World;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, sync_channel};
use std::time::Duration;

pub(super) const IDLE_READ_INTERVAL: Duration = Duration::from_millis(500);
const IDLE_READ_COLUMNS: usize = 2;
const IDLE_READ_MARGIN: i32 = 2;
const IDLE_READ_LIMIT: usize = 24;
const IDLE_READ_BYTES: usize = 8 * 1024 * 1024;

fn read_subchunks_bytes(data: &ReadSubchunks) -> usize {
    data.present.values().map(SubChunk::estimated_cpu_bytes).sum::<usize>()
        + data.requested.capacity()
        + std::mem::size_of::<ReadSubchunks>()
}

struct IdleRead {
    result: Receiver<Vec<(ChunkPos, ReadSubchunks)>>,
    cancel: CancelFlag,
}

impl Drop for IdleRead {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Staging for packed subchunks only. No model expansion, scene or GPU
/// handles are retained here. A new foreground view can cancel the read.
#[derive(Default)]
pub(super) struct IdleChunkReads {
    ready: BTreeMap<ChunkPos, ReadSubchunks>,
    read: Option<IdleRead>,
    cursor: ChunkCursor,
    attempts: usize,
    bytes: usize,
}

impl IdleChunkReads {
    pub(super) fn active(&self) -> bool {
        self.read.is_some() || self.attempts < IDLE_READ_LIMIT
    }

    pub(super) fn has_ready(&self) -> bool {
        !self.ready.is_empty()
    }

    pub(super) fn take(&mut self, pos: ChunkPos) -> Option<ReadSubchunks> {
        let result = self.ready.remove(&pos)?;
        self.bytes -= read_subchunks_bytes(&result);
        Some(result)
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    fn harvest(&mut self) {
        let Some(read) = &self.read else {
            return;
        };
        match read.result.try_recv() {
            Ok(results) => {
                self.read = None;
                for (pos, data) in results {
                    let bytes = read_subchunks_bytes(&data);
                    if self.bytes.saturating_add(bytes) <= IDLE_READ_BYTES {
                        self.bytes += bytes;
                        self.ready.insert(pos, data);
                    }
                }
            }
            Err(TryRecvError::Disconnected) => self.read = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    pub(super) fn pause(&mut self) {
        self.harvest();
        self.read = None;
    }

    pub(super) fn update_view(&mut self, view: ViewState, center: (i32, i32)) {
        self.cursor = ChunkCursor { ring: view.radius / 2, ..ChunkCursor::default() };
        self.attempts = 0;
        self.ready.retain(|&pos, _| {
            inside_view(pos, view.dimension, center.0, center.1, view.radius + IDLE_READ_MARGIN)
        });
        self.bytes = self.ready.values().map(read_subchunks_bytes).sum();
    }

    pub(super) fn step(
        &mut self,
        world: Arc<World>,
        mut options: ChunkLoadOptions,
        view: ViewState,
        center: (i32, i32),
        cached: impl Fn(ChunkPos) -> bool,
    ) {
        self.harvest();
        if self.read.is_some() || self.attempts >= IDLE_READ_LIMIT {
            return;
        }
        let mut positions = Vec::with_capacity(IDLE_READ_COLUMNS);
        let (x, z) = center;
        while positions.len() < IDLE_READ_COLUMNS && self.attempts < IDLE_READ_LIMIT {
            let Some(pos) = self.cursor.next(
                view.dimension, x, z, view.radius + IDLE_READ_MARGIN, view.forward_xz,
            ) else {
                self.attempts = IDLE_READ_LIMIT;
                break;
            };
            if inside_view(pos, view.dimension, x, z, view.radius)
                || cached(pos)
                || self.ready.contains_key(&pos)
            {
                continue;
            }
            let dx = pos.x.saturating_sub(x) as f32;
            let dz = pos.z.saturating_sub(z) as f32;
            if dx * view.forward_xz[0] + dz * view.forward_xz[1] < 0.0 {
                continue;
            }
            positions.push(pos);
            self.attempts += 1;
        }
        let Some(first) = positions.first() else {
            return;
        };
        let (min_y, max_y) = first.subchunk_index_range(ChunkVersion::New);
        // Prefer the surface and camera-height layers. Remaining exact
        // requests are made only when a column enters the foreground.
        let camera_y = (view.camera[1] / 16.0).floor() as i32;
        let preferred_y = if view.terrain_focus().is_some() {
            camera_y.min(TERRAIN_REFERENCE_Y.div_euclid(16) + 1)
        } else {
            camera_y
        };
        let top = preferred_y.clamp(i32::from(min_y), i32::from(max_y)) as i8;
        let bottom = top.saturating_sub(5).max(min_y);
        let requested = (bottom..=top).collect::<Vec<_>>();
        options.data_request = requested.iter().fold(ChunkDataRequest::new(), |request, &y| {
            request.layer(i32::from(y) * 16)
        });
        options.threading = WorldThreadingOptions::Single;
        let cancel = CancelFlag::new();
        options.cancel = Some(cancel.clone());
        let (tx, result) = sync_channel(1);
        rayon::spawn(move || {
            let Ok(chunks) = world.load_render_chunks(positions, options) else {
                return;
            };
            let results = chunks.into_iter()
                // Old terrain needs its complete legacy fallback on entry.
                .filter(|chunk| chunk.legacy_terrain.is_none())
                .map(|chunk| (chunk.pos, ReadSubchunks {
                    requested: requested.clone(),
                    present: chunk.subchunks,
                }))
                .collect();
            let _ = tx.send(results);
        });
        self.read = Some(IdleRead { result, cancel });
    }
}

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
    #[ignore = "requires a read-only Bedrock map with terrain near the test position"]
    fn idle_reads_reuse_real_map_subchunks_on_entry() {
        let path = std::env::var_os("BMCBL_WORLD3D_TEST_WORLD").expect("test world");
        let world = Arc::new(
            World::open(
                std::path::PathBuf::from(path),
                bedrock_world::world::OpenOptions { read_only: true, ..Default::default() },
            ).unwrap(),
        );
        let old_center = (-7, 11);
        let view = ViewState {
            generation: 1,
            dimension: bedrock_render::Dimension::Overworld,
            camera: [-108.0, 144.0, 178.0],
            forward_xz: [0.0, -1.0],
            forward_y: -0.35,
            radius: 2,
            view_distance: 32.0,
        };
        let mut idle = IdleChunkReads::default();
        idle.update_view(view, old_center);
        let options = ChunkLoadOptions {
            subchunk_decode: bedrock_world::SubChunkDecodeMode::PackedIndices,
            data_request: ChunkDataRequest::new().full_3d_indices(),
            threading: WorldThreadingOptions::Single,
            ..Default::default()
        };
        idle.step(Arc::clone(&world), options.clone(), view, old_center, |_| false);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while idle.read.is_some() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            idle.harvest();
        }
        assert!(idle.read.is_none());
        let new_center = (-7, 9);
        let positions = idle.ready.keys().copied().collect::<Vec<_>>();
        assert_eq!(positions.len(), IDLE_READ_COLUMNS);
        assert!(positions.iter().all(|&pos| {
            !inside_view(pos, view.dimension, old_center.0, old_center.1, view.radius)
                && inside_view(pos, view.dimension, new_center.0, new_center.1, view.radius)
        }));
        idle.update_view(view, new_center);
        let predecoded = positions.iter().map(|&pos| (pos, idle.take(pos).unwrap()))
            .collect::<BTreeMap<_, _>>();
        let (expected, cold) = world.load_render_chunks_with_stats(positions.iter().copied(), options.clone()).unwrap();
        let (actual, warm) = world.load_render_chunks_reusing_subchunks(positions, options, predecoded).unwrap();
        eprintln!(
            "world3d idle read test keys={}->{} decodes={}->{}",
            cold.keys_requested, warm.keys_requested, cold.subchunks_decoded, warm.subchunks_decoded,
        );
        assert_eq!(expected, actual);
        assert!(warm.keys_requested < cold.keys_requested);
        assert!(warm.subchunks_decoded < cold.subchunks_decoded);
    }

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
                    preflight_masks: Default::default(),
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

    #[test]
    fn a_view_notification_does_not_consume_the_prefetch_result() {
        let (result_tx, receiver) = sync_channel(1);
        let (command_tx, commands) = channel();
        let cancel = CancelFlag::new();
        let pending = PendingChunkBatch {
            receiver,
            cancel: cancel.clone(),
            positions: Vec::new(),
        };
        command_tx.send(ChunkServiceCommand::ViewUpdated).unwrap();
        assert!(matches!(
            pending.wait(&commands),
            BatchWait::Command(ChunkServiceCommand::ViewUpdated)
        ));
        assert!(!cancel.is_cancelled());
        result_tx
            .send((
                LoadedChunkBatch {
                    chunks: Vec::new(),
                    preflight_masks: Default::default(),
                    stats: Default::default(),
                    query_time: Duration::ZERO,
                    collect_time: Duration::ZERO,
                },
                Duration::from_millis(7),
            ))
            .unwrap();
        assert!(matches!(pending.wait(&commands), BatchWait::Loaded(..)));
    }
}
