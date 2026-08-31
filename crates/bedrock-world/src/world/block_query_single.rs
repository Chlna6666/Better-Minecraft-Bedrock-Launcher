//! Single-position authoritative Minecraft Bedrock BlockState query.

use super::{BedrockWorld, WorldStorageHandle};
use crate::{BlockPos, BlockState, Dimension, Result};

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Reads one exact persisted primary-layer BlockState on the calling thread.
    ///
    /// This is a single-position convenience entry point over the canonical batched authoritative
    /// query. It does not introduce another SubChunk/palette decoder and therefore retains exactly
    /// the same modern SubChunk and LegacyTerrain fallback semantics as
    /// [`BedrockWorld::get_block_states_at_blocking`].
    pub fn get_block_state_at_blocking(
        &self,
        dimension: Dimension,
        position: BlockPos,
    ) -> Result<Option<BlockState>> {
        let mut results = self.get_block_states_at_blocking(dimension, [position])?;
        Ok(results.pop().and_then(|result| result.state))
    }
}
