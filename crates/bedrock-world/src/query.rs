//! Professional map/world queries used by viewers and offline tools.
//!
//! Query implementation lives under `query/`; this facade preserves the historical public API while
//! allowing read-only inspection, overlays, selection statistics and guarded writes to evolve separately.

#[path = "query/impl.rs"]
mod implementation;

pub use implementation::*;

/// Read-only chunk and block inspection queries.
pub mod inspect {
    pub use super::{
        BlockTip, ChunkDetail, ChunkRecordDetail, ChunkRecordFingerprint, ChunkRecordQuery,
        ChunkRecordQueryResult, query_block_tip_blocking, query_chunk_detail_blocking,
        query_chunk_records_many_blocking, query_chunk_records_many_blocking_with_control,
        fingerprint_chunk_records_many_blocking, fingerprint_chunk_records_many_blocking_with_control,
    };
}

/// Region overlay and map-analysis queries.
pub mod overlay {
    pub use super::{
        BlockEntityOverlay, EntityOverlay, HardcodedSpawnAreaOverlay, PendingTickOverlay,
        RegionOverlayQuery, RegionOverlayQueryOptions, VillageOverlay, VillageOverlayIndex,
        query_region_overlays_blocking, query_region_overlays_blocking_with_control,
    };
}

/// Selection and slime-chunk analysis.
pub mod analysis {
    pub use super::{
        SelectionStats, SlimeChunkBounds, SlimeChunkWindow, SlimeWindowSize,
        is_bedrock_slime_chunk, is_slime_chunk, query_selection_stats_blocking,
        query_slime_chunk_windows,
    };
}

/// Explicit guarded mutation helpers retained for compatibility.
pub mod write {
    pub use super::{
        WriteGuard, delete_chunk_positions_blocking, delete_chunks_blocking,
        write_chunk_record_nbt_blocking,
    };
}
