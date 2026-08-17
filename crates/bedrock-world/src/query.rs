//! Minecraft Bedrock world inspection and overlay queries.
//!
//! Query operations are grouped by their actual implementation ownership. Exact non-rectangular
//! selections are an independent responsibility and therefore keep a dedicated child module.

mod operations;
pub mod selection;

/// Guarded mutation helpers used by query-driven world tools.
pub mod write {
    pub use super::operations::{
        WriteGuard, delete_chunk_positions_blocking, delete_chunks_blocking,
        write_chunk_record_nbt_blocking,
    };
}

pub use operations::*;
pub use selection::*;
pub(crate) use crate::parsed::ParsedChunkRecordValue;
