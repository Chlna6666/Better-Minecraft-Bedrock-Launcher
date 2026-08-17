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

#[path = "model/block_state.rs"]
mod block_state;
pub mod chunk;
#[path = "world/discover.rs"]
mod discover;
/// Crate-wide Bedrock world error types.
pub mod error;
mod mcstructure;
mod nbt_ref;
mod parsed;
#[path = "model/player.rs"]
mod player_impl;
mod selection_query;
#[path = "model/surface.rs"]
mod surface;

// Internal implementation groupings. These are deliberately private in 0.7;
// consumers use Bedrock-domain modules below.
mod model;
mod codec;
mod migration;
mod audit;
mod storage;
mod edit;

/// Blocks, block states, palettes and block-entity data.
pub mod block {
    pub use crate::chunk::palette::block_storage_index;
    pub use crate::model::{BlockEntityRecord, BlockPalette, BlockPos, BlockState, ParsedBlockEntity};
}

/// Biome and height-map data stored by Bedrock chunks.
pub mod biome {
    pub use crate::model::{
        Biome2d, Biome3d, HeightMap2d, LegacyBiomeSample, ParsedBiomeData, ParsedBiomeStorage,
    };
}

/// Bedrock actor/entity records and actor-index identities.
pub mod entity {
    pub use crate::model::{
        ActorDigestKey, ActorRecord, ActorResolution, ActorSource, ActorUid, EntityData,
        ParsedActorDigest, ParsedEntity,
    };
    pub use crate::parsed::{encode_actor_digest_ids, parse_actor_digest_ids};
}

/// Bedrock player records and inventory item data.
pub mod player {
    pub use crate::model::{ItemStack, ParsedPlayer};
    pub use crate::player_impl::{PlayerData, PlayerId};
}

/// Bedrock map item records.
pub mod map {
    pub use crate::model::{MapKnownFields, MapPixels, MapRecordId, ParsedMapData};
}

/// Bedrock village database records.
pub mod village {
    pub use crate::model::{ParsedVillageData, ParsedVillageKey, VillageRecordKind};
}

/// Bedrock `.mcstructure` files and structure placement.
pub mod structure {
    pub use crate::codec::{
        McStructureBlock, McStructureFile, McStructurePaletteEntry, McStructurePlacement,
        McStructureRotation, McStructureSize, read_mcstructure_file, write_mcstructure_file,
    };
}

/// Bedrock little-endian NBT parsing, writing and borrowed views.
pub mod nbt {
    pub use crate::codec::nbt::{
        NbtEvent, NbtReader, NbtRef, NbtTag, NbtValue, NbtView, NbtWriter,
        nbt_tags_equal_for_write, parse_consecutive_root_nbt, parse_root_nbt,
        parse_root_nbt_with_consumed, serialize_root_nbt, validate_root_nbt_for_write,
        visit_nbt_events,
    };
}

/// `level.dat` document access and world-level metadata.
pub mod level {
    pub use crate::codec::level_dat::*;
}

/// Bedrock world database records, scanning and LevelDB-backed world storage.
pub mod database {
    pub use crate::storage::*;
    pub(crate) use crate::world::CancelFlag;
}

/// Explicit historical Bedrock world and BlockState upgrades.
pub mod upgrade {
    pub use crate::migration::*;
}

/// Bedrock world compatibility and integrity validation.
pub mod integrity {
    pub use crate::audit::*;
}

/// Typed policy-guarded Bedrock world editing.
pub mod editor {
    pub use crate::edit::*;
}

/// Read/query APIs for maps, regions and selections.
pub mod query;
/// High-level lazy world lifecycle, scans and transactions.
pub mod world;

// Temporary crate-private aliases for implementation files that are still being physically moved to
// the domain modules above. They are intentionally not public API and are removed as each implementation
// file is migrated; external consumers cannot use the pre-0.7 crate-root surface through these names.
pub(crate) use audit::{ChunkCapabilities, CompatibilityLevel, WritePolicy};
pub(crate) use chunk::palette::block_storage_index;
pub(crate) use codec::nbt::{NbtReader, NbtTag, NbtWriter};
pub(crate) use error::{BedrockWorldError, BedrockWorldErrorKind, Result};
pub(crate) use level as level_dat;
pub(crate) use model::*;
pub(crate) use query::WriteGuard;
pub(crate) use storage::{MemoryStorage, StorageCachePolicy, StorageReadOptions, WorldStorage};
pub(crate) use world::{
    BedrockWorld, CancelFlag, OpenOptions, SurfaceColumn, WorldChunkQueryRegion, WorldStorageHandle,
    WorldTransaction,
};
