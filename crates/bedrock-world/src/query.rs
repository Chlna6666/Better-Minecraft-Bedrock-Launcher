//! Professional map/world queries used by viewers and offline tools.
//!
//! Implementation remains behind a compatibility facade while responsibility entry points live in
//! dedicated files under `query/`.

#[path = "query/impl.rs"]
mod implementation;

pub mod analysis;
pub mod inspect;
pub mod overlay;

/// Explicit guarded mutation helpers retained during the 0.6 transition.
pub mod write {
    pub use super::implementation::{
        WriteGuard, delete_chunk_positions_blocking, delete_chunks_blocking,
        write_chunk_record_nbt_blocking,
    };
}

pub use implementation::*;
