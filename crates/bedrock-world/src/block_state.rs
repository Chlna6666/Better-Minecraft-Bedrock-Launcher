//! Canonical semantic identity helpers for Bedrock BlockState values.
//!
//! Implementation lives under `model/block_state.rs`; the root module remains a compatibility facade.

#[path = "model/block_state.rs"]
mod implementation;

pub use implementation::*;
