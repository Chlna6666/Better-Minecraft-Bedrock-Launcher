//! `level.dat` document access, atomic writes and explicit version conversion.

mod document;
/// Explicit caller-requested `level.dat` conversion.
pub mod conversion;

pub use conversion::{LevelDatConversionOptions, convert_level_dat_document};
pub use document::*;
