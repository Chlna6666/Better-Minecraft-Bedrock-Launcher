//! Multi-version Minecraft Bedrock world reading, writing and inspection.
//!
//! `bedrock-world` owns Minecraft Bedrock world semantics. Mojang LevelDB mechanics belong exclusively
//! to `bedrock-leveldb`. Ordinary reads and writes keep the persisted Bedrock representation. World
//! upgrade and downgrade are separate caller-requested operations.
//!
//! This reusable semantic layer parses and edits typed world data through [`WorldStorage`]; it does
//! not require a LevelDB handle. [`World`] can use in-memory storage or another `WorldStorage`
//! implementation, while its transactions coordinate record writes and commits.

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
pub mod global;
/// Crate-wide Bedrock world error types.
pub mod error;
/// Bedrock saved-item identities, stacks and historical item data.
pub mod item;
mod mcstructure;
pub mod player;
/// Historical Pocket Edition world files.
pub mod pocket;
/// Structured Bedrock record parsing and consecutive-record encoding APIs.
pub mod scan;
/// Minecraft Bedrock game and persisted data version information.
pub mod version;

/// Bedrock map item records.
pub mod map_item;

/// Bedrock village database records.
pub mod village;

/// Bedrock `.mcstructure` files and structure placement.
pub mod structure {
    pub use crate::mcstructure::{
        McStructureBlock, McStructureFile, McStructurePaletteEntry, McStructurePlacement,
        McStructureRotation, McStructureSize, read_mcstructure_file, write_mcstructure_file,
    };
}

/// Bedrock little-endian NBT parsing, writing and borrowed views.
pub use bedrock_nbt as nbt;

/// `level.dat` document access and world-level metadata.
pub mod level;

/// Raw Bedrock records, storage contracts and supported world-storage backends.
pub mod storage;

/// Terrain surface sampling and region loading.
pub mod surface;

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

pub use biome::{
    Biome2d, Biome3d, BiomeData, BiomeStorage, HeightMap2d, LegacyBiomeSample, encode_data3d_quart,
};
pub use block::{
    BlockPalette, BlockPos, BlockState, BlockStateBatchStats, BlockStateQueryControl,
    BlockStateQueryResult, BlockStateView, block_storage_index,
};
pub use chunk::{
    ActorDigestKey, ActorUid, ChunkBlockEntities, ChunkHeightMap, ChunkHeightMapStatus, ChunkKey,
    ChunkPos, ChunkRecord, ChunkRecordTag, ChunkVersion, Dimension, LevelChunk, SubChunk,
    SubChunkDecodeMode, SubChunkFormat, VillageKey,
};
pub use error::{BedrockWorldError, BedrockWorldErrorKind, Result};
pub use integrity::{ChunkCapabilities, CompatibilityLevel};
pub use item::ItemStack;
pub use level::{
    LevelChunkVersionCount, LevelDatDocument, LevelDatHeader, LevelDatReadWarning,
    SubChunkVersionCount, WorldClock, WorldVersions,
};
pub use nbt::{NbtReader, NbtTag, NbtWriter};
pub use player::{PlayerData, PlayerId};
pub use query::{
    ExactChunkSelection, SlimeChunkBounds, SlimeChunkWindow, SlimeFarmCandidate,
    SlimeFarmQueryMode, SlimeWindowSize, WriteGuard, exact_selection_stats,
    slime_farm_candidates, rasterize_chunk_line,
};
pub use scan::ChunkEntry;
pub use storage::*;
pub use world::*;
