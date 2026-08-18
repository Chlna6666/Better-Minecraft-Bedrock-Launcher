//! `level.dat` document access, atomic writes and version-aware metadata migration.

mod document;
/// Explicit `level.dat` migration with preservation guards.
pub mod migration;

pub use document::*;
pub use migration::{LevelDatMigrationOptions, migrate_level_dat_document};
