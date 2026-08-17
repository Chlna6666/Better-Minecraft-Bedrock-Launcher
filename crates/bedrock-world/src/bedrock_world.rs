//! Tools for inspecting, upgrading and editing Minecraft Bedrock worlds.
//!
//! `bedrock-world` owns Minecraft Bedrock world semantics. Mojang LevelDB mechanics belong exclusively
//! to `bedrock-leveldb`. The 0.7 API is intentionally breaking and is organised by Bedrock game-data
//! domains instead of generic software layers.

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

#[path = "block/state.rs"]
mod block_state;
pub mod chunk;
#[path = "world/discover.rs"]
mod discover;
/// Crate-wide Bedrock world error types.
pub mod error;
mod mcstructure;
mod nbt_ref;
mod parsed;
#[path = "player/data.rs"]
mod player_impl;
mod selection_query;
#[path = "world/surface.rs"]
mod surface;

/// Blocks, block states, palettes and block-entity data.
pub mod block {
    pub use crate::chunk::model::BlockPos;
    pub use crate::chunk::palette::{BlockPalette, BlockState, block_storage_index};
    pub use crate::parsed::model::{BlockEntityRecord, ParsedBlockEntity};
}

/// Biome and height-map data stored by Bedrock chunks.
pub mod biome {
    pub use crate::chunk::legacy::LegacyBiomeSample;
    pub use crate::parsed::model::{
        Biome2d, Biome3d, HeightMap2d, ParsedBiomeData, ParsedBiomeStorage,
    };
}

/// Bedrock actor/entity records and actor-index identities.
pub mod entity {
    pub use crate::chunk::key::{ActorDigestKey, ActorUid};
    pub use crate::chunk::model::EntityData;
    pub use crate::parsed::model::{
        ActorRecord, ActorResolution, ActorSource, ParsedActorDigest, ParsedEntity,
    };
    pub use crate::parsed::{encode_actor_digest_ids, parse_actor_digest_ids};
}

/// Bedrock player records and inventory item data.
pub mod player {
    pub use crate::parsed::model::{ItemStack, ParsedPlayer};
    pub use crate::player_impl::{PlayerData, PlayerId};
}

/// Bedrock map item records.
pub mod map {
    pub use crate::chunk::key::MapRecordId;
    pub use crate::parsed::model::{MapKnownFields, MapPixels, ParsedMapData};
}

/// Bedrock village database records.
pub mod village {
    pub use crate::chunk::key::{ParsedVillageKey, VillageRecordKind};
    pub use crate::parsed::model::ParsedVillageData;
}

/// Bedrock `.mcstructure` files and structure placement.
pub mod structure {
    pub use crate::mcstructure::codec::{
        McStructureBlock, McStructureFile, McStructurePaletteEntry, McStructureSize,
        read_mcstructure_file, write_mcstructure_file,
    };
    pub use crate::mcstructure::placement::{McStructurePlacement, McStructureRotation};
}

/// Bedrock little-endian NBT parsing, writing and borrowed views.
pub mod nbt;

/// `level.dat` document access and world-level metadata.
pub mod level;

/// Bedrock world database records, scanning and LevelDB-backed world storage.
pub mod database;

/// Explicit historical Bedrock world and BlockState upgrades.
pub mod upgrade;

/// Bedrock world compatibility and integrity validation.
pub mod integrity;

/// Typed policy-guarded Bedrock world editing.
pub mod editor;

/// Read/query APIs for maps, regions and selections.
pub mod query;
/// High-level lazy world lifecycle, scans and transactions.
pub mod world;

// Crate-private migration shims. Physical generic directories are gone; these aliases exist only so
// large implementation files can be migrated incrementally without restoring any pre-0.7 public API.
mod model {
    pub(crate) use crate::chunk::key::{
        ActorDigestKey, ActorUid, BedrockDbKey, BedrockDbKeyKind, ChunkKey, ChunkRecordTag,
        EncodedChunkKey, GlobalRecordKind, MapRecordId, ParsedVillageKey, VillageRecordKind,
    };
    pub(crate) use crate::chunk::legacy::{LegacyBiomeSample, LegacySubChunk, LegacyTerrain};
    pub(crate) use crate::chunk::model::{
        BlockPos, Chunk, ChunkPos, ChunkRecord, ChunkVersion, Dimension, EntityData,
    };
    pub(crate) use crate::chunk::palette::{BlockPalette, BlockState};
    pub(crate) use crate::chunk::subchunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat};
    pub(crate) use crate::parsed::model::{
        ActorRecord, ActorResolution, ActorSource, Biome2d, Biome3d, BlockEntityRecord,
        HardcodedSpawnAreaKind, HeightMap2d, ItemStack, MapKnownFields, MapPixels,
        ParsedActorDigest, ParsedBiomeData, ParsedBiomeStorage, ParsedBlockEntity, ParsedChunkData,
        ParsedChunkRecord, ParsedChunkRecordValue, ParsedDbEntry, ParsedDbValue, ParsedEntity,
        ParsedGlobalData, ParsedHardcodedSpawnArea, ParsedMapData, ParsedPlayer, ParsedVillageData,
        ParsedWorld,
    };
    pub(crate) use crate::player_impl::{PlayerData, PlayerId};
}

mod storage {
    pub(crate) use crate::database::*;
}

// Temporary crate-root aliases for implementation files still being moved to direct domain imports.
pub(crate) use chunk::key::{
    ActorDigestKey, ActorUid, BedrockDbKey, BedrockDbKeyKind, ChunkKey, ChunkRecordTag,
    EncodedChunkKey, GlobalRecordKind, MapRecordId, ParsedVillageKey, VillageRecordKind,
};
pub(crate) use chunk::legacy::{LegacyBiomeSample, LegacySubChunk, LegacyTerrain};
pub(crate) use chunk::model::{
    BlockPos, Chunk, ChunkPos, ChunkRecord, ChunkVersion, Dimension, EntityData,
};
pub(crate) use chunk::palette::{BlockPalette, BlockState, block_storage_index};
pub(crate) use chunk::subchunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat};
pub(crate) use parsed::model::{
    ActorRecord, ActorResolution, ActorSource, Biome2d, Biome3d, BlockEntityRecord,
    HardcodedSpawnAreaKind, HeightMap2d, ItemStack, MapKnownFields, MapPixels, ParsedActorDigest,
    ParsedBiomeData, ParsedBiomeStorage, ParsedBlockEntity, ParsedChunkData, ParsedChunkRecord,
    ParsedChunkRecordValue, ParsedDbEntry, ParsedDbValue, ParsedEntity, ParsedGlobalData,
    ParsedHardcodedSpawnArea, ParsedMapData, ParsedPlayer, ParsedVillageData, ParsedWorld,
};
pub(crate) use player_impl::{PlayerData, PlayerId};
pub(crate) use integrity::{ChunkCapabilities, CompatibilityLevel, WritePolicy};
pub(crate) use nbt::{NbtReader, NbtTag, NbtWriter};
pub(crate) use error::{BedrockWorldError, BedrockWorldErrorKind, Result};
pub(crate) use level as level_dat;
pub(crate) use query::WriteGuard;
pub(crate) use database::{MemoryStorage, StorageCachePolicy, StorageReadOptions, WorldStorage};
pub(crate) use world::{
    BedrockWorld, CancelFlag, OpenOptions, SurfaceColumn, WorldChunkQueryRegion, WorldStorageHandle,
    WorldTransaction,
};
