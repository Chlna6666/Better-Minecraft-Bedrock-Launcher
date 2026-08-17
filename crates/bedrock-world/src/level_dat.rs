//! `level.dat` parsing, validation and atomic write helpers.
//!
//! Implementation lives under `codec/level_dat/`; the root module remains a compatibility facade.

#[path = "codec/level_dat/impl.rs"]
mod implementation;

pub use implementation::*;
