//! Filesystem discovery for Minecraft Bedrock world folders.
//!
//! Implementation lives under `world/discover.rs`; the root module remains a compatibility facade.

#[path = "world/discover.rs"]
mod implementation;

pub use implementation::*;
