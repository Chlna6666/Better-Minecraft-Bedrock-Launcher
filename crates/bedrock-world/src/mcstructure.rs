//! Minecraft Bedrock `.mcstructure` codecs and world placement helpers.
//!
//! The implementation lives under `mcstructure/` so file codec, transform logic and world placement
//! can be separated without changing the historical public API.

#[path = "mcstructure/impl.rs"]
mod implementation;

pub use implementation::*;

/// `.mcstructure` file model and codec entry points.
pub mod codec {
    pub use super::{
        McStructureBlock, McStructureFile, McStructurePaletteEntry, McStructureSize,
        read_mcstructure_file, write_mcstructure_file,
    };
}

/// Placement, rotation and write-progress models.
pub mod placement {
    pub use super::{
        McStructurePlacement, McStructureRotation, McStructureWritePhase, McStructureWriteProgress,
        McStructureWriteResult,
    };
}
