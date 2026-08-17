//! Storage abstraction used by `bedrock-world`.
//!
//! World-level raw record access is separated from Minecraft semantics and Mojang LevelDB internals.
//! Public consumers use this module instead of depending on `bedrock-leveldb` details directly.

#[path = "storage/impl.rs"]
mod implementation;

pub mod adapters;
pub mod core;
pub mod memory;
pub mod pipeline;

pub use implementation::*;
