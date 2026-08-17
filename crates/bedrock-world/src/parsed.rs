//! Structured parsers layered above raw Bedrock world records.
//!
//! Parser implementation lives under `parsed/` so model definitions, decoders and report building can
//! be split without changing the historical `bedrock_world::parsed::*` API.

#[path = "parsed/impl.rs"]
mod implementation;

pub use implementation::*;

/// Parsed semantic record/model types.
pub mod model {
    pub use super::{
        ActorRecord, ActorResolution, ActorSource, Biome2d, Biome3d, BlockEntityRecord,
        HardcodedSpawnAreaKind, HeightMap2d, ItemStack, MapKnownFields, MapPixels,
        ParsedActorDigest, ParsedBiomeData, ParsedBiomeStorage, ParsedBlockEntity, ParsedChunkData,
        ParsedChunkRecord, ParsedChunkRecordValue, ParsedDbEntry, ParsedDbValue, ParsedEntity,
        ParsedGlobalData, ParsedHardcodedSpawnArea, ParsedMapData, ParsedPlayer, ParsedVillageData,
        ParsedWorld,
    };
}

/// Whole-world parse policy and reporting types.
pub mod report {
    pub use super::{RetentionMode, WorldParseCategories, WorldParseOptions, WorldParseReport};
}
