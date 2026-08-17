//! Storage abstraction used by `bedrock-world`.
//!
//! World-level raw record access is separated from Minecraft semantics and Mojang LevelDB internals.
//! The compatibility implementation remains under `storage/impl.rs` while public responsibilities are
//! exposed through dedicated child modules.

#[path = "storage/impl.rs"]
mod implementation;

pub mod adapters;
pub mod core;
pub mod memory;
pub mod pipeline;

pub use implementation::*;
