//! Minecraft Bedrock `~local_player` LevelDB record and explicit movement to/from `level.dat.Player`.

use super::level_dat::{read_level_dat_player, remove_level_dat_player, write_level_dat_player};
use super::storage::{LocalPlayerRecords, classify_local_player_records};
use crate::storage::{StorageBatch, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use crate::level::{LevelDatDocument, read_level_dat_document, write_level_dat_document};
use crate::player::{PlayerData, PlayerId};
use bytes::Bytes;
use std::path::Path;

const LOCAL_PLAYER_KEY: &[u8] = b"~local_player";

/// Result of explicitly moving the local player between its two historical Bedrock storage forms.
///
/// Cross-file atomicity is impossible because `level.dat` and LevelDB are separate persistence
/// systems. The move functions therefore write the destination first and remove the source second.
/// An interruption can leave two identical copies, but cannot intentionally leave zero copies; the
/// next call recognizes the matching destination and safely completes the move.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalPlayerStorageMoveReport {
    /// Whether the requested source representation existed.
    pub source_found: bool,
    /// Whether the destination representation was created by this call.
    pub target_created: bool,
    /// Whether an identical destination representation already existed and was reused.
    pub target_reused: bool,
    /// Whether the source representation was removed after the destination was durable.
    pub source_removed: bool,
}

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

/// Explicitly moves historical `level.dat.Player` data to the `~local_player` LevelDB record.
///
/// This changes only the physical player storage location. It does not upgrade saved items, fields,
/// abilities, attributes, or any other player NBT to a newer game representation.
///
/// The destination is written first. If replacing `level.dat` then fails, both copies remain and a
/// retry completes the source removal. Matching duplicate records are therefore a recoverable state.
/// Conflicting `level.dat.Player` and `~local_player` NBT is rejected before any write.
pub fn move_level_dat_player_to_local_player(
    world_path: &Path,
    storage: &dyn WorldStorage,
) -> Result<LocalPlayerStorageMoveReport> {
    let level_path = world_path.join("level.dat");
    let mut level = read_level_dat_document(&level_path)?;
    let source = read_level_dat_player(&level)?;
    let existing = read_local_player_with_level(storage, &level)?;
    let state = classify_local_player_records(source.as_ref(), existing.as_ref());

    let mut report = LocalPlayerStorageMoveReport::default();
    match state {
        LocalPlayerRecords::None | LocalPlayerRecords::LocalPlayer => return Ok(report),
        LocalPlayerRecords::ConflictingLevelDatAndLocalPlayer => {
            return Err(BedrockWorldError::ConcurrentWrite(
                "level.dat.Player and ~local_player contain different player NBT".to_string(),
            ));
        }
        LocalPlayerRecords::MatchingLevelDatAndLocalPlayer => {
            report.source_found = true;
            report.target_reused = true;
        }
        LocalPlayerRecords::LevelDatPlayer => {
            let source = source.as_ref().expect("classified level.dat player exists");
            let target =
                PlayerData::from_nbt_with_level(PlayerId::Local, source.nbt.clone(), &level)?;
            write_local_player(storage, &target)?;
            report.source_found = true;
            report.target_created = true;
        }
    }

    if remove_level_dat_player(&mut level)?.is_some() {
        write_level_dat_document(&level_path, &level)?;
        report.source_removed = true;
    }
    Ok(report)
}

