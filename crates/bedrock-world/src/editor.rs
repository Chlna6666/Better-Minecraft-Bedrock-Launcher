//! Typed, policy-guarded Minecraft Bedrock world editing.

pub mod block_edit;
mod prepared_block_edit;

pub use crate::mcstructure::{
    McStructurePlacement, McStructureRotation, McStructureWritePhase, McStructureWriteProgress,
    McStructureWriteResult,
};
pub use crate::query::{
    WriteGuard, delete_chunk_positions_blocking, delete_chunks_blocking,
    write_chunk_record_nbt_blocking,
};
pub use block_edit::{
    BlockEdit, BlockEditOptions, BlockEditResult, BlockEntityEdit, BlockStorageLayer,
    apply_block_edits_blocking, set_block_state_blocking,
};
pub use prepared_block_edit::{
    PreparedBlockEditBatch, PreparedBlockEditValidation, prepare_block_edits_blocking,
};
