//! Observational Minecraft Bedrock player storage state.
//!
//! This module reports the actual record families present in a world without moving, upgrading or
//! rewriting any player. Raw `player_*` keys are retained as bytes because their suffix is not
//! guaranteed to be an Xbox XUID or even UTF-8 in every historical/custom world.

use super::level_dat::read_level_dat_player;
use super::local_player::read_local_player_with_level;
use crate::storage::{StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::Result;
use crate::level::LevelDatDocument;
use crate::player::PlayerData;
use bytes::Bytes;

/// Explicit physical storage target for the Bedrock local player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalPlayerStorage {
    /// Historical player compound embedded at `level.dat.Player`.
    LevelDatPlayer,
    /// LevelDB record stored under the exact `~local_player` key.
    LocalPlayer,
}

/// Actual local-player records present across `level.dat.Player` and `~local_player`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LocalPlayerRecords {
    /// Neither local-player representation exists.
    #[default]
    None,
    /// Only historical `level.dat.Player` exists.
    LevelDatPlayer,
    /// Only `~local_player` exists.
    LocalPlayer,
    /// Both records exist and contain the same player NBT.
    MatchingLevelDatAndLocalPlayer,
    /// Both records exist but contain different player NBT.
    ConflictingLevelDatAndLocalPlayer,
}

impl LocalPlayerRecords {
    /// Returns whether both local-player storage forms exist simultaneously.
    #[must_use]
    pub const fn has_duplicate_records(self) -> bool {
        matches!(
            self,
            Self::MatchingLevelDatAndLocalPlayer | Self::ConflictingLevelDatAndLocalPlayer
        )
    }

    /// Returns whether the two local-player records disagree and must not be merged automatically.
    #[must_use]
    pub const fn has_conflict(self) -> bool {
        matches!(self, Self::ConflictingLevelDatAndLocalPlayer)
    }
}

/// Non-mutating snapshot of Minecraft Bedrock player storage families in one world.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerStorageOverview {
    /// State of `level.dat.Player` and `~local_player`.
    pub local_player: LocalPlayerRecords,
    /// Exact LevelDB keys beginning with `player_`, sorted lexicographically and retained as raw bytes.
    pub player_keys: Vec<Bytes>,
}

impl PlayerStorageOverview {
    /// Returns whether the local player exists in either Bedrock storage form.
    #[must_use]
    pub const fn has_local_player(&self) -> bool {
        !matches!(self.local_player, LocalPlayerRecords::None)
    }

    /// Returns whether `level.dat.Player` and `~local_player` contain conflicting NBT.
    #[must_use]
    pub const fn has_local_player_conflict(&self) -> bool {
        self.local_player.has_conflict()
    }
}

/// Classifies already-read local player records without additional storage I/O.
pub(crate) fn classify_local_player_records(
    level_dat_player: Option<&PlayerData>,
    local_player: Option<&PlayerData>,
) -> LocalPlayerRecords {
    match (level_dat_player, local_player) {
        (None, None) => LocalPlayerRecords::None,
        (Some(_), None) => LocalPlayerRecords::LevelDatPlayer,
        (None, Some(_)) => LocalPlayerRecords::LocalPlayer,
        (Some(level_dat), Some(local)) if level_dat.nbt == local.nbt => {
            LocalPlayerRecords::MatchingLevelDatAndLocalPlayer
        }
        (Some(_), Some(_)) => LocalPlayerRecords::ConflictingLevelDatAndLocalPlayer,
    }
}

/// Inspects player storage without modifying `level.dat` or LevelDB.
pub fn inspect_player_storage(
    storage: &dyn WorldStorage,
    level: &LevelDatDocument,
) -> Result<PlayerStorageOverview> {
    let level_dat_player = read_level_dat_player(level)?;
    let local_player = read_local_player_with_level(storage, level)?;
    let local_player =
        classify_local_player_records(level_dat_player.as_ref(), local_player.as_ref());

    let mut player_keys = Vec::<Bytes>::new();
    storage.for_each_prefix_key(b"player_", StorageReadOptions::default(), &mut |key| {
        player_keys.push(Bytes::copy_from_slice(key));
        Ok(StorageVisitorControl::Continue)
    })?;
    player_keys.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
    player_keys.dedup();

    Ok(PlayerStorageOverview {
        local_player,
        player_keys,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryStorage;
    use crate::nbt::{NbtTag, serialize_root_nbt};
    use crate::player::PlayerId;
    use indexmap::IndexMap;

    fn player(unique_id: i64, name: &str) -> NbtTag {
        NbtTag::Compound(IndexMap::from([
            ("UniqueID".to_string(), NbtTag::Long(unique_id)),
            ("PlayerName".to_string(), NbtTag::String(name.to_string())),
        ]))
    }

    fn level_with_player(player: Option<NbtTag>) -> LevelDatDocument {
        let mut root = IndexMap::new();
        root.insert("LevelName".to_string(), NbtTag::String("test".to_string()));
        if let Some(player) = player {
            root.insert("Player".to_string(), player);
        }
        LevelDatDocument::new(10, NbtTag::Compound(root))
    }

    #[test]
    fn classifier_uses_already_read_player_nbt() {
        let source = player(1, "Alex");
        let embedded = PlayerData::from_nbt(PlayerId::LegacyLevelDat, source.clone()).unwrap();
        let local = PlayerData::from_nbt(PlayerId::Local, source).unwrap();
        assert_eq!(
            classify_local_player_records(Some(&embedded), Some(&local)),
            LocalPlayerRecords::MatchingLevelDatAndLocalPlayer
        );

        let other = PlayerData::from_nbt(PlayerId::Local, player(2, "Steve")).unwrap();
        assert_eq!(
            classify_local_player_records(Some(&embedded), Some(&other)),
            LocalPlayerRecords::ConflictingLevelDatAndLocalPlayer
        );
    }

    #[test]
    fn overview_distinguishes_matching_and_conflicting_local_player_copies() {
        let source = player(1, "Alex");
        let level = level_with_player(Some(source.clone()));
        let storage = MemoryStorage::new();
        storage
            .put(b"~local_player", &serialize_root_nbt(&source).unwrap())
            .unwrap();

        let matching = inspect_player_storage(&storage, &level).unwrap();
        assert_eq!(
            matching.local_player,
            LocalPlayerRecords::MatchingLevelDatAndLocalPlayer
        );
        assert!(matching.local_player.has_duplicate_records());
        assert!(!matching.has_local_player_conflict());

        let different = player(2, "Steve");
        storage
            .put(b"~local_player", &serialize_root_nbt(&different).unwrap())
            .unwrap();
        let conflicting = inspect_player_storage(&storage, &level).unwrap();
        assert_eq!(
            conflicting.local_player,
            LocalPlayerRecords::ConflictingLevelDatAndLocalPlayer
        );
        assert!(conflicting.has_local_player_conflict());
    }

    #[test]
    fn overview_preserves_raw_player_keys_without_xuid_assumptions() {
        let level = level_with_player(None);
        let storage = MemoryStorage::new();
        storage.put(b"player_-123", b"a").unwrap();
        storage.put(b"player_custom-name", b"b").unwrap();
        storage.put(b"player_\xff", b"c").unwrap();

        let overview = inspect_player_storage(&storage, &level).unwrap();
        assert_eq!(
            overview.player_keys,
            vec![
                Bytes::from_static(b"player_-123"),
                Bytes::from_static(b"player_custom-name"),
                Bytes::from_static(b"player_\xff"),
            ]
        );
    }
}
