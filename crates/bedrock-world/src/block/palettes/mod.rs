//! Read-only query API for runtime block-palette dumps.
//!
//! The data comes from `pmmp/bds-mod-mapping` runtime dumps, archived in
//! `opencollab-incubator/BedrockBlockPaletteArchive`. Each state's index in a
//! dump is its network runtime ID for that game version. States carry exact
//! typed NBT properties and the persisted NBT storage version.
//!
//! This is runtime registry evidence. It has no default-state field and no
//! legacy numeric ID; version labels are preserved verbatim (many are beta
//! builds, not four-component stable versions). It must not be used to write
//! worlds or drive destructive migration. See
//! [`crate::block::version`] for the migration path.

mod runtime;

pub use runtime::{
    RuntimePalette, RuntimePaletteState, RuntimePaletteValue, runtime_palette_for,
    runtime_palette_state, runtime_palette_versions,
};
