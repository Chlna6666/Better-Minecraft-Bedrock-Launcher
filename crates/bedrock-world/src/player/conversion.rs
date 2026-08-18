//! Explicit conversion between persisted Minecraft Bedrock player-data representations.
//!
//! Normal player reads and writes never call this module automatically. Conversion is caller-driven,
//! bidirectional where the target can represent the source, and reports unsupported version changes
//! instead of silently upgrading or downgrading data.

use crate::error::{BedrockWorldError, Result};
use crate::player::{PlayerData, PlayerDataFormat, PlayerId, PlayerStorage, SavedItemFormat};
use crate::version::{ConversionCompatibility, LevelVersion};

/// Explicit target for player-data conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerDataTarget {
    /// Target Bedrock player identifier/storage location.
    pub id: PlayerId,
    /// Target saved-item representation.
    pub saved_items: SavedItemFormat,
    /// Target world/level version evidence, when known.
    pub level_version: Option<LevelVersion>,
}

impl PlayerDataTarget {
    /// Creates a target that changes only the physical player storage location.
    #[must_use]
    pub fn preserve_data_format(id: PlayerId, source: &PlayerDataFormat) -> Self {
        Self {
            id,
            saved_items: source.saved_items,
            level_version: source.level_version.clone(),
        }
    }

    /// Returns the target physical storage location.
    #[must_use]
    pub const fn storage(&self) -> PlayerStorage {
        PlayerStorage::from_player_id(&self.id)
    }
}

/// Reports whether the current implementation can represent a requested player conversion.
#[must_use]
pub fn player_conversion_compatibility(
    source: &PlayerDataFormat,
    target: &PlayerDataTarget,
) -> ConversionCompatibility {
    if matches!(target.storage(), PlayerStorage::Unknown) {
        return ConversionCompatibility::Unsupported;
    }
    if source.saved_items != target.saved_items {
        return ConversionCompatibility::Unsupported;
    }

    let source_game = source
        .level_version
        .as_ref()
        .and_then(|version| version.last_opened_with.as_ref());
    let target_game = target
        .level_version
        .as_ref()
        .and_then(|version| version.last_opened_with.as_ref());
    if source_game.is_some() && target_game.is_some() && source_game != target_game {
        // Player field/item rules for arbitrary game-version changes are intentionally not guessed.
        return ConversionCompatibility::Unsupported;
    }
    ConversionCompatibility::Lossless
}

/// Converts a player representation only when the requested path is currently lossless.
///
/// This currently supports bidirectional storage-form conversion (`level.dat` <-> local/server)
/// without changing player NBT. Historical player/item schema changes will be added as explicit
/// source/target rules; until then such requests return `Unsupported` rather than mutating data.
pub fn convert_player_data(player: &PlayerData, target: PlayerDataTarget) -> Result<PlayerData> {
    match player_conversion_compatibility(&player.format, &target) {
        ConversionCompatibility::Lossless => PlayerData::from_nbt_with_level_version(
            target.id,
            player.nbt.clone(),
            target.level_version,
        ),
        ConversionCompatibility::Lossy => Err(BedrockWorldError::Validation(
            "lossy player-data conversion requires an explicit lossy conversion implementation"
                .to_string(),
        )),
        ConversionCompatibility::Unsupported => Err(BedrockWorldError::Validation(
            "requested player-data conversion is not supported by authoritative version rules"
                .to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::NbtTag;
    use indexmap::IndexMap;

    #[test]
    fn storage_conversion_is_bidirectional_and_lossless() {
        let source = PlayerData::from_nbt(
            PlayerId::LegacyLevelDat,
            NbtTag::Compound(IndexMap::new()),
        )
        .unwrap();
        let local_target = PlayerDataTarget::preserve_data_format(PlayerId::Local, &source.format);
        let local = convert_player_data(&source, local_target).unwrap();
        assert_eq!(local.format.storage, PlayerStorage::Local);

        let legacy_target =
            PlayerDataTarget::preserve_data_format(PlayerId::LegacyLevelDat, &local.format);
        let legacy = convert_player_data(&local, legacy_target).unwrap();
        assert_eq!(legacy.format.storage, PlayerStorage::LevelDat);
        assert_eq!(legacy.nbt, source.nbt);
    }
}
