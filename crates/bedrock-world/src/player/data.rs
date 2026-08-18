//! Minecraft Bedrock player data read from `level.dat.Player`, `~local_player` or `player_<id>`.

use crate::error::{BedrockWorldError, Result};
use crate::level::LevelDatDocument;
use crate::nbt::{NbtTag, parse_root_nbt, serialize_root_nbt};
use crate::version::{GameVersion, LevelVersion};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Player identifier used by the typed player convenience APIs.
///
/// Arbitrary raw `player_*` suffix bytes are intentionally represented by [`crate::player::PlayerKeyRecord`]
/// and [`crate::player::PlayerStorageOverview`] rather than being forced through this textual enum.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlayerId {
    /// Local player record stored under `~local_player`.
    Local,
    /// Player record stored under a textual `player_<id>` LevelDB key.
    ///
    /// The historical variant name is retained for source compatibility. The library does not assume
    /// every textual suffix is actually an Xbox XUID.
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
            Self::Xuid(id) => Some(Cow::Owned(format!("player_{id}").into_bytes())),
            Self::LegacyLevelDat | Self::Unknown(_) => None,
        }
    }

    /// Detects a textual player id directly from a Bedrock LevelDB key.
    ///
    /// Returns `None` for non-UTF-8 `player_*` suffixes. Use the raw-key player APIs when arbitrary
    /// suffix bytes must be retained.
    #[must_use]
    pub fn from_storage_key(key: &[u8]) -> Option<Self> {
        if key == b"~local_player" {
            return Some(Self::Local);
        }
        let suffix = key.strip_prefix(b"player_")?;
        if suffix.is_empty() {
            return None;
        }
        std::str::from_utf8(suffix)
            .ok()
            .map(|id| Self::Xuid(id.to_string()))
    }

    /// Returns the textual suffix of a `player_<id>` key when this is such a record.
    #[must_use]
    pub fn player_key_id(&self) -> Option<&str> {
        match self {
            Self::Xuid(id) => Some(id),
            _ => None,
        }
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
///
/// The original NBT and bytes are retained so an unchanged record can be written byte-for-byte.
/// Editing does not select or apply another historical representation automatically. Constructors
/// normally interpret their NBT argument as the player record itself; the level-aware convenience
/// constructor has one narrow compatibility case for the exact owning `LevelDatDocument::root`.
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
    original_nbt: NbtTag,
}

impl PlayerData {
    /// Reads one exact player payload and detects its saved-item representation automatically.
    pub fn from_raw(id: PlayerId, raw: Bytes) -> Result<Self> {
        Self::from_raw_with_level_version(id, raw, None)
    }

    /// Reads one exact player payload with actual version evidence from its owning `level.dat`.
    pub fn from_raw_with_level(id: PlayerId, raw: Bytes, level: &LevelDatDocument) -> Result<Self> {
        Self::from_raw_with_level_version(id, raw, Some(LevelVersion::detect(level)?))
    }

    /// Reads a known textual Bedrock player LevelDB key and its NBT payload.
    ///
    /// Returns `Ok(None)` for keys other than `~local_player` and UTF-8 `player_<id>` records. Use
    /// [`crate::player::read_player_key`] for arbitrary raw `player_*` suffixes.
    pub fn from_leveldb_key(key: &[u8], raw: Bytes) -> Result<Option<Self>> {
        let Some(id) = PlayerId::from_storage_key(key) else {
            return Ok(None);
        };
        Self::from_raw(id, raw).map(Some)
    }

    /// Reads a known textual Bedrock player LevelDB key with version evidence from its owning `level.dat`.
    ///
    /// Returns `Ok(None)` for keys other than `~local_player` and UTF-8 `player_<id>` records.
    pub fn from_leveldb_key_with_level(
        key: &[u8],
        raw: Bytes,
        level: &LevelDatDocument,
    ) -> Result<Option<Self>> {
        let Some(id) = PlayerId::from_storage_key(key) else {
            return Ok(None);
        };
        Self::from_raw_with_level(id, raw, level).map(Some)
    }

    /// Builds one player record from structured NBT.
    pub fn from_nbt(id: PlayerId, nbt: NbtTag) -> Result<Self> {
        Self::from_nbt_with_level_version(id, nbt, None)
    }

