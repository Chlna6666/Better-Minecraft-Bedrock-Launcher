//! Older-Modern saved-item checks and same-record writes for persisted Bedrock player storage.

use crate::database::{StorageReadOptions, StorageVisitorControl};
use crate::error::{BedrockWorldError, Result};
use crate::item::{
    ModernSavedItemCheckReport, ModernSavedItemConversionOutcome, ModernSavedItemConversionReport,
    ModernSavedItemTarget, check_saved_items_for_modern_target,
    convert_saved_items_to_modern_target,
};
use crate::nbt::{NbtTag, parse_root_nbt};
use crate::player::{
    PlayerData, read_level_dat_player, read_local_player_with_level, read_player_key,
    write_level_dat_player, write_local_player, write_player_key,
};
use crate::version::LevelVersion;
use crate::world::{BedrockWorld, PlayerSavedItemStorage, WorldStorageHandle};
use bytes::Bytes;

/// One physical player record whose saved items are not fully proven for the selected Modern target.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerModernSavedItemCheckEntry {
    /// Exact physical Bedrock player record.
    pub storage: PlayerSavedItemStorage,
    /// Record-level incompatibilities.
    pub report: ModernSavedItemCheckReport,
}

/// Aggregate Modern target preflight across all physical player records in one world.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorldPlayerModernSavedItemCheckReport {
    pub records_checked: usize,
    pub items_seen: usize,
    pub string_sources: usize,
    pub numeric_sources: usize,
    pub identity_conflicts: usize,
    pub metadata_conflicts: usize,
    pub items_proven: usize,
    pub block_items_proven: usize,
    pub item_missing: usize,
    pub item_ambiguous: usize,
    pub metadata_out_of_range: usize,
    pub block_incompatible: usize,
    /// Only unresolved records retain per-record detail. Raw `player_*` keys are copied only here.
    pub unresolved_records: Vec<PlayerModernSavedItemCheckEntry>,
}

impl WorldPlayerModernSavedItemCheckReport {
    /// Returns whether every physical player record is exactly representable in the selected target.
    #[must_use]
    pub fn is_fully_proven(&self) -> bool {
        self.numeric_sources == 0
            && self.identity_conflicts == 0
            && self.metadata_conflicts == 0
            && self.item_missing == 0
            && self.item_ambiguous == 0
            && self.metadata_out_of_range == 0
            && self.block_incompatible == 0
    }

    fn add_counts(&mut self, report: &ModernSavedItemCheckReport) {
        self.records_checked = self.records_checked.saturating_add(1);
        self.items_seen = self.items_seen.saturating_add(report.items_seen);
        self.string_sources = self.string_sources.saturating_add(report.string_sources);
        self.numeric_sources = self.numeric_sources.saturating_add(report.numeric_sources);
        self.identity_conflicts = self
            .identity_conflicts
            .saturating_add(report.identity_conflicts);
        self.metadata_conflicts = self
            .metadata_conflicts
            .saturating_add(report.metadata_conflicts);
        self.items_proven = self.items_proven.saturating_add(report.items_proven);
        self.block_items_proven = self
            .block_items_proven
            .saturating_add(report.block_items_proven);
        self.item_missing = self.item_missing.saturating_add(report.item_missing);
        self.item_ambiguous = self.item_ambiguous.saturating_add(report.item_ambiguous);
        self.metadata_out_of_range = self
            .metadata_out_of_range
            .saturating_add(report.metadata_out_of_range);
        self.block_incompatible = self
            .block_incompatible
            .saturating_add(report.block_incompatible);
    }

