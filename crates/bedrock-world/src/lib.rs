//! Multi-version Minecraft Bedrock world reading, writing and inspection.
//!
//! `bedrock-world` owns Minecraft Bedrock world semantics. Mojang LevelDB mechanics belong exclusively
//! to `bedrock-leveldb`. Ordinary reads and writes keep the persisted Bedrock representation. World
//! upgrade and downgrade are separate caller-requested operations.

#![deny(missing_docs)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::items_after_test_module,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::struct_excessive_bools,
    clippy::type_complexity,
    clippy::wildcard_imports
)]

pub mod biome;
pub mod block;
pub mod chunk;
pub mod entity;
/// Crate-wide Bedrock world error types.
pub mod error;
/// Bedrock saved-item identities, stacks and historical item data.
pub mod item;
mod mcstructure;
mod nbt_ref;
/// Structured Bedrock record parsing and consecutive-record encoding APIs.
pub mod parsed;
pub mod player;
/// Minecraft Bedrock game and persisted data version information.
pub mod version;

/// Bedrock map item records.
pub mod map {
    pub use crate::chunk::key::MapRecordId;
    pub use crate::parsed::{MapKnownFields, MapPixels, ParsedMapData};
}

/// Bedrock village database records.
pub mod village {
    pub use crate::chunk::key::{ParsedVillageKey, VillageRecordKind};
    pub use crate::parsed::ParsedVillageData;
}

/// Bedrock `.mcstructure` files and structure placement.
pub mod structure {
    pub use crate::mcstructure::{
        McStructureBlock, McStructureFile, McStructurePaletteEntry, McStructurePlacement,
        McStructureRotation, McStructureSize, read_mcstructure_file, write_mcstructure_file,
    };
}

/// Bedrock little-endian NBT parsing, writing and borrowed views.
pub mod nbt;

/// `level.dat` document access and world-level metadata.
pub mod level;

/// Raw Bedrock records, storage contracts and supported world-storage backends.
pub mod storage;

/// Bedrock world compatibility and integrity validation.
pub mod integrity;

/// Typed Bedrock world editing.
pub mod editor;

/// Read/query APIs for maps, regions and selections.
pub mod query;
/// High-level lazy world lifecycle, scans and transactions.
pub mod world;

/// Bedrock world-folder discovery APIs.
pub mod discover {
    pub use crate::world::discover::*;
}

pub(crate) use world::surface;

pub use biome::{
    Biome2d, Biome3d, HeightMap2d, LegacyBiomeSample, ParsedBiomeData, ParsedBiomeStorage,
    encode_data3d_quart,
};
pub use block::{BlockPalette, BlockPos, BlockState, block_storage_index};
pub use chunk::{
    ActorDigestKey, ActorUid, Chunk, ChunkKey, ChunkPos, ChunkRecord, ChunkRecordTag, ChunkVersion,
    Dimension, ParsedVillageKey, SubChunk, SubChunkDecodeMode, SubChunkFormat,
};
pub use error::{BedrockWorldError, BedrockWorldErrorKind, Result};
pub use integrity::{ChunkCapabilities, CompatibilityLevel};
pub use item::ItemStack;
pub use level::*;
pub use nbt::{NbtReader, NbtTag, NbtWriter};
pub use parsed::ParsedChunkRecord;
pub use player::{PlayerData, PlayerId};
pub use query::{
    ExactChunkSelection, SlimeChunkBounds, SlimeChunkWindow, SlimeWindowSize, WriteGuard,
    query_selection_stats_exact_blocking, rasterize_chunk_line,
};
pub use storage::*;
pub use world::*;

pub(crate) mod level_dat {
    pub(crate) use crate::level::{
        LevelDatDocument, read_level_dat_document, write_level_dat_document,
    };
}
