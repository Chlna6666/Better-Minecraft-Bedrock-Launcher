//! Exact non-rectangular chunk selection and selection statistics.
//!
//! Implementation lives with the rest of query code under `query/selection.rs`.

#[path = "query/selection.rs"]
mod implementation;

pub use implementation::*;
