//! Minecraft Bedrock `player_<xuid>` LevelDB records.

use crate::database::{StorageBatch, WorldStorage};
use crate::error::Result;
use crate::level::LevelDatDocument;
use crate::player::{PlayerData, PlayerId};
use bytes::Bytes;

fn key(xuid: &str) -> Bytes {
    Bytes::from(format!("player_{xuid}"))
}

/// Reads one `player_<xuid>` record.
pub fn read_server_player(storage: &dyn WorldStorage, xuid: &str) -> Result<Option<PlayerData>> {
    storage
        .get(&key(xuid))?
        .map(|raw| PlayerData::from_raw(PlayerId::Xuid(xuid.to_string()), raw))
        .transpose()
}

/// Reads one `player_<xuid>` record with actual Bedrock version evidence from `level.dat`.
pub fn read_server_player_with_level(
    storage: &dyn WorldStorage,
    xuid: &str,
    level: &LevelDatDocument,
) -> Result<Option<PlayerData>> {
    storage
        .get(&key(xuid))?
        .map(|raw| PlayerData::from_raw_with_level(PlayerId::Xuid(xuid.to_string()), raw, level))
        .transpose()
}

/// Writes player NBT to the exact `player_<xuid>` key selected by the caller.
pub fn write_server_player(
    storage: &dyn WorldStorage,
    xuid: &str,
    player: &PlayerData,
) -> Result<()> {
    let mut batch = StorageBatch::new();
    batch.put(key(xuid), player.to_raw()?);
    storage.write_batch(&batch)
}

/// Deletes one exact `player_<xuid>` key.
pub fn delete_server_player(storage: &dyn WorldStorage, xuid: &str) -> Result<()> {
    let mut batch = StorageBatch::new();
    batch.delete(key(xuid));
    storage.write_batch(&batch)
}
