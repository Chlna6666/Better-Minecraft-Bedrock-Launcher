//! Upgrade of legacy player data embedded in `level.dat`.

use crate::database::{StorageBatch, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use crate::level::LevelDatDocument;
use crate::nbt::{NbtTag, serialize_root_nbt};
use crate::player::{PlayerData, PlayerId};
use bytes::Bytes;

/// Result of preparing/upgrading a legacy embedded player.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayerMigrationReport {
    /// Whether a legacy `Player` compound was present in `level.dat`.
    pub embedded_player_found: bool,
    /// Whether a new `~local_player` record was created.
    pub local_record_created: bool,
    /// Whether an identical `~local_player` record already existed.
    pub local_record_reused: bool,
    /// Whether the in-memory `level.dat` document had its legacy `Player` field removed.
    pub embedded_player_removed: bool,
}

/// Migrates legacy `level.dat.Player` data into `~local_player` storage.
///
/// This is intentionally a two-phase safety operation. The database write is committed first; only
/// after that succeeds is the in-memory `Player` field removed from `document`. The caller then writes
/// the returned document atomically through the normal `level.dat` API. A crash between phases leaves
/// duplicate player data rather than losing it, and the operation is idempotent when the local record
/// bytes already match.
pub fn migrate_embedded_player_blocking(
    storage: &dyn WorldStorage,
    document: &mut LevelDatDocument,
) -> Result<PlayerMigrationReport> {
    let NbtTag::Compound(root) = &document.root else {
        return Err(BedrockWorldError::CorruptWorld(
            "level.dat root is not a compound".to_string(),
        ));
    };
    let Some(player_tag) = root.get("Player").cloned() else {
        return Ok(PlayerMigrationReport::default());
    };
    if !matches!(player_tag, NbtTag::Compound(_)) {
        return Err(BedrockWorldError::CorruptWorld(
            "legacy level.dat Player field is not an NBT compound".to_string(),
        ));
    }

    let player = PlayerData::from_nbt(PlayerId::Local, player_tag)?;
    let key = PlayerId::Local
        .storage_key()
        .ok_or_else(|| BedrockWorldError::Validation("local player has no storage key".to_string()))?;
    let existing = storage.get(key.as_ref())?;
    let mut report = PlayerMigrationReport {
        embedded_player_found: true,
        ..PlayerMigrationReport::default()
    };
    match existing {
        Some(existing) if existing == player.raw => {
            report.local_record_reused = true;
        }
        Some(_) => {
            return Err(BedrockWorldError::ConcurrentWrite(
                "~local_player already exists with different bytes; refusing to overwrite player data"
                    .to_string(),
            ));
        }
        None => {
            let mut batch = StorageBatch::new();
            batch.put(Bytes::copy_from_slice(key.as_ref()), player.raw.clone());
            storage.write_batch(&batch)?;
            report.local_record_created = true;
        }
    }

    let NbtTag::Compound(root) = &mut document.root else {
        unreachable!("validated level.dat compound above");
    };
    root.shift_remove("Player");
    report.embedded_player_removed = true;
    Ok(report)
}

/// Returns the encoded bytes that would be used for a legacy embedded player without mutating data.
pub fn embedded_player_bytes(document: &LevelDatDocument) -> Result<Option<Bytes>> {
    let NbtTag::Compound(root) = &document.root else {
        return Err(BedrockWorldError::CorruptWorld(
            "level.dat root is not a compound".to_string(),
        ));
    };
    root.get("Player")
        .map(|tag| serialize_root_nbt(tag).map(Bytes::from))
        .transpose()
}
