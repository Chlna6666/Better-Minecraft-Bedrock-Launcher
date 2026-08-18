//! Explicit `level.dat` conversion with preservation guards.

use crate::error::{BedrockWorldError, Result};
use crate::level::LevelDatDocument;
use crate::nbt::NbtTag;

/// Explicit target options for `level.dat` conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelDatConversionOptions {
    /// Optional target header version. `None` preserves the source header version.
    pub target_header_version: Option<u32>,
    /// Existing map seed must remain byte-semantically unchanged when true.
    pub preserve_existing_seed: bool,
}

impl Default for LevelDatConversionOptions {
    fn default() -> Self {
        Self {
            target_header_version: None,
            preserve_existing_seed: true,
        }
    }
}

/// Applies a caller-supplied explicit conversion while preserving `level.dat` safety invariants.
///
/// Normal reads and writes never invoke this function. Unknown root fields are retained because the
/// source document is cloned and mutated in place. The library does not invent version-specific field
/// rewrites; callers compose authoritative conversion rules in `transform`.
pub fn convert_level_dat_document<F>(
    document: &LevelDatDocument,
    options: LevelDatConversionOptions,
    transform: F,
) -> Result<LevelDatDocument>
where
    F: FnOnce(&mut NbtTag) -> Result<()>,
{
    let before_seed = document.random_seed()?;
    let mut converted = document.clone();
    transform(&mut converted.root)?;
    if let Some(version) = options.target_header_version {
        converted.header.version = version;
    }
    if options.preserve_existing_seed {
        let after_seed = converted.random_seed()?;
        if before_seed.is_some() && before_seed != after_seed {
            return Err(BedrockWorldError::Validation(format!(
                "level.dat conversion attempted to change existing RandomSeed from {before_seed:?} to {after_seed:?}"
            )));
        }
    }
    Ok(converted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn conversion_rejects_seed_change_by_default() {
        let document = LevelDatDocument::new(
            10,
            NbtTag::Compound(IndexMap::from([(
                "RandomSeed".to_string(),
                NbtTag::Long(123),
            )])),
        );
        let result = convert_level_dat_document(
            &document,
            LevelDatConversionOptions::default(),
            |root| {
                let NbtTag::Compound(root) = root else { unreachable!() };
                root.insert("RandomSeed".to_string(), NbtTag::Long(456));
                Ok(())
            },
        );
        assert!(result.is_err());
    }
}
