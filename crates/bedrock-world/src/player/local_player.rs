//! Minecraft Bedrock `~local_player` LevelDB record.

use crate::database::{StorageBatch, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use crate::level::LevelDatDocument;
use crate::player::{PlayerData, PlayerId};
use bytes::Bytes;

const LOCAL_PLAYER_KEY: &[u8] = b"~local_player";

/// Reads `~local_player` and detects saved-item/version evidence from its contents.
pub fn read_local_player(storage: &dyn WorldStorage) -> Result<Option<PlayerData>> {
    storage
        .get(LOCAL_PLAYER_KEY)?
        .map(|raw| PlayerData::from_raw(PlayerId::Local, raw))
        .transpose()
}

/// Reads `~local_player` with real Bedrock version evidence from `level.dat`.
pub fn read_local_player_with_level(
    storage: &dyn WorldStorage,
    level: &LevelDatDocument,
) -> Result<Option<PlayerData>> {
    storage
        .get(LOCAL_PLAYER_KEY)?
        .map(|raw| PlayerData::from_raw_with_level(PlayerId::Local, raw, level))
        .transpose()
}

/// Writes a `~local_player` record back to the exact `~local_player` key.
///
/// The supplied [`PlayerData`] must itself represent `~local_player`; this does not implicitly move
/// another Bedrock player-record family.
pub fn write_local_player(storage: &dyn WorldStorage, player: &PlayerData) -> Result<()> {
    if player.id != PlayerId::Local {
        return Err(BedrockWorldError::Validation(
            "write_local_player requires PlayerId::Local".to_string(),
        ));
    }
    let mut batch = StorageBatch::new();
    batch.put(Bytes::from_static(LOCAL_PLAYER_KEY), player.to_raw()?);
    storage.write_batch(&batch)
}

/// Deletes the exact `~local_player` key.
pub fn delete_local_player(storage: &dyn WorldStorage) -> Result<()> {
    let mut batch = StorageBatch::new();
    batch.delete(Bytes::from_static(LOCAL_PLAYER_KEY));
    storage.write_batch(&batch)
}