    fn record(&mut self, storage: PlayerSavedItemStorage, report: ModernSavedItemCheckReport) {
        self.add_counts(&report);
        if !report.is_fully_proven() {
            self.unresolved_records
                .push(PlayerModernSavedItemCheckEntry { storage, report });
        }
    }
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Checks every persisted player record against one concrete older Modern target release.
    ///
    /// The world's `LastOpenedWithVersion` must exactly equal `target.source_game_version()`. The scan
    /// does not modify storage. Matching duplicate local-player records are physical records and are
    /// counted independently. Raw `player_*` keys are copied only when their record is unresolved.
    pub fn check_player_saved_items_for_modern_target_blocking(
        &self,
        target: &ModernSavedItemTarget,
    ) -> Result<WorldPlayerModernSavedItemCheckReport> {
        let level = self.read_level_dat_blocking()?;
        ensure_world_source_game_version(&level, target)?;
        let mut aggregate = WorldPlayerModernSavedItemCheckReport::default();

        if let Some(player) = read_level_dat_player(&level)? {
            aggregate.record(
                PlayerSavedItemStorage::LevelDatPlayer,
                check_player(&player, target)?,
            );
        }
        if let Some(player) = read_local_player_with_level(self.storage(), &level)? {
            aggregate.record(
                PlayerSavedItemStorage::LocalPlayer,
                check_player(&player, target)?,
            );
        }
        self.storage().for_each_prefix_ref(
            b"player_",
            StorageReadOptions::default(),
            &mut |entry| {
                let nbt = parse_root_nbt(entry.value)?;
                if !matches!(nbt, NbtTag::Compound(_)) {
                    return Err(BedrockWorldError::CorruptWorld(format!(
                        "player_* record {:?} root is not an NBT compound",
                        entry.key
                    )));
                }
                let report = check_saved_items_for_modern_target(&nbt, target)?;
                if report.is_fully_proven() {
                    aggregate.add_counts(&report);
                } else {
                    aggregate.record(
                        PlayerSavedItemStorage::PlayerKey(Bytes::copy_from_slice(entry.key)),
                        report,
                    );
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        Ok(aggregate)
    }

    /// Converts exactly one physical player record to the selected older Modern release.
    ///
    /// The world source version is checked before the record is read for conversion. Only the selected
    /// physical record is written; this function never attempts a cross-`level.dat`/LevelDB transaction.
    /// Missing records return `Ok(None)`.
    pub fn convert_player_saved_items_to_modern_target_blocking(
        &self,
        storage: &PlayerSavedItemStorage,
        target: &ModernSavedItemTarget,
    ) -> Result<Option<ModernSavedItemConversionReport>> {
        let mut level = self.read_level_dat_blocking()?;
        ensure_world_source_game_version(&level, target)?;

        match storage {
            PlayerSavedItemStorage::LevelDatPlayer => {
                let Some(mut player) = read_level_dat_player(&level)? else {
                    return Ok(None);
                };
                let report = player.convert_saved_items_to_modern_target(target)?;
                write_level_dat_player(&mut level, &player)?;
                self.write_level_dat_blocking(&level)?;
                Ok(Some(report))
            }
            PlayerSavedItemStorage::LocalPlayer => {
                let Some(mut player) = read_local_player_with_level(self.storage(), &level)? else {
                    return Ok(None);
                };
                let report = player.convert_saved_items_to_modern_target(target)?;
                write_local_player(self.storage(), &player)?;
                Ok(Some(report))
            }
            PlayerSavedItemStorage::PlayerKey(key) => {
                let Some(mut player) = read_player_key(self.storage(), key)? else {
                    return Ok(None);
                };
                let outcome = convert_nbt(&player.nbt, target)?;
                let report = outcome.report;
                player.edit_nbt(|nbt| *nbt = outcome.nbt);
                write_player_key(self.storage(), &player)?;
                Ok(Some(report))
            }
        }
    }
}

fn check_player(
    player: &PlayerData,
    target: &ModernSavedItemTarget,
) -> Result<ModernSavedItemCheckReport> {
    player.check_saved_items_for_modern_target(target)
}

fn convert_nbt(
    nbt: &NbtTag,
    target: &ModernSavedItemTarget,
) -> Result<ModernSavedItemConversionOutcome> {
    convert_saved_items_to_modern_target(nbt, target)
}

fn ensure_world_source_game_version(
    level: &crate::level::LevelDatDocument,
    target: &ModernSavedItemTarget,
) -> Result<()> {
    let version = LevelVersion::detect(level)?;
    let actual = version.last_opened_with.as_ref().ok_or_else(|| {
        BedrockWorldError::Validation(
            "Modern world saved-item conversion requires level.dat LastOpenedWithVersion evidence"
                .to_string(),
        )
    })?;
    if actual != target.source_game_version() {
        return Err(BedrockWorldError::Validation(format!(
            "Modern world saved-item source version mismatch: world={actual}, target-source={}",
            target.source_game_version()
        )));
    }
    Ok(())
}
