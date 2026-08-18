//! Minecraft Bedrock player data read from `level.dat.Player`, `~local_player` or `player_<xuid>`.

use crate::error::Result;
use crate::level::LevelDatDocument;
use crate::nbt::{NbtTag, parse_root_nbt, serialize_root_nbt};
use crate::version::{GameVersion, LevelVersion};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Player identifier as stored by Minecraft Bedrock.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlayerId {
    /// Local player record stored under `~local_player`.
    Local,
    /// Xbox user id record stored under `player_<xuid>`.
    Xuid(String),
    /// Historical player data embedded in `level.dat.Player`.
    LegacyLevelDat,
    /// Player-like identifier whose Bedrock source is not known.
    Unknown(String),
}

impl PlayerId {
    /// Encodes this id as its Bedrock LevelDB key when applicable.
    #[must_use]
    pub fn storage_key(&self) -> Option<Cow<'_, [u8]>> {
        match self {
            Self::Local => Some(Cow::Borrowed(b"~local_player")),
            Self::Xuid(xuid) => Some(Cow::Owned(format!("player_{xuid}").into_bytes())),
            Self::LegacyLevelDat | Self::Unknown(_) => None,
        }
    }

    /// Detects a player id directly from a Bedrock LevelDB key.
    #[must_use]
    pub fn from_storage_key(key: &[u8]) -> Option<Self> {
        if key == b"~local_player" {
            return Some(Self::Local);
        }
        let text = std::str::from_utf8(key).ok()?;
        text.strip_prefix("player_")
            .map(|xuid| Self::Xuid(xuid.to_string()))
    }
}

/// Saved-item representation observed inside one player NBT payload.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SavedItemKind {
    /// No recognisable saved item was found.
    #[default]
    None,
    /// Historical numeric `id` representation.
    LegacyNumeric,
    /// Named item representation using `Name` or a string `id`.
    Named,
    /// Named item carrying a persisted `Block` BlockState compound.
    NamedBlockState,
    /// More than one saved-item representation exists in the same player payload.
    Mixed,
}

impl SavedItemKind {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::None, value) | (value, Self::None) => value,
            (left, right) if left == right => left,
            _ => Self::Mixed,
        }
    }
}

/// Parsed Minecraft Bedrock player record with source/version evidence retained.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerData {
    /// Bedrock player source/id.
    pub id: PlayerId,
    /// Parsed player NBT.
    pub nbt: NbtTag,
    /// Original bytes read from the source.
    pub raw: Bytes,
    /// Saved-item representation detected recursively from this payload.
    pub saved_items: SavedItemKind,
    /// `level.dat` version evidence when the owning level was available.
    pub level_version: Option<LevelVersion>,
}

impl PlayerData {
    /// Reads one player payload and detects its saved-item representation automatically.
    pub fn from_raw(id: PlayerId, raw: Bytes) -> Result<Self> {
        Self::from_raw_with_level_version(id, raw, None)
    }

    /// Reads one player payload with actual version evidence from its owning `level.dat`.
    pub fn from_raw_with_level(id: PlayerId, raw: Bytes, level: &LevelDatDocument) -> Result<Self> {
        Self::from_raw_with_level_version(id, raw, Some(LevelVersion::detect(level)?))
    }

    /// Builds one player record from structured NBT.
    pub fn from_nbt(id: PlayerId, nbt: NbtTag) -> Result<Self> {
        Self::from_nbt_with_level_version(id, nbt, None)
    }

    /// Builds one player record with actual version evidence from its owning `level.dat`.
    pub fn from_nbt_with_level(id: PlayerId, nbt: NbtTag, level: &LevelDatDocument) -> Result<Self> {
        Self::from_nbt_with_level_version(id, nbt, Some(LevelVersion::detect(level)?))
    }

    /// Returns the exact last-opened Bedrock version reported by the owning `level.dat`, when known.
    #[must_use]
    pub fn game_version(&self) -> Option<&GameVersion> {
        self.level_version
            .as_ref()
            .and_then(|version| version.last_opened_with.as_ref())
    }

    /// Serializes the current player NBT without selecting another historical representation.
    pub fn to_raw(&self) -> Result<Bytes> {
        Ok(Bytes::from(serialize_root_nbt(&self.nbt)?))
    }

    /// Re-detects the saved-item representation after the caller edits player NBT.
    pub fn refresh_saved_items(&mut self) {
        self.saved_items = detect_saved_items(&self.nbt);
    }

    pub(crate) fn from_nbt_with_level_version(
        id: PlayerId,
        nbt: NbtTag,
        level_version: Option<LevelVersion>,
    ) -> Result<Self> {
        let raw = Bytes::from(serialize_root_nbt(&nbt)?);
        let saved_items = detect_saved_items(&nbt);
        Ok(Self {
            id,
            nbt,
            raw,
            saved_items,
            level_version,
        })
    }

    pub(crate) fn from_raw_with_level_version(
        id: PlayerId,
        raw: Bytes,
        level_version: Option<LevelVersion>,
    ) -> Result<Self> {
        let nbt = parse_root_nbt(&raw)?;
        let saved_items = detect_saved_items(&nbt);
        Ok(Self {
            id,
            nbt,
            raw,
            saved_items,
            level_version,
        })
    }
}

fn detect_saved_items(nbt: &NbtTag) -> SavedItemKind {
    let mut detected = SavedItemKind::None;
    inspect_saved_items(nbt, &mut detected);
    detected
}

fn inspect_saved_items(tag: &NbtTag, detected: &mut SavedItemKind) {
    match tag {
        NbtTag::Compound(root) => {
            if let Some(kind) = saved_item_kind(root) {
                *detected = detected.merge(kind);
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

fn saved_item_kind(root: &indexmap::IndexMap<String, NbtTag>) -> Option<SavedItemKind> {
    if !matches!(
        root.get("Count"),
        Some(NbtTag::Byte(_) | NbtTag::Short(_) | NbtTag::Int(_) | NbtTag::Long(_))
    ) {
        return None;
    }
    if matches!(root.get("id"), Some(NbtTag::Byte(_) | NbtTag::Short(_) | NbtTag::Int(_) | NbtTag::Long(_))) {
        return Some(SavedItemKind::LegacyNumeric);
    }
    let named = matches!(root.get("Name"), Some(NbtTag::String(_)))
        || matches!(root.get("id"), Some(NbtTag::String(_)));
    if !named {
        return None;
    }
    if matches!(root.get("Block"), Some(NbtTag::Compound(_))) {
        Some(SavedItemKind::NamedBlockState)
    } else {
        Some(SavedItemKind::Named)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn player_key_detection_uses_real_bedrock_keys() {
        assert_eq!(PlayerId::from_storage_key(b"~local_player"), Some(PlayerId::Local));
        assert_eq!(
            PlayerId::from_storage_key(b"player_123"),
            Some(PlayerId::Xuid("123".to_string()))
        );
    }

    #[test]
    fn detects_mixed_saved_item_generations() {
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
        let player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        assert_eq!(player.saved_items, SavedItemKind::Mixed);
    }
}
