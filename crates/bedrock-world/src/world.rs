//! High-level lazy world access built on top of the storage layer.
//!
//! Existing `bedrock_world::world::*` consumers keep the same API while implementation code is moved
//! into responsibility-oriented modules under `world/`.

#[path = "world/access.rs"]
mod implementation;

pub mod chunk_io;
pub mod open;
pub mod scan;
pub mod terrain;
pub mod transaction;

pub use implementation::*;
