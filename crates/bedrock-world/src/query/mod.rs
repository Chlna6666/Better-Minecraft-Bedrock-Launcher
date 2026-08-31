//! Minecraft Bedrock world inspection and overlay queries.
//!
//! Query operations are grouped by their actual implementation ownership. Exact non-rectangular
//! selections are an independent responsibility and therefore keep a dedicated child module.

mod operations;
pub mod selection;

pub(crate) use crate::parsed::ParsedChunkRecordValue;
pub use operations::*;
pub use selection::*;
