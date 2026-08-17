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
