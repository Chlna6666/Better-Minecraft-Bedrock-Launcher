//! Public exact-selection planning and data loading for render consumers.
//!
//! 2D surface rendering and 3D chunk rendering intentionally use separate data
//! contracts. Surface consumers receive compact `SurfaceMapChunk` values, while
//! 3D/mesh consumers continue to receive full `ChunkData`.

use crate::Result;
pub use bedrock_world::query::ExactChunkSelection;
use bedrock_world::{
    chunk::ChunkPos,
    query::SlimeChunkBounds,
    surface::{
        ChunkData, ChunkLoadOptions, SurfaceMapBatchStats, SurfaceMapChunk, SurfaceMapQueryOptions,
    },
    world::{StorageBackend, World},
};

/// A render-oriented plan derived from an exact non-rectangular chunk selection.
///
/// `bounds` is only spatial metadata. Consumers must use `positions` or
/// `selection.contains(...)` for membership decisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    selection: ExactChunkSelection,
    bounds: SlimeChunkBounds,
    positions: Vec<ChunkPos>,
    rectangle_cover: Vec<SlimeChunkBounds>,
}

impl Plan {
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

/// Full chunk data loaded for an exact render selection.
///
/// This is the 3D/mesh-oriented contract and deliberately retains the existing
/// `ChunkData` path.
#[derive(Clone, Debug)]
pub struct Chunks {
    /// The exact plan used to perform the load.
    pub plan: Plan,
    /// Loaded render data for the exact requested positions.
    pub chunks: Vec<ChunkData>,
}

/// Compact 2D surface data loaded for an exact render selection.
#[derive(Clone, Debug)]
pub struct Surfaces {
    /// The exact plan used to perform the load.
    pub plan: Plan,
    /// Compact exact 16x16 surface planes for the requested positions.
    pub chunks: Vec<SurfaceMapChunk>,
    /// Storage/decode diagnostics for the surface batch.
    pub stats: SurfaceMapBatchStats,
}

/// Loads exactly the selected chunks for 3D/mesh consumers, preserving holes and
/// disconnected components.
///
/// This uses the canonical explicit-position world query and never expands the
/// selection to its bounding rectangle.
///
/// # Errors
///
/// Returns storage, decode, cancellation, or chunk-loading errors from `bedrock-world`.
pub fn load_chunks<S>(
    world: &World<S>,
    selection: ExactChunkSelection,
    options: ChunkLoadOptions,
) -> Result<Chunks>
where
    S: StorageBackend,
{
    let plan = Plan::new(selection);
    let (chunks, _) =
        world.load_render_chunks_with_stats(plan.positions().iter().copied(), options)?;
    Ok(Chunks { plan, chunks })
}

/// Loads exactly the selected chunks as compact 2D surface planes.
///
/// Unlike [`load_chunks`], this route does not expose
/// general `ChunkData`, full 3D indices, or block entities. The world-layer
/// surface contract is exact; rendering colors remain the responsibility of
/// `bedrock-render`.
///
/// # Errors
///
/// Returns storage, decode, cancellation, or surface-projection errors from
/// `bedrock-world`.
pub fn load_surfaces<S>(
    world: &World<S>,
    selection: ExactChunkSelection,
    options: SurfaceMapQueryOptions,
) -> Result<Surfaces>
where
    S: StorageBackend,
{
    let plan = Plan::new(selection);
    let (chunks, stats) =
        world.query_surface_map_many(plan.positions().iter().copied(), options)?;
    Ok(Surfaces {
        plan,
        chunks,
        stats,
    })
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
        let selection =
            ExactChunkSelection::new([chunk(0, 0), chunk(2, 0)]).expect("exact selection");
        let plan = Plan::new(selection);

        assert_eq!(plan.chunk_count(), 2);
        assert!(plan.contains(chunk(0, 0)));
        assert!(!plan.contains(chunk(1, 0)));
        assert!(plan.contains(chunk(2, 0)));
        assert_eq!(plan.rectangle_cover().len(), 2);
    }
}