    /// Builds one player record with actual version evidence from its owning `level.dat`.
    ///
    /// Ordinarily `nbt` is interpreted exactly as the player record. For the historical
    /// [`PlayerId::LegacyLevelDat`] convenience path only, when `nbt` is exactly equal to the supplied
    /// `level.root`, this method extracts that document's `Player` compound. It does not unwrap an
    /// arbitrary compound merely because a field named `Player` exists.
    pub fn from_nbt_with_level(
        id: PlayerId,
        nbt: NbtTag,
        level: &LevelDatDocument,
    ) -> Result<Self> {
        let nbt = if matches!(id, PlayerId::LegacyLevelDat) && nbt == level.root {
            let NbtTag::Compound(root) = &level.root else {
                return Err(BedrockWorldError::CorruptWorld(
                    "level.dat root is not an NBT compound".to_string(),
                ));
            };
            match root.get("Player") {
                Some(NbtTag::Compound(_)) => root["Player"].clone(),
                Some(other) => {
                    return Err(BedrockWorldError::CorruptWorld(format!(
                        "level.dat Player field has unexpected NBT type: {other:?}"
                    )));
                }
                None => {
                    return Err(BedrockWorldError::CorruptWorld(
                        "level.dat does not contain a Player compound".to_string(),
                    ));
                }
            }
        } else {
            nbt
        };
        Self::from_nbt_with_level_version(id, nbt, Some(LevelVersion::detect(level)?))
    }

    /// Returns the exact last-opened Bedrock version reported by the owning `level.dat`, when known.
    #[must_use]
    pub fn game_version(&self) -> Option<&GameVersion> {
        self.level_version
            .as_ref()
            .and_then(|version| version.last_opened_with.as_ref())
    }

    /// Returns the exact `MinimumCompatibleClientVersion` reported by `level.dat`, when known.
    #[must_use]
    pub fn minimum_compatible_client_version(&self) -> Option<&GameVersion> {
        self.level_version
            .as_ref()
            .and_then(|version| version.minimum_compatible_client_version.as_ref())
    }

    /// Returns the exact `InventoryVersion` reported by `level.dat`, when known.
    #[must_use]
    pub fn inventory_version(&self) -> Option<&str> {
        self.level_version
            .as_ref()
            .and_then(|version| version.inventory_version.as_deref())
    }

    /// Returns the player's literal `format_version` NBT value when present.
    pub fn format_version(&self) -> Result<Option<&str>> {
        let root = self.root()?;
        match root.get("format_version") {
            Some(NbtTag::String(value)) => Ok(Some(value)),
            Some(other) => Err(BedrockWorldError::CorruptWorld(format!(
                "player format_version has unexpected NBT type: {other:?}"
            ))),
            None => Ok(None),
        }
    }

    /// Returns whether the structured NBT differs from the value that was read or constructed.
    #[must_use]
    pub fn is_modified(&self) -> bool {
        self.nbt != self.original_nbt
    }

    /// Serializes the current player NBT without selecting another historical representation.
    ///
    /// An unchanged record returns its original bytes verbatim, preserving unknown/future encoding
    /// details and compound ordering.
    pub fn to_raw(&self) -> Result<Bytes> {
        if !self.is_modified() {
            return Ok(self.raw.clone());
        }
        Ok(Bytes::from(serialize_root_nbt(&self.nbt)?))
    }

    /// Edits the owned player NBT and refreshes saved-item representation evidence afterwards.
    pub fn edit_nbt<R>(&mut self, edit: impl FnOnce(&mut NbtTag) -> R) -> R {
        let result = edit(&mut self.nbt);
        self.refresh_saved_items();
        result
    }

    /// Re-detects the saved-item representation after the caller edits player NBT.
    pub fn refresh_saved_items(&mut self) {
        self.saved_items = detect_saved_items(&self.nbt);
    }

    pub(crate) fn root(&self) -> Result<&indexmap::IndexMap<String, NbtTag>> {
        match &self.nbt {
            NbtTag::Compound(root) => Ok(root),
            _ => Err(BedrockWorldError::CorruptWorld(
                "player root is not an NBT compound".to_string(),
            )),
        }
    }

    pub(crate) fn root_mut(&mut self) -> Result<&mut indexmap::IndexMap<String, NbtTag>> {
        match &mut self.nbt {
            NbtTag::Compound(root) => Ok(root),
            _ => Err(BedrockWorldError::CorruptWorld(
                "player root is not an NBT compound".to_string(),
            )),
        }
    }

    pub(crate) fn finish_edit(&mut self) {
        self.refresh_saved_items();
    }

