//! Terrain surface-role helpers shared by chunk decoding and render sampling.
//!
//! Implementation lives under `model/surface.rs`; the root module remains a compatibility facade.

#[path = "model/surface.rs"]
mod implementation;

pub use implementation::*;
