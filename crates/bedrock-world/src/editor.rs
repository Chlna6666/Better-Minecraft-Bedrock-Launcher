//! Typed, policy-guarded Minecraft Bedrock world editing.

pub mod block_edit;

pub use block_edit::{
    BlockEdit, BlockEditOptions, BlockEditResult, BlockEntityEdit, BlockStorageLayer,
    apply_block_edits_blocking, set_block_state_blocking,
};
pub use crate::mcstructure::placement::{
    McStructurePlacement, McStructureRotation, McStructureWritePhase, McStructureWriteProgress,
    McStructureWriteResult,
};
pub use crate::query::write::{
    WriteGuard, delete_chunk_positions_blocking, delete_chunks_blocking,
    write_chunk_record_nbt_blocking,
};
