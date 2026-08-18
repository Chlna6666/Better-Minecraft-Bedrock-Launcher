//! Persisted Minecraft Bedrock player-data format detection.
//!
//! Player NBT does not expose one universal player schema byte. Detection therefore reports only
//! evidence that is actually present: the player storage location, saved-item representation and
//! optional `level.dat` version information supplied by the caller.

use crate::nbt::NbtTag;
use crate::player::PlayerId;
use crate::version::{GameVersion, LevelVersion};
use serde::{Deserialize, Serialize};

/// Physical location used by a persisted Bedrock player record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlayerStorage {
    /// Historical `Player` compound embedded in `level.dat`.
    LevelDat,
    /// Local player stored under the `~local_player` LevelDB key.
    Local,
    /// Server/Xbox player stored under a `player_<xuid>` LevelDB key.
    Server,
    /// A player-like value whose storage location is not known.
    Unknown,
}

impl PlayerStorage {
    /// Returns the storage location implied by a player identifier.
    #[must_use]
    pub const fn from_player_id(id: &PlayerId) -> Self {
        match id {
            PlayerId::LegacyLevelDat => Self::LevelDat,
            PlayerId::Local => Self::Local,
            PlayerId::Xuid(_) => Self::Server,
            PlayerId::Unknown(_) => Self::Unknown,
        }
    }
}

/// Saved-item representation observed inside player NBT.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SavedItemFormat {
    /// No recognisable saved item stack was found.
    #[default]
    None,
    /// Historical numeric item `id` stored as an integer tag.
    LegacyNumeric,
    /// Named item representation using `Name` or a string `id`.
    Named,
    /// Named item carrying a persisted `Block` BlockState compound.
    NamedBlockState,
    /// Multiple saved-item representations occur in the same player payload.
    Mixed,
}

impl SavedItemFormat {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::None, value) | (value, Self::None) => value,
            (left, right) if left == right => left,
            _ => Self::Mixed,
        }
    }
}

/// Automatically detected persisted format information for one player payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerDataFormat {
    /// Physical storage location inferred from the player key/source.
    pub storage: PlayerStorage,
    /// Saved-item representation observed recursively in the player NBT.
    pub saved_items: SavedItemFormat,
    /// World/level version evidence when a `level.dat` context was supplied.
    pub level_version: Option<LevelVersion>,
}

impl PlayerDataFormat {
    /// Detects player-data format without converting or rewriting the NBT.
    #[must_use]
    pub fn detect(id: &PlayerId, nbt: &NbtTag, level_version: Option<LevelVersion>) -> Self {
        let mut saved_items = SavedItemFormat::None;
        inspect_saved_items(nbt, &mut saved_items);
        Self {
            storage: PlayerStorage::from_player_id(id),
            saved_items,
            level_version,
        }
    }

    /// Returns the exact last-opened game version supplied by `level.dat`, when known.
    #[must_use]
    pub fn game_version(&self) -> Option<&GameVersion> {
        self.level_version
            .as_ref()
            .and_then(|version| version.last_opened_with.as_ref())
    }
}

fn inspect_saved_items(tag: &NbtTag, detected: &mut SavedItemFormat) {
    match tag {
        NbtTag::Compound(root) => {
            if let Some(format) = saved_item_format(root) {
                *detected = detected.merge(format);
            }
            for value in root.values() {
                inspect_saved_items(value, detected);
            }
        }
        NbtTag::List(values) => {
            for value in values {
                inspect_saved_items(value, detected);
            }
        }
        _ => {}
    }
}

fn saved_item_format(root: &indexmap::IndexMap<String, NbtTag>) -> Option<SavedItemFormat> {
    if !matches!(
        root.get("Count"),
        Some(NbtTag::Byte(_) | NbtTag::Short(_) | NbtTag::Int(_) | NbtTag::Long(_))
    ) {
        return None;
    }

    if matches!(root.get("id"), Some(NbtTag::Short(_) | NbtTag::Int(_))) {
        return Some(SavedItemFormat::LegacyNumeric);
    }

    let named = matches!(root.get("Name"), Some(NbtTag::String(_)))
        || matches!(root.get("id"), Some(NbtTag::String(_)));
    if !named {
        return None;
    }
    if matches!(root.get("Block"), Some(NbtTag::Compound(_))) {
        Some(SavedItemFormat::NamedBlockState)
    } else {
        Some(SavedItemFormat::Named)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn detects_mixed_numeric_and_named_saved_items() {
        let nbt = NbtTag::Compound(IndexMap::from([(
            "Inventory".to_string(),
            NbtTag::List(vec![
                NbtTag::Compound(IndexMap::from([
                    ("id".to_string(), NbtTag::Short(1)),
                    ("Count".to_string(), NbtTag::Byte(1)),
                ])),
                NbtTag::Compound(IndexMap::from([
                    ("Name".to_string(), NbtTag::String("minecraft:stone".to_string())),
                    ("Count".to_string(), NbtTag::Byte(1)),
                    ("Block".to_string(), NbtTag::Compound(IndexMap::new())),
                ])),
            ]),
        )]));
        let format = PlayerDataFormat::detect(&PlayerId::Local, &nbt, None);
        assert_eq!(format.storage, PlayerStorage::Local);
        assert_eq!(format.saved_items, SavedItemFormat::Mixed);
    }
}
