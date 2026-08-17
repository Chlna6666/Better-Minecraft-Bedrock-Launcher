//! Public exact-selection planning and chunk loading for render consumers.

use crate::Result;
use bedrock_world::{
    chunk::ChunkPos,
    query::{ExactChunkSelection, SlimeChunkBounds},
    world::{BedrockWorld, ChunkData, ChunkLoadOptions, WorldStorageHandle},
};

/// A render-oriented plan derived from an exact non-rectangular chunk selection.
///
/// `bounds` is only spatial metadata. Consumers must use `positions` or
/// `selection.contains(...)` for membership decisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactChunkRenderPlan {
    selection: ExactChunkSelection,
    bounds: SlimeChunkBounds,
    positions: Vec<ChunkPos>,
    rectangle_cover: Vec<SlimeChunkBounds>,
}

impl ExactChunkRenderPlan {
    /// Builds a render plan without expanding the exact selection.
    #[must_use]
    pub fn new(selection: ExactChunkSelection) -> Self {
        let bounds = selection.bounds();
        let positions = selection.to_vec();
        let rectangle_cover = selection.rectangle_cover();
        Self {
            selection,
            bounds,
            positions,
            rectangle_cover,
        }
    }

    /// Returns the exact selection backing this plan.
    #[must_use]
    pub const fn selection(&self) -> &ExactChunkSelection {
        &self.selection
    }

    /// Returns the bounding rectangle used only for framing and spatial metadata.
    #[must_use]
    pub const fn bounds(&self) -> SlimeChunkBounds {
        self.bounds
    }

    /// Returns the exact selected positions in stable row-major order.
    #[must_use]
    pub fn positions(&self) -> &[ChunkPos] {
        &self.positions
    }

    /// Returns the exact number of selected chunks.
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.positions.len()
    }

    /// Returns an exact rectangle cover for adapting rectangle-oriented renderers.
    ///
    /// Every returned rectangle is fully contained in the selection, so holes and
    /// disconnected gaps are never filled.
    #[must_use]
    pub fn rectangle_cover(&self) -> &[SlimeChunkBounds] {
        &self.rectangle_cover
    }

    /// Returns whether a chunk belongs to the exact render selection.
    #[must_use]
    pub fn contains(&self, chunk: ChunkPos) -> bool {
        self.selection.contains(chunk)
    }
}

/// Chunk data loaded for an exact render selection.
#[derive(Clone, Debug)]
pub struct ExactChunkRenderData {
    /// The exact plan used to perform the load.
    pub plan: ExactChunkRenderPlan,
    /// Loaded render data for the exact requested positions.
    pub chunks: Vec<ChunkData>,
}

/// Loads exactly the selected chunks, preserving holes and disconnected components.
///
/// This uses the canonical explicit-position world query and never expands the
/// selection to its bounding rectangle.
///
/// # Errors
///
/// Returns storage, decode, cancellation, or chunk-loading errors from `bedrock-world`.
pub fn load_exact_chunk_render_data_blocking<S>(
    world: &BedrockWorld<S>,
    selection: ExactChunkSelection,
    options: ChunkLoadOptions,
) -> Result<ExactChunkRenderData>
where
    S: WorldStorageHandle,
{
    let plan = ExactChunkRenderPlan::new(selection);
    let (chunks, _) = world.query_chunk_data_with_stats_blocking(
        plan.positions().iter().copied(),
        options,
    )?;
    Ok(ExactChunkRenderData { plan, chunks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_world::chunk::{ChunkPos, Dimension};

    fn chunk(x: i32, z: i32) -> ChunkPos {
        ChunkPos {
            x,
            z,
            dimension: Dimension::Overworld,
        }
    }

    #[test]
    fn plan_preserves_disconnected_selection() {
        let selection = ExactChunkSelection::new([chunk(0, 0), chunk(2, 0)])
            .expect("exact selection");
        let plan = ExactChunkRenderPlan::new(selection);

        assert_eq!(plan.chunk_count(), 2);
        assert!(plan.contains(chunk(0, 0)));
        assert!(!plan.contains(chunk(1, 0)));
        assert!(plan.contains(chunk(2, 0)));
        assert_eq!(plan.rectangle_cover().len(), 2);
    }
}
