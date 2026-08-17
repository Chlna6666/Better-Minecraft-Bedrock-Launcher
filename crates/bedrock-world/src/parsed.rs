//! Structured parsers layered above raw Bedrock world records.
//!
//! Parser implementation remains behind a compatibility facade while semantic models and parse-report
//! policy have stable responsibility entry points under `parsed/`.

#[path = "parsed/impl.rs"]
mod implementation;

pub mod model;
pub mod report;

pub use implementation::*;