/// Explicitly moves `~local_player` data back into historical `level.dat.Player` storage.
///
/// This is the reverse physical-storage operation, not a reverse game-version upgrade. The player NBT
/// is preserved as-is. `level.dat` is atomically replaced before `~local_player` is deleted, so an
/// interruption can leave two identical copies but does not intentionally lose the player. Matching
/// duplicates are safely resumed; conflicting copies are rejected before any write.
pub fn move_local_player_to_level_dat(
    world_path: &Path,
    storage: &dyn WorldStorage,
) -> Result<LocalPlayerStorageMoveReport> {
    let level_path = world_path.join("level.dat");
    let mut level = read_level_dat_document(&level_path)?;
    let existing = read_level_dat_player(&level)?;
    let source = read_local_player_with_level(storage, &level)?;
    let state = classify_local_player_records(existing.as_ref(), source.as_ref());

    let mut report = LocalPlayerStorageMoveReport::default();
    match state {
        LocalPlayerRecords::None | LocalPlayerRecords::LevelDatPlayer => return Ok(report),
        LocalPlayerRecords::ConflictingLevelDatAndLocalPlayer => {
            return Err(BedrockWorldError::ConcurrentWrite(
                "level.dat.Player and ~local_player contain different player NBT".to_string(),
            ));
        }
        LocalPlayerRecords::MatchingLevelDatAndLocalPlayer => {
            report.source_found = true;
            report.target_reused = true;
        }
        LocalPlayerRecords::LocalPlayer => {
            let source = source.as_ref().expect("classified local player exists");
            let target = PlayerData::from_nbt_with_level(
                PlayerId::LegacyLevelDat,
                source.nbt.clone(),
                &level,
            )?;
            write_level_dat_player(&mut level, &target)?;
            write_level_dat_document(&level_path, &level)?;
            report.source_found = true;
            report.target_created = true;
        }
    }

    delete_local_player(storage)?;
    report.source_removed = true;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryStorage;
    use crate::nbt::NbtTag;
    use indexmap::IndexMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_world(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "bedrock-world-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    fn player_nbt(unique_id: i64, name: &str) -> NbtTag {
        NbtTag::Compound(IndexMap::from([
            ("UniqueID".to_string(), NbtTag::Long(unique_id)),
            ("PlayerName".to_string(), NbtTag::String(name.to_string())),
        ]))
    }

    fn write_level_with_player(path: &Path, player: NbtTag) {
        fs::create_dir_all(path).expect("create world");
        let root = NbtTag::Compound(IndexMap::from([
            ("LevelName".to_string(), NbtTag::String("test".to_string())),
            ("Player".to_string(), player),
        ]));
        write_level_dat_document(&path.join("level.dat"), &LevelDatDocument::new(10, root))
            .expect("write level.dat");
    }

    #[test]
    fn local_player_storage_moves_both_directions_without_changing_nbt() {
        let world = temporary_world("player-storage-roundtrip");
        let original = player_nbt(42, "Alex");
        write_level_with_player(&world, original.clone());
        let storage = MemoryStorage::new();

        let forward = move_level_dat_player_to_local_player(&world, &storage)
            .expect("move level.dat Player to local");
        assert!(forward.target_created);
        assert!(forward.source_removed);
        let level = read_level_dat_document(&world.join("level.dat")).expect("read level.dat");
        assert!(
            read_level_dat_player(&level)
                .expect("read embedded")
                .is_none()
        );
        assert_eq!(
            read_local_player_with_level(&storage, &level)
                .expect("read local")
                .expect("local exists")
                .nbt,
            original
        );

        let reverse = move_local_player_to_level_dat(&world, &storage)
            .expect("move local to level.dat Player");
        assert!(reverse.target_created);
        assert!(reverse.source_removed);
        let level = read_level_dat_document(&world.join("level.dat")).expect("read level.dat");
        assert_eq!(
            read_level_dat_player(&level)
                .expect("read embedded")
                .expect("embedded exists")
                .nbt,
            original
        );
        assert!(read_local_player(&storage).expect("read local").is_none());

        fs::remove_dir_all(world).expect("cleanup");
    }

    #[test]
    fn matching_duplicate_resumes_forward_move_by_removing_only_source() {
        let world = temporary_world("player-storage-forward-resume");
        let original = player_nbt(42, "Alex");
        write_level_with_player(&world, original.clone());
        let storage = MemoryStorage::new();
        let local = PlayerData::from_nbt(PlayerId::Local, original.clone()).unwrap();
        write_local_player(&storage, &local).unwrap();

        let report = move_level_dat_player_to_local_player(&world, &storage).unwrap();
        assert!(report.source_found);
        assert!(report.target_reused);
        assert!(!report.target_created);
        assert!(report.source_removed);

        let level = read_level_dat_document(&world.join("level.dat")).unwrap();
        assert!(read_level_dat_player(&level).unwrap().is_none());
        assert_eq!(read_local_player(&storage).unwrap().unwrap().nbt, original);
        fs::remove_dir_all(world).unwrap();
    }

    #[test]
    fn different_existing_local_player_blocks_forward_move() {
        let world = temporary_world("player-storage-conflict");
        let embedded = player_nbt(7, "Alex");
        write_level_with_player(&world, embedded.clone());
        let storage = MemoryStorage::new();
        let local =
            PlayerData::from_nbt(PlayerId::Local, player_nbt(8, "Steve")).expect("build local");
        write_local_player(&storage, &local).expect("seed local");

        assert!(move_level_dat_player_to_local_player(&world, &storage).is_err());
        let level = read_level_dat_document(&world.join("level.dat")).expect("read level.dat");
        assert_eq!(
            read_level_dat_player(&level)
                .expect("read embedded")
                .expect("embedded exists")
                .nbt,
            embedded
        );

        fs::remove_dir_all(world).expect("cleanup");
    }
}
