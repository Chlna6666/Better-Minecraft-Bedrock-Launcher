//! Minecraft Bedrock world database access.
//!
//! This module owns Bedrock world record scanning and database adapters while delegating Mojang
//! LevelDB engine mechanics to `bedrock-leveldb`.

#[path = "database/storage.rs"]
mod implementation;

pub mod adapters;
pub mod core;
pub mod memory;
pub mod pipeline;

pub use implementation::*;
pub(crate) use crate::world::CancelFlag;
