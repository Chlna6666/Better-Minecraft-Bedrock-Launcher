//! Minecraft Bedrock `player_<id>` LevelDB records.
//!
//! The suffix is retained exactly. This module does not assume that every historical `player_<id>`
//! key contains an Xbox XUID.

use crate::database::{StorageBatch, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use crate::level::LevelDatDocument;
use crate::player::{PlayerData, PlayerId};
use bytes::Bytes;

fn key(id: &str) -> Result<Bytes> {
    if id.is_empty() {
        return Err(BedrockWorldError::Validation(
            "player_<id> suffix cannot be empty".to_string(),
        ));
    }
    Ok(Bytes::from(format!("player_{id}")))
}

/// Reads one `player_<id>` record.
pub fn read_server_player(storage: &dyn WorldStorage, id: &str) -> Result<Option<PlayerData>> {
    storage
        .get(&key(id)?)?
        .map(|raw| PlayerData::from_raw(PlayerId::Xuid(id.to_string()), raw))
        .transpose()
}

/// Reads one `player_<id>` record with actual Bedrock version evidence from `level.dat`.
pub fn read_server_player_with_level(
    storage: &dyn WorldStorage,
    id: &str,
    level: &LevelDatDocument,
) -> Result<Option<PlayerData>> {
    storage
        .get(&key(id)?)?
        .map(|raw| PlayerData::from_raw_with_level(PlayerId::Xuid(id.to_string()), raw, level))
        .transpose()
}

/// Writes one player back to the exact `player_<id>` key selected when it was read.
///
/// The record suffix must match the supplied player's persisted source id.
pub fn write_server_player(
    storage: &dyn WorldStorage,
    id: &str,
    player: &PlayerData,
) -> Result<()> {
    if player.id.player_key_id() != Some(id) {
        return Err(BedrockWorldError::Validation(format!(
            "write_server_player target player_{id} does not match PlayerData source {:?}",
            player.id
        )));
    }
    let mut batch = StorageBatch::new();
    batch.put(key(id)?, player.to_raw()?);
    storage.write_batch(&batch)
}

/// Deletes one exact `player_<id>` key.
pub fn delete_server_player(storage: &dyn WorldStorage, id: &str) -> Result<()> {
    let mut batch = StorageBatch::new();
    batch.delete(key(id)?);
    storage.write_batch(&batch)
}
