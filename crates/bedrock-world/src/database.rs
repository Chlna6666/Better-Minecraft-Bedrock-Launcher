//! Minecraft Bedrock world database access.
//!
//! This module owns Bedrock world record scanning and database adapters while delegating Mojang
//! LevelDB engine mechanics to `bedrock-leveldb`.

mod storage;

pub use storage::*;
pub use storage::backend::BedrockLevelDbStorage;
pub(crate) use crate::world::CancelFlag;
