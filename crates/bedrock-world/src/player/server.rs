//! Minecraft Bedrock server-player records.
//!
//! The storage key is authoritative. This module reads the textual `player_<id>` form; [`key`]
//! preserves arbitrary raw suffix bytes without assuming they are an Xbox XUID or valid UTF-8.

use crate::error::{BedrockWorldError, Result};
use crate::level::LevelDatDocument;
use crate::player::{PlayerData, PlayerId};
use crate::storage::{StorageBatch, WorldStorage};
use bytes::Bytes;

pub mod key;

fn record_key(id: &str) -> Result<Bytes> {
    if id.is_empty() {
        return Err(BedrockWorldError::Validation(
            "player_<id> suffix cannot be empty".to_string(),
        ));
    }
    Ok(Bytes::from(format!("player_{id}")))
}

/// Reads one textual `player_<id>` record.
pub fn read(
    storage: &dyn WorldStorage,
    id: &str,
    level: Option<&LevelDatDocument>,
) -> Result<Option<PlayerData>> {
    storage
        .get(&record_key(id)?)?
        .map(|raw| PlayerData::from_raw(PlayerId::Xuid(id.to_string()), raw, level))
        .transpose()
}

/// Writes one player back to the exact textual `player_<id>` key selected when it was read.
///
/// The record suffix must match the supplied player's persisted source id.
pub fn write(storage: &dyn WorldStorage, id: &str, player: &PlayerData) -> Result<()> {
    if player.id.key_id() != Some(id) {
        return Err(BedrockWorldError::Validation(format!(
            "player::server::write target player_{id} does not match PlayerData source {:?}",
            player.id
        )));
    }
    let mut batch = StorageBatch::new();
    batch.put(record_key(id)?, player.to_raw()?);
    storage.write_batch(&batch)
}

/// Deletes one exact textual `player_<id>` key.
pub fn delete(storage: &dyn WorldStorage, id: &str) -> Result<()> {
    key::delete(storage, &record_key(id)?)
}
