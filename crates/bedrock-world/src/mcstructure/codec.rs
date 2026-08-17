//! `.mcstructure` file model and codec entry points.

pub use super::implementation::{
    McStructureBlock, McStructureFile, McStructurePaletteEntry, McStructureSize,
    read_mcstructure_file, write_mcstructure_file,
};
