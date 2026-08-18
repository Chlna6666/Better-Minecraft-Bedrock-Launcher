//! Safe `level.dat` migration guards.

use crate::level::LevelDatDocument;
use crate::nbt::NbtTag;
use crate::error::{BedrockWorldError, Result};

/// Explicit target for a `level.dat` migration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LevelDatMigrationOptions {
    /// Optional target header version. `None` preserves the source header version.
    pub target_header_version: Option<u32>,
    /// Existing map seed must remain byte-semantically unchanged when true.
    pub preserve_existing_seed: bool,
}

/// Applies a caller-supplied semantic migration while preserving `level.dat` safety invariants.
///
/// Unknown root fields are retained because the original document is cloned and mutated in place.
/// The library does not invent version-specific field rewrites; callers compose authoritative rules
/// in `transform`. Existing `RandomSeed` is protected by default.
pub fn migrate_level_dat_document<F>(
    document: &LevelDatDocument,
    options: LevelDatMigrationOptions,
    transform: F,
) -> Result<LevelDatDocument>
where
    F: FnOnce(&mut NbtTag) -> Result<()>,
{
    let before_seed = document.random_seed()?;
    let mut migrated = document.clone();
    transform(&mut migrated.root)?;
    if let Some(version) = options.target_header_version {
        migrated.header.version = version;
    }
    if options.preserve_existing_seed {
        let after_seed = migrated.random_seed()?;
        if before_seed.is_some() && before_seed != after_seed {
            return Err(BedrockWorldError::Validation(format!(
                "level.dat migration attempted to change existing RandomSeed from {before_seed:?} to {after_seed:?}"
            )));
        }
    }
    Ok(migrated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn migration_rejects_seed_change() {
        let document = LevelDatDocument::new(
            10,
            NbtTag::Compound(IndexMap::from([(
                "RandomSeed".to_string(),
                NbtTag::Long(123),
            )])),
        );
        let result = migrate_level_dat_document(
            &document,
            LevelDatMigrationOptions {
                target_header_version: Some(11),
                preserve_existing_seed: true,
            },
            |root| {
                let NbtTag::Compound(root) = root else { unreachable!() };
                root.insert("RandomSeed".to_string(), NbtTag::Long(456));
                Ok(())
            },
        );
        assert!(result.is_err());
    }
}
