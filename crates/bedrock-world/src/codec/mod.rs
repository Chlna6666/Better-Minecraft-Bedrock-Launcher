//! Stable Bedrock codec surface.
//!
//! Codecs translate raw Minecraft Bedrock bytes into semantic models and back. They do not decide
//! whether a historical record should be migrated or whether a write is allowed.

pub mod level_dat;
pub mod nbt;
pub mod subchunk;

pub use crate::chunk::legacy::{
    LEGACY_SUBCHUNK_BLOCK_COUNT, LEGACY_SUBCHUNK_MIN_VALUE_LEN,
    LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN, LEGACY_TERRAIN_BLOCK_COUNT,
    LEGACY_TERRAIN_VALUE_LEN,
};
pub use crate::chunk::palette::block_storage_index;
pub use crate::chunk::subchunk::{parse_subchunk, parse_subchunk_with_mode};
pub use crate::mcstructure::codec::{
    McStructureBlock, McStructureFile, McStructurePaletteEntry, McStructureSize,
    read_mcstructure_file, write_mcstructure_file,
};
pub use crate::mcstructure::placement::{McStructurePlacement, McStructureRotation};
pub use nbt::{NbtEvent, NbtReader, NbtRef, NbtTag, NbtValue, NbtView, NbtWriter, visit_nbt_events};
pub use crate::parsed::report::{RetentionMode, WorldParseCategories, WorldParseOptions, WorldParseReport};
pub use subchunk::{encode_palette_layer, encode_paletted_subchunk};
