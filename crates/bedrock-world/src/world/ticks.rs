//! Chunk tick queue access through the world transaction boundary.

use super::*;
use crate::chunk::ticks::ChunkTicks;

impl<S> World<S>
where
    S: StorageBackend,
{
    /// Reads the scheduled and random tick queues persisted for one chunk.
    ///
    /// # Errors
    ///
    /// Returns storage errors or errors for malformed tick NBT.
    pub fn chunk_ticks(&self, pos: ChunkPos) -> Result<ChunkTicks> {
        let pending = self
            .storage()
            .get(&ChunkKey::new(pos, ChunkRecordTag::PendingTicks).encode())?;
        let random = self
            .storage()
            .get(&ChunkKey::new(pos, ChunkRecordTag::RandomTicks).encode())?;
        ChunkTicks::from_records(pending, random)
    }

    /// Writes edited scheduled and random tick queues for one chunk in a single transaction.
    ///
    /// A queue set to `None` is deleted; a present empty queue is written as an empty payload.
    /// Source bytes from [`Self::chunk_ticks`] are checked before commit, so an older read cannot
    /// overwrite a newer queue written through this world.
    ///
    /// # Errors
    ///
    /// Returns read-only, validation, stale-source, serialization, or storage errors.
    pub fn write_chunk_ticks(&self, pos: ChunkPos, ticks: &ChunkTicks) -> Result<()> {
        self.ensure_writable()?;
        let mut transaction = self.transaction();
        transaction.write_chunk_ticks(pos, ticks)?;
        transaction.commit()
    }
}

impl<S> WorldTransaction<'_, S>
where
    S: StorageBackend,
{
    /// Stages both tick queues for one chunk and validates their source snapshots at commit.
    ///
    /// # Errors
    ///
    /// Returns validation or serialization errors for non-compound tick entries.
    pub fn write_chunk_ticks(&mut self, pos: ChunkPos, ticks: &ChunkTicks) -> Result<()> {
        // Validate both queues before staging either: callers may continue using
        // this transaction after a failed edit.
        let pending = ticks.encode_pending()?;
        let random = ticks.encode_random()?;
        self.stage_tick_queue(
            ChunkKey::new(pos, ChunkRecordTag::PendingTicks),
            ticks.pending_source(),
            pending,
        );
        self.stage_tick_queue(
            ChunkKey::new(pos, ChunkRecordTag::RandomTicks),
            ticks.random_source(),
            random,
        );
        Ok(())
    }

    fn stage_tick_queue(&mut self, key: ChunkKey, source: Option<&Bytes>, encoded: Option<Bytes>) {
        let key = key.encode();
        self.require_raw_key(key.clone(), source.cloned());
        match encoded {
            Some(value) => self.batch.put(key, value),
            None => self.batch.delete(key),
        }
    }
}
