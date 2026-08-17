//! Tools for inspecting, migrating and editing Minecraft Bedrock worlds.
//!
//! `bedrock-world` owns Minecraft Bedrock world semantics. Mojang LevelDB mechanics belong exclusively
//! to `bedrock-leveldb`. The 0.7 API is intentionally breaking: consumers use responsibility modules
//! instead of crate-root type/function re-exports.

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

#[cfg(feature = "backend-bedrock-leveldb")]
mod bedrock_leveldb {
    pub use ::bedrock_leveldb::access::*;
    pub use ::bedrock_leveldb::engine::*;
    pub use ::bedrock_leveldb::error::*;
    pub use ::bedrock_leveldb::format::*;
}

#[path = "model/block_state.rs"]
mod block_state;
mod chunk;
#[path = "world/discover.rs"]
mod discover;
/// Crate-wide Bedrock world error types.
pub mod error;
mod mcstructure;
mod nbt_ref;
mod parsed;
#[path = "model/player.rs"]
mod player;
mod selection_query;
#[path = "model/surface.rs"]
mod surface;

/// Semantic Minecraft Bedrock models.
pub mod model;
/// Binary and NBT codecs. Codecs do not choose migration or write policy.
pub mod codec;
/// Historical schema/format migration.
pub mod migration;
/// Typed policy-guarded mutation APIs.
pub mod edit;
/// Compatibility and integrity auditing.
pub mod audit;
/// Read/query APIs for maps, regions and selections.
pub mod query;
/// Minecraft world storage abstraction and LevelDB adapter.
pub mod storage;
/// High-level lazy world lifecycle, scans and transactions.
pub mod world;

// Implementation-only migration surface. These names are crate-private, so the removed pre-0.7
// crate-root API is not available to external consumers. They disappear as the remaining large
// implementation files are physically split into their responsibility modules.
pub(crate) use audit::{
    ActorStorageModel, ChunkCapabilities, CompatibilityLevel, SubChunkCodecKind, WorldCapabilities,
    WritePolicy,
};
pub(crate) use codec::{
    NbtReader, NbtTag, NbtWriter, block_storage_index, level_dat, nbt,
};
pub(crate) use edit::block_edit;
pub(crate) use error::{BedrockWorldError, BedrockWorldErrorKind, Result};
pub(crate) use migration::{
    BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader, block_state_graph,
    block_state_upgrade, historical_chunk, legacy_import,
};
pub(crate) use model::*;
pub(crate) use query::*;
pub(crate) use selection_query::*;
pub(crate) use storage::*;
pub(crate) use world::*;