    pub(crate) fn from_nbt_with_level_version(
        id: PlayerId,
        nbt: NbtTag,
        level_version: Option<LevelVersion>,
    ) -> Result<Self> {
        ensure_player_compound(&nbt)?;
        let raw = Bytes::from(serialize_root_nbt(&nbt)?);
        let saved_items = detect_saved_items(&nbt);
        Ok(Self {
            id,
            original_nbt: nbt.clone(),
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
        ensure_player_compound(&nbt)?;
        let saved_items = detect_saved_items(&nbt);
        Ok(Self {
            id,
            original_nbt: nbt.clone(),
            nbt,
            raw,
            saved_items,
            level_version,
        })
    }
}

fn ensure_player_compound(nbt: &NbtTag) -> Result<()> {
    if matches!(nbt, NbtTag::Compound(_)) {
        Ok(())
    } else {
        Err(BedrockWorldError::CorruptWorld(
            "player root is not an NBT compound".to_string(),
        ))
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
    if matches!(
        root.get("id"),
        Some(NbtTag::Byte(_) | NbtTag::Short(_) | NbtTag::Int(_) | NbtTag::Long(_))
    ) {
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
        assert_eq!(
            PlayerId::from_storage_key(b"~local_player"),
            Some(PlayerId::Local)
        );
        assert_eq!(
            PlayerId::from_storage_key(b"player_-123"),
            Some(PlayerId::Xuid("-123".to_string()))
        );
        assert_eq!(PlayerId::from_storage_key(b"player_\xff"), None);
        assert_eq!(PlayerId::from_storage_key(b"player_"), None);
        assert_eq!(PlayerId::from_storage_key(b"actorprefix123"), None);
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
                    (
                        "Name".to_string(),
                        NbtTag::String("minecraft:stone".to_string()),
                    ),
                    ("Count".to_string(), NbtTag::Byte(1)),
                    ("Block".to_string(), NbtTag::Compound(IndexMap::new())),
                ])),
            ]),
        )]));
        let player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        assert_eq!(player.saved_items, SavedItemKind::Mixed);
    }

    #[test]
    fn unchanged_player_returns_original_bytes() {
        let nbt = NbtTag::Compound(IndexMap::from([
            ("UnknownFutureField".to_string(), NbtTag::Long(9)),
            (
                "format_version".to_string(),
                NbtTag::String("1.0.0".to_string()),
            ),
        ]));
        let raw = Bytes::from(serialize_root_nbt(&nbt).unwrap());
        let player = PlayerData::from_raw(PlayerId::Local, raw.clone()).unwrap();
        assert_eq!(player.to_raw().unwrap(), raw);
        assert!(!player.is_modified());
        assert_eq!(player.format_version().unwrap(), Some("1.0.0"));
    }

    #[test]
    fn raw_legacy_constructor_does_not_guess_a_surrounding_level_root() {
        let player_nbt = NbtTag::Compound(IndexMap::from([(
            "PlayerLevel".to_string(),
            NbtTag::Int(4),
        )]));
        let level_root = NbtTag::Compound(IndexMap::from([(
            "Player".to_string(),
            player_nbt,
        )]));
        let raw = Bytes::from(serialize_root_nbt(&level_root).unwrap());
        let player = PlayerData::from_raw(PlayerId::LegacyLevelDat, raw.clone()).unwrap();
        assert_eq!(player.nbt, level_root);
        assert_eq!(player.to_raw().unwrap(), raw);
    }

    #[test]
    fn level_aware_legacy_constructor_extracts_only_exact_owning_level_root() {
        let player_nbt = NbtTag::Compound(IndexMap::from([(
            "PlayerLevel".to_string(),
            NbtTag::Int(4),
        )]));
        let level_root = NbtTag::Compound(IndexMap::from([
            ("Player".to_string(), player_nbt.clone()),
            ("LevelName".to_string(), NbtTag::String("legacy".to_string())),
        ]));
        let level = LevelDatDocument::new(10, level_root.clone());
        let player = PlayerData::from_nbt_with_level(
            PlayerId::LegacyLevelDat,
            level_root,
            &level,
        )
        .unwrap();
        assert_eq!(player.nbt, player_nbt);
        assert_eq!(player.id, PlayerId::LegacyLevelDat);

        let unrelated = NbtTag::Compound(IndexMap::from([(
            "Player".to_string(),
            NbtTag::Compound(IndexMap::from([(
                "PlayerLevel".to_string(),
                NbtTag::Int(99),
            )])),
        )]));
        let not_unwrapped = PlayerData::from_nbt_with_level(
            PlayerId::LegacyLevelDat,
            unrelated.clone(),
            &level,
        )
        .unwrap();
        assert_eq!(not_unwrapped.nbt, unrelated);
    }
}
