//! Read and write access for the player storage forms used by Minecraft Bedrock worlds.
//!
//! These functions preserve the selected storage form. Moving a player between storage forms is an
//! explicit conversion and is not performed by normal reads or writes.

use crate::database::{StorageBatch, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use crate::level::LevelDatDocument;
use crate::nbt::NbtTag;
use crate::player::{PlayerData, PlayerId};
use crate::version::LevelVersion;
use bytes::Bytes;

/// Reads `level.dat.Player` without moving it to LevelDB.
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

/// Writes a player into `level.dat.Player` without creating or modifying LevelDB player records.
pub fn write_level_dat_player(document: &mut LevelDatDocument, player: &PlayerData) -> Result<()> {
    let target_level = LevelVersion::detect(document)?;
    if let (Some(source), Some(target)) = (
        player.format.game_version(),
        target_level.last_opened_with.as_ref(),
    ) {
        if source != target {
            return Err(BedrockWorldError::Validation(format!(
                "player was read for Bedrock {source} but target level.dat reports {target}; explicit player conversion is required"
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

/// Removes and returns `level.dat.Player` without touching any LevelDB player record.
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

/// Reads `~local_player` or `player_<xuid>` without changing the stored representation.
pub fn read_player_record(
    storage: &dyn WorldStorage,
    id: PlayerId,
) -> Result<Option<PlayerData>> {
    let key = id
        .storage_key()
        .ok_or_else(|| {
            BedrockWorldError::Validation(
                "requested player id is not stored as a LevelDB player record".to_string(),
            )
        })?
        .into_owned();
    storage
        .get(&key)?
        .map(|raw| PlayerData::from_raw(id, raw))
        .transpose()
}

/// Reads a LevelDB player record with version evidence from the owning `level.dat`.
pub fn read_player_record_with_level(
    storage: &dyn WorldStorage,
    id: PlayerId,
    level: &LevelDatDocument,
) -> Result<Option<PlayerData>> {
    let key = id
        .storage_key()
        .ok_or_else(|| {
            BedrockWorldError::Validation(
                "requested player id is not stored as a LevelDB player record".to_string(),
            )
        })?
        .into_owned();
    storage
        .get(&key)?
        .map(|raw| PlayerData::from_raw_with_level(id, raw, level))
        .transpose()
}

/// Writes a local/server player record at its existing Bedrock LevelDB key.
pub fn write_player_record(storage: &dyn WorldStorage, player: &PlayerData) -> Result<()> {
    let key = player.id.storage_key().ok_or_else(|| {
        BedrockWorldError::Validation(
            "player id is not a LevelDB player storage form".to_string(),
        )
    })?;
    let mut batch = StorageBatch::new();
    batch.put(Bytes::copy_from_slice(key.as_ref()), player.to_raw()?);
    storage.write_batch(&batch)
}

/// Deletes a local/server player record without creating another player representation.
pub fn delete_player_record(storage: &dyn WorldStorage, id: &PlayerId) -> Result<()> {
    let key = id.storage_key().ok_or_else(|| {
        BedrockWorldError::Validation(
            "player id is not a LevelDB player storage form".to_string(),
        )
    })?;
    let mut batch = StorageBatch::new();
    batch.delete(Bytes::copy_from_slice(key.as_ref()));
    storage.write_batch(&batch)
}
