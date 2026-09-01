//! Typed, policy-guarded Minecraft Bedrock world editing.

pub mod block_edit;
mod block_edit_plan;

pub use crate::mcstructure::{
    McStructurePlacement, McStructureRotation, McStructureWritePhase, McStructureWriteProgress,
    McStructureWriteResult,
};
pub use crate::query::{
    WriteGuard, delete_selected_chunks, delete_chunks,
    save_chunk_nbt,
};
pub use block_edit::{
    BlockEdit, BlockEditOptions, BlockEditResult, BlockEntityEdit, BlockStorageLayer,
    apply_block_edits, set_block_state,
};
pub use block_edit_plan::{BlockEditPlan, BlockStateCondition, PlanStatus, plan_block_edits};
