//! Player identifiers and raw player record helpers.
//!
//! Implementation lives under `model/player.rs`; the root module remains a compatibility facade.

#[path = "model/player.rs"]
mod implementation;

pub use implementation::*;
