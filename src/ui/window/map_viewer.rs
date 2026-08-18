mod actions;
mod bottom_panel;
mod canvas;
mod editor;
#[cfg(debug_assertions)]
mod entity_debug_paint;
mod exact_selection_ops;
mod helpers;
mod history_panel;
mod import_preview;
mod interactions;
mod layout;
mod lifecycle;
pub(crate) mod map_history;
mod mcstructure;
mod menu_overlay;
mod menus;
mod model;
mod overlays;
mod paint;
mod panels;
mod player_item_menu;
mod player_panel;
mod player_workspace;
mod players;
mod prelude;

// Private current-domain import surface for large map-viewer compilation units. This does not expose
// or restore removed bedrock-world crate-root APIs: every binding originates from a public 0.7 domain.
pub(super) mod bedrock_world_domains {
    pub(super) use ::bedrock_world::biome::{
        Biome2d, Biome3d, HeightMap2d, LegacyBiomeSample, ParsedBiomeStorage,
    };
    pub(super) use ::bedrock_world::block::{
        BlockEntityRecord, BlockPalette, BlockPos, BlockState, ParsedBlockEntity,
        block_storage_index,
    };
    pub(super) use ::bedrock_world::chunk::{
        Chunk, ChunkKey, ChunkPos, ChunkRecord, ChunkRecordTag, ChunkVersion, Dimension,
        HardcodedSpawnAreaKind, LegacyTerrain, ParsedChunkData, ParsedChunkRecord,
        ParsedChunkRecordValue, ParsedHardcodedSpawnArea, SubChunk, SubChunkDecodeMode,
        SubChunkFormat,
    };
    pub(super) use ::bedrock_world::database::{
        BedrockDbKey, BedrockLevelDbStorage, MemoryStorage, PartitionedWorldStorage, StorageBatch,
        StorageCachePolicy, StorageCancelFlag, StorageEntry, StorageEntryRef, StorageOp,
        StoragePipelineOptions, StorageProgressSink, StorageReadOptions, StorageScanMode,
        StorageScanOutcome, StorageScanProgress, StorageThreadingOptions, StorageVisitorControl,
        WorldStorage,
    };
    pub(super) use ::bedrock_world::entity::{
        ActorDigestKey, ActorRecord, ActorResolution, ActorSource, ActorUid, ParsedEntity,
    };
    pub(super) use ::bedrock_world::error::{BedrockWorldError, BedrockWorldErrorKind, Result};
    pub(super) use ::bedrock_world::map::{MapKnownFields, MapPixels, MapRecordId, ParsedMapData};
    pub(super) use ::bedrock_world::nbt::NbtTag;
    pub(super) use ::bedrock_world::player::{PlayerData, PlayerId};
    pub(super) use ::bedrock_world::query::*;
    pub(super) use ::bedrock_world::structure::{
        McStructureBlock, McStructureFile, McStructurePaletteEntry, McStructurePlacement,
        McStructureRotation, McStructureSize, read_mcstructure_file, write_mcstructure_file,
    };
    pub(super) use ::bedrock_world::editor::McStructureWritePhase;
    pub(super) use ::bedrock_world::world::*;

    pub(super) mod biome {
        pub(super) use ::bedrock_world::biome::*;
    }
    pub(super) mod block {
        pub(super) use ::bedrock_world::block::*;
    }
    pub(super) mod chunk {
        pub(super) use ::bedrock_world::chunk::*;
    }
    pub(super) mod database {
        pub(super) use ::bedrock_world::database::*;
    }
    pub(super) mod entity {
        pub(super) use ::bedrock_world::entity::*;
    }
    pub(super) mod error {
        pub(super) use ::bedrock_world::error::*;
    }
    pub(super) mod level {
        pub(super) use ::bedrock_world::level::*;
    }
    pub(super) mod map {
        pub(super) use ::bedrock_world::map::*;
    }
    pub(super) mod nbt {
        pub(super) use ::bedrock_world::nbt::*;
    }
    pub(super) mod parsed {
        pub(super) use ::bedrock_world::chunk::{
            ParsedChunkData, ParsedChunkRecord, ParsedChunkRecordValue,
            parse_chunk_records, parse_chunk_records_with_options,
        };
    }
    pub(super) mod player {
        pub(super) use ::bedrock_world::player::*;
    }
    pub(super) mod query {
        pub(super) use ::bedrock_world::query::*;
    }
    pub(super) mod structure {
        pub(super) use ::bedrock_world::structure::*;
    }
    pub(super) mod world {
        pub(super) use ::bedrock_world::world::*;
    }
}

// The 3D preview and tile renderer are split by responsibility rather than by migration generation.
// These large source units bind their historical local crate name to the current-domain surface above.
mod preview_3d {
    use super::bedrock_world_domains as bedrock_world;
    include!("map_viewer/preview_3d.rs");
}
mod preview_3d_obj;
mod preview_3d_source {
    use super::bedrock_world_domains as bedrock_world;
    include!("map_viewer/preview_3d_source.rs");
}
mod preview_panel;
mod preview_panel_render;
mod professional_panel;
mod query_cache;
mod region_package;
mod right_panel;
mod selection;
mod state;
mod status_bar;
#[cfg(test)]
mod r#tests;
mod tile_cache;
mod tile_occupancy;
mod tile_plan;
mod tile_render;
mod tile_render_composite;
mod tile_render_core;
mod tile_state;
mod tool_stripe;
mod top_bar;
mod view;
mod viewport;

pub use actions::init;
pub use model::MapViewerWindowInit;
pub use view::open_map_viewer_window;