//! Minecraft Bedrock `~local_player` LevelDB record and explicit movement to/from `level.dat.Player`.

use super::level_dat;
use super::storage::{LocalPlayerRecords, classify_local_player_records};
use crate::error::{BedrockWorldError, Result};
use crate::level::{self, LevelDatDocument};
use crate::player::{PlayerData, PlayerId};
use crate::storage::{StorageBatch, WorldStorage};
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
pub struct StorageMoveReport {
    /// Whether the requested source representation existed.
    pub source_found: bool,
    /// Whether the destination representation was created by this call.
    pub target_created: bool,
    /// Whether an identical destination representation already existed and was reused.
    pub target_reused: bool,
    /// Whether the source representation was removed after the destination was durable.
    pub source_removed: bool,
}

/// Reads `~local_player`, optionally using version evidence from its owning `level.dat`.
pub fn read(
    storage: &dyn WorldStorage,
    level: Option<&LevelDatDocument>,
) -> Result<Option<PlayerData>> {
    storage
        .get(LOCAL_PLAYER_KEY)?
        .map(|raw| PlayerData::from_raw(PlayerId::Local, raw, level))
        .transpose()
}

/// Writes a `~local_player` record back to the exact `~local_player` key.
///
/// The supplied [`PlayerData`] must itself represent `~local_player`; this does not implicitly move
/// another Bedrock player-record family.
pub fn write(storage: &dyn WorldStorage, player: &PlayerData) -> Result<()> {
    if player.id != PlayerId::Local {
        return Err(BedrockWorldError::Validation(
            "player::local::write requires PlayerId::Local".to_string(),
        ));
    }
    let mut batch = StorageBatch::new();
    batch.put(Bytes::from_static(LOCAL_PLAYER_KEY), player.to_raw()?);
    storage.write_batch(&batch)
}

/// Deletes the exact `~local_player` key.
pub fn delete(storage: &dyn WorldStorage) -> Result<()> {
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
///
/// This is a physical storage migration only: the parsed player NBT, including unknown/future fields,
/// is carried forward without game-version conversion. `level.dat` and LevelDB cannot share one atomic
/// transaction; the LevelDB record is written before `level.dat.Player` is removed. Callers must
/// serialize writes to this world while the move is in progress; this operation does not coordinate
/// with Minecraft or another process editing the same files.
///
/// # Errors
///
/// Returns an error when `level.dat` or LevelDB data cannot be read or written, the player NBT cannot
/// be parsed/serialized, the destination conflicts with the source, or the destination write fails.
pub fn move_from_level(world_path: &Path, storage: &dyn WorldStorage) -> Result<StorageMoveReport> {
    let level_path = world_path.join("level.dat");
    let mut level = level::read(&level_path)?;
    let source = level_dat::read(&level)?;
    let existing = read(storage, Some(&level))?;
    let state = classify_local_player_records(source.as_ref(), existing.as_ref());

    let mut report = StorageMoveReport::default();
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
            let target = PlayerData::from_nbt(PlayerId::Local, source.nbt.clone(), Some(&level))?;
            write(storage, &target)?;
            report.source_found = true;
            report.target_created = true;
        }
    }

    if level_dat::remove(&mut level)?.is_some() {
        level::write(&level_path, &level)?;
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
///
/// The two backing stores do not participate in one transaction. Callers must serialize writes to this
/// world while the move is in progress; this operation does not coordinate with Minecraft or another
/// process editing the same files.
///
/// # Errors
///
/// Returns an error when either storage representation cannot be read, `level.dat` cannot be replaced,
/// LevelDB cannot delete `~local_player`, the player NBT cannot be parsed/serialized, or the two
/// existing records conflict.
pub fn move_to_level(world_path: &Path, storage: &dyn WorldStorage) -> Result<StorageMoveReport> {
    let level_path = world_path.join("level.dat");
    let mut level = level::read(&level_path)?;
    let existing = level_dat::read(&level)?;
    let source = read(storage, Some(&level))?;
    let state = classify_local_player_records(existing.as_ref(), source.as_ref());

    let mut report = StorageMoveReport::default();
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
            let target =
                PlayerData::from_nbt(PlayerId::LegacyLevelDat, source.nbt.clone(), Some(&level))?;
            level_dat::write(&mut level, &target)?;
            level::write(&level_path, &level)?;
            report.source_found = true;
            report.target_created = true;
        }
    }

    delete(storage)?;
    report.source_removed = true;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::NbtTag;
    use crate::storage::MemoryStorage;
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
        level::write(&path.join("level.dat"), &LevelDatDocument::new(10, root))
            .expect("write level.dat");
    }

    #[test]
    fn local_player_storage_moves_both_directions_without_changing_nbt() {
        let world = temporary_world("player-storage-roundtrip");
        let original = player_nbt(42, "Alex");
        write_level_with_player(&world, original.clone());
        let storage = MemoryStorage::new();

        let forward = move_from_level(&world, &storage).expect("move level.dat Player to local");
        assert!(forward.target_created);
        assert!(forward.source_removed);
        let level = level::read(&world.join("level.dat")).expect("read level.dat");
        assert!(level_dat::read(&level).expect("read embedded").is_none());
        assert_eq!(
            read(&storage, Some(&level))
                .expect("read local")
                .expect("local exists")
                .nbt,
            original
        );

        let reverse = move_to_level(&world, &storage).expect("move local to level.dat Player");
        assert!(reverse.target_created);
        assert!(reverse.source_removed);
        let level = level::read(&world.join("level.dat")).expect("read level.dat");
        assert_eq!(
            level_dat::read(&level)
                .expect("read embedded")
                .expect("embedded exists")
                .nbt,
            original
        );
        assert!(read(&storage, None).expect("read local").is_none());

        fs::remove_dir_all(world).expect("cleanup");
    }

    #[test]
    fn matching_duplicate_resumes_forward_move_by_removing_only_source() {
        let world = temporary_world("player-storage-forward-resume");
        let original = player_nbt(42, "Alex");
        write_level_with_player(&world, original.clone());
        let storage = MemoryStorage::new();
        let local = PlayerData::from_nbt(PlayerId::Local, original.clone(), None).unwrap();
        write(&storage, &local).unwrap();

        let report = move_from_level(&world, &storage).unwrap();
        assert!(report.source_found);
        assert!(report.target_reused);
        assert!(!report.target_created);
        assert!(report.source_removed);

        let level = level::read(&world.join("level.dat")).unwrap();
        assert!(level_dat::read(&level).unwrap().is_none());
        assert_eq!(read(&storage, None).unwrap().unwrap().nbt, original);
        fs::remove_dir_all(world).unwrap();
    }

    #[test]
    fn different_existing_local_player_blocks_forward_move() {
        let world = temporary_world("player-storage-conflict");
        let embedded = player_nbt(7, "Alex");
        write_level_with_player(&world, embedded.clone());
        let storage = MemoryStorage::new();
        let local = PlayerData::from_nbt(PlayerId::Local, player_nbt(8, "Steve"), None)
            .expect("build local");
        write(&storage, &local).expect("seed local");

        assert!(move_from_level(&world, &storage).is_err());
        let level = level::read(&world.join("level.dat")).expect("read level.dat");
        assert_eq!(
            level_dat::read(&level)
                .expect("read embedded")
                .expect("embedded exists")
                .nbt,
            embedded
        );

        fs::remove_dir_all(world).expect("cleanup");
    }
}
