//! Bedrock chunk facade.
//!
//! The implementation lives under `chunk/` so the historical key/model/palette/legacy/subchunk
//! responsibilities can be split incrementally without changing the existing public `chunk::*` API.

#[path = "chunk/impl.rs"]
mod implementation;

pub use implementation::*;
