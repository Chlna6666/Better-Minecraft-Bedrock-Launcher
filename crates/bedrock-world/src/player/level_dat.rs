//! Historical Minecraft Bedrock player data stored in `level.dat.Player`.

use crate::error::{BedrockWorldError, Result};
use crate::level::LevelDatDocument;
use crate::nbt::NbtTag;
use crate::player::{PlayerData, PlayerId};
use crate::version::LevelVersion;

/// Reads the historical `Player` compound embedded in `level.dat`.
pub fn read_level_dat_player(document: &LevelDatDocument) -> Result<Option<PlayerData>> {
    let NbtTag::Compound(root) = &document.root else {
        return Err(BedrockWorldError::CorruptWorld(
            "level.dat root is not a compound".to_string(),
        ));
    };
    let Some(player) = root.get("Player").cloned() else {
        return Ok(None);
    };
    if !matches!(&player, NbtTag::Compound(_)) {
        return Err(BedrockWorldError::CorruptWorld(
            "level.dat Player field is not an NBT compound".to_string(),
        ));
    }
    PlayerData::from_nbt_with_level(PlayerId::LegacyLevelDat, player, document).map(Some)
}

/// Writes a `level.dat.Player` record back into `level.dat`.
///
/// The player must have been selected as the historical `level.dat.Player` record. This function does
/// not move a `~local_player` or `player_<id>` record into `level.dat` implicitly.
pub fn write_level_dat_player(document: &mut LevelDatDocument, player: &PlayerData) -> Result<()> {
    if player.id != PlayerId::LegacyLevelDat {
        return Err(BedrockWorldError::Validation(
            "write_level_dat_player requires PlayerId::LegacyLevelDat".to_string(),
        ));
    }
    let target = LevelVersion::detect(document)?;
    if let (Some(source), Some(target)) = (player.game_version(), target.last_opened_with.as_ref()) {
        if source != target {
            return Err(BedrockWorldError::Validation(format!(
                "player belongs to Bedrock {source}, target level.dat reports {target}; write the player for that target version explicitly before storing it"
            )));
        }
    }
    let NbtTag::Compound(root) = &mut document.root else {
        return Err(BedrockWorldError::CorruptWorld(
            "level.dat root is not a compound".to_string(),
        ));
    };
    if !matches!(&player.nbt, NbtTag::Compound(_)) {
        return Err(BedrockWorldError::Validation(
            "player root must be an NBT compound".to_string(),
        ));
    }
    root.insert("Player".to_string(), player.nbt.clone());
    Ok(())
}

/// Removes and returns `level.dat.Player` without touching LevelDB player keys.
pub fn remove_level_dat_player(document: &mut LevelDatDocument) -> Result<Option<PlayerData>> {
    let level_version = LevelVersion::detect(document)?;
    let NbtTag::Compound(root) = &mut document.root else {
        return Err(BedrockWorldError::CorruptWorld(
            "level.dat root is not a compound".to_string(),
        ));
    };
    let Some(player) = root.shift_remove("Player") else {
        return Ok(None);
    };
    PlayerData::from_nbt_with_level_version(
        PlayerId::LegacyLevelDat,
        player,
        Some(level_version),
    )
    .map(Some)
}
