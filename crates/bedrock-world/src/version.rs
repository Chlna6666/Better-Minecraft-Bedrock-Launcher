//! Minecraft Bedrock game and persisted version values read from world data.

use crate::error::{BedrockWorldError, Result};
use crate::level::LevelDatDocument;
use crate::nbt::NbtTag;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Minecraft Bedrock game version components read from persisted world metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GameVersion {
    components: Vec<i32>,
}

impl GameVersion {
    /// Creates a game version from the exact component list stored by Bedrock.
    pub fn new(components: Vec<i32>) -> Result<Self> {
        if components.is_empty() {
            return Err(BedrockWorldError::Validation(
                "Bedrock game version component list cannot be empty".to_string(),
            ));
        }
        if components.iter().any(|value| *value < 0) {
            return Err(BedrockWorldError::Validation(
                "Bedrock game version components cannot be negative".to_string(),
            ));
        }
        Ok(Self { components })
    }

    /// Returns the exact persisted version components without normalising their length.
    #[must_use]
    pub fn components(&self) -> &[i32] {
        &self.components
    }
}

impl fmt::Display for GameVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, component) in self.components.iter().enumerate() {
            if index != 0 {
                formatter.write_str(".")?;
            }
            write!(formatter, "{component}")?;
        }
        Ok(())
    }
}

/// Version values read from one `level.dat` document.
///
/// These fields are independent persisted values. The library does not derive one from another and
/// does not use them to mutate world data while reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelVersion {
    /// Binary `level.dat` header version.
    pub header_version: u32,
    /// `StorageVersion` NBT value when present.
    pub storage_version: Option<i32>,
    /// Exact `lastOpenedWithVersion`/`LastOpenedWithVersion` value when present.
    pub last_opened_with: Option<GameVersion>,
    /// Exact `MinimumCompatibleClientVersion` value when present.
    pub minimum_compatible_client_version: Option<GameVersion>,
    /// Exact `InventoryVersion` string when present.
    pub inventory_version: Option<String>,
}

impl LevelVersion {
    /// Reads version values directly from a parsed `level.dat` document.
    pub fn detect(document: &LevelDatDocument) -> Result<Self> {
        let NbtTag::Compound(root) = &document.root else {
            return Err(BedrockWorldError::CorruptWorld(
                "level.dat root is not a compound".to_string(),
            ));
        };

        let storage_version = integer_field(root.get("StorageVersion"), "StorageVersion")?;

        let lower = root
            .get("lastOpenedWithVersion")
            .map(|tag| game_version_from_tag(tag, "lastOpenedWithVersion"))
            .transpose()?;
        let upper = root
            .get("LastOpenedWithVersion")
            .map(|tag| game_version_from_tag(tag, "LastOpenedWithVersion"))
            .transpose()?;
        if let (Some(lower), Some(upper)) = (&lower, &upper) {
            if lower != upper {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "level.dat contains conflicting last-opened game versions {lower} and {upper}"
                )));
            }
        }

        let minimum_compatible_client_version = root
            .get("MinimumCompatibleClientVersion")
            .map(|tag| game_version_from_tag(tag, "MinimumCompatibleClientVersion"))
            .transpose()?;

        let inventory_version = match root.get("InventoryVersion") {
            Some(NbtTag::String(value)) => Some(value.clone()),
            Some(other) => {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "level.dat InventoryVersion has unexpected NBT type: {other:?}"
                )));
            }
            None => None,
        };

        Ok(Self {
            header_version: document.header.version,
            storage_version,
            last_opened_with: lower.or(upper),
            minimum_compatible_client_version,
            inventory_version,
        })
    }
}

fn integer_field(tag: Option<&NbtTag>, field: &str) -> Result<Option<i32>> {
    let Some(tag) = tag else {
        return Ok(None);
    };
    let value = match tag {
        NbtTag::Byte(value) => i32::from(*value),
        NbtTag::Short(value) => i32::from(*value),
        NbtTag::Int(value) => *value,
        NbtTag::Long(value) => i32::try_from(*value).map_err(|_| {
            BedrockWorldError::CorruptWorld(format!(
                "level.dat {field} value {value} does not fit i32"
            ))
        })?,
        other => {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "level.dat {field} has unexpected NBT type: {other:?}"
            )));
        }
    };
    Ok(Some(value))
}

fn game_version_from_tag(tag: &NbtTag, field: &str) -> Result<GameVersion> {
    let NbtTag::List(values) = tag else {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "level.dat {field} has unexpected NBT type: {tag:?}"
        )));
    };
    let mut components = Vec::with_capacity(values.len());
    for value in values {
        let component = match value {
            NbtTag::Byte(value) => i32::from(*value),
            NbtTag::Short(value) => i32::from(*value),
            NbtTag::Int(value) => *value,
            NbtTag::Long(value) => i32::try_from(*value).map_err(|_| {
                BedrockWorldError::CorruptWorld(format!(
                    "level.dat {field} component {value} does not fit i32"
                ))
            })?,
            other => {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "level.dat {field} contains non-integer component: {other:?}"
                )));
            }
        };
        components.push(component);
    }
    GameVersion::new(components)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn reads_independent_level_version_values() {
        let document = LevelDatDocument::new(
            10,
            NbtTag::Compound(IndexMap::from([
                ("StorageVersion".to_string(), NbtTag::Int(8)),
                (
                    "lastOpenedWithVersion".to_string(),
                    NbtTag::List(vec![
                        NbtTag::Int(1),
                        NbtTag::Int(26),
                        NbtTag::Int(40),
                        NbtTag::Int(5),
                    ]),
                ),
                (
                    "MinimumCompatibleClientVersion".to_string(),
                    NbtTag::List(vec![
                        NbtTag::Int(1),
                        NbtTag::Int(21),
                        NbtTag::Int(0),
                        NbtTag::Int(0),
                    ]),
                ),
                (
                    "InventoryVersion".to_string(),
                    NbtTag::String("1.21.0".to_string()),
                ),
            ])),
        );
        let version = LevelVersion::detect(&document).unwrap();
        assert_eq!(version.storage_version, Some(8));
        assert_eq!(
            version.last_opened_with.unwrap().components(),
            &[1, 26, 40, 5]
        );
        assert_eq!(
            version.minimum_compatible_client_version.unwrap().components(),
            &[1, 21, 0, 0]
        );
        assert_eq!(version.inventory_version.as_deref(), Some("1.21.0"));
    }
}
