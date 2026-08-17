//! Blocks, block states, palettes and block-entity data.

// Compile canonical BlockState identity helpers as part of the block domain instead of
// mounting the file at the crate root with `#[path]`.
mod state;

pub use crate::chunk::model::BlockPos;
pub use crate::chunk::palette::{BlockPalette, BlockState, block_storage_index};
pub use crate::parsed::{BlockEntityRecord, ParsedBlockEntity};
