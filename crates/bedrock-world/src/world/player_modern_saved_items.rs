//! Older-Modern saved-item preflight for persisted Bedrock player storage.
//!
//! This module is observational at world scope. Converting only saved-item NBT to an older release
//! while leaving `level.dat`, player fields, chunks and other world records at the source version would
//! create a mixed world. Actual item transformation remains available on owned [`crate::player::PlayerData`]
//! and item values for use by a complete target-version export/write flow.

use crate::database::{StorageReadOptions, StorageVisitorControl};
use crate::error::{BedrockWorldError, Result};
use crate::item::{
    ModernSavedItemCheckReport, ModernSavedItemTarget, check_saved_items_for_modern_target,
};
use crate::nbt::{NbtTag, parse_root_nbt};
use crate::player::{PlayerData, read_level_dat_player, read_local_player_with_level};
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
    /// Number of physical player records inspected.
    pub records_checked: usize,
    /// Number of saved items inspected.
    pub items_seen: usize,
    /// Number of string-form saved-item sources.
    pub string_sources: usize,
    /// Number of numeric saved-item sources incompatible with a Modern-only target conversion.
    pub numeric_sources: usize,
    /// Number of item identity conflicts.
    pub identity_conflicts: usize,
    /// Number of metadata conflicts.
    pub metadata_conflicts: usize,
    /// Number of non-block items proven for the target.
    pub items_proven: usize,
    /// Number of block items proven for the target.
    pub block_items_proven: usize,
    /// Number of missing target item identities.
    pub item_missing: usize,
    /// Number of ambiguous target item identities.
    pub item_ambiguous: usize,
    /// Number of metadata values outside the target representation.
    pub metadata_out_of_range: usize,
    /// Number of block-item states incompatible with the target.
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
}

fn check_player(
    player: &PlayerData,
    target: &ModernSavedItemTarget,
) -> Result<ModernSavedItemCheckReport> {
    player.check_saved_items_for_modern_target(target)
}

fn ensure_world_source_game_version(
    level: &crate::level::LevelDatDocument,
    target: &ModernSavedItemTarget,
) -> Result<()> {
    let version = LevelVersion::detect(level)?;
    let actual = version.last_opened_with.as_ref().ok_or_else(|| {
        BedrockWorldError::Validation(
            "Modern world saved-item preflight requires level.dat LastOpenedWithVersion evidence"
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
