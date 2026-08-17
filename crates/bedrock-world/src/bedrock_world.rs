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

pub mod block;
pub mod chunk;
/// Crate-wide Bedrock world error types.
pub mod error;
mod mcstructure;
mod nbt_ref;
mod parsed;
pub mod player;

/// Biome and height-map data stored by Bedrock chunks.
pub mod biome {
    pub use crate::chunk::legacy::LegacyBiomeSample;
    pub use crate::parsed::{Biome2d, Biome3d, HeightMap2d, ParsedBiomeData, ParsedBiomeStorage};
}

/// Bedrock actor/entity records and actor-index identities.
pub mod entity {
    pub use crate::chunk::key::{ActorDigestKey, ActorUid};
    pub use crate::chunk::EntityData;
    pub use crate::parsed::{
        ActorRecord, ActorResolution, ActorSource, ParsedActorDigest, ParsedEntity,
        encode_actor_digest_ids, parse_actor_digest_ids,
    };
}

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

pub(crate) use world::{discover, surface};

pub(crate) use biome::{Biome2d, Biome3d};
pub(crate) use block::{BlockPalette, BlockPos, BlockState, block_storage_index};
pub(crate) use chunk::{
    ActorDigestKey, ActorUid, Chunk, ChunkKey, ChunkPos, ChunkRecord, ChunkRecordTag, ChunkVersion,
    Dimension, ParsedVillageKey, SubChunk, SubChunkFormat,
};
pub(crate) use database::{StorageCachePolicy, StorageReadOptions};
pub(crate) use error::{BedrockWorldError, BedrockWorldErrorKind, Result};
pub(crate) use integrity::{ChunkCapabilities, CompatibilityLevel, WritePolicy};
pub(crate) use nbt::{NbtReader, NbtTag, NbtWriter};
pub(crate) use parsed::ParsedChunkRecord;
pub(crate) use query::WriteGuard;
pub(crate) use world::{
    BedrockWorld, CancelFlag, SurfaceColumn, WorldChunkQueryRegion, WorldStorageHandle,
    WorldTransaction,
};

pub(crate) mod level_dat {
    pub(crate) use crate::level::{
        LevelDatDocument, read_level_dat_document, write_level_dat_document,
    };
}

pub(crate) mod storage {
    pub(crate) use crate::database::*;

    pub(crate) mod backend {
        pub(crate) use crate::database::BedrockLevelDbStorage;
    }
}
