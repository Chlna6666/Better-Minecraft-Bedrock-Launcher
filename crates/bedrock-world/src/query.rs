//! Minecraft Bedrock world inspection and overlay queries.
//!
//! Query APIs are organised around game data and spatial inspection. Generic implementation-layer
//! names are not part of the 0.7 public API.

#[path = "query/impl.rs"]
mod implementation;

pub mod analysis;
pub mod inspect;
pub mod overlay;

/// Guarded mutation helpers used by query-driven world tools.
pub mod write {
    pub use super::implementation::{
        WriteGuard, delete_chunk_positions_blocking, delete_chunks_blocking,
        write_chunk_record_nbt_blocking,
    };
}

pub use implementation::*;

// Temporary crate-private bridge while selection/query implementation files are physically moved to
// their final game-domain locations. This is not exported to external consumers.
pub(crate) use crate::parsed::model::ParsedChunkRecordValue;
