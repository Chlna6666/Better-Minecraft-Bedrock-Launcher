//! MCPE <= 1.5 exact Classic saved-item preflight for persisted Bedrock player storage.
//!
//! World-scope operations here are read-only. Rewriting only player saved items to Classic while
//! leaving the world/player record generation unchanged would create mixed-version data. Transform an
//! owned [`crate::player::PlayerData`] first, then write it only through a complete concrete target
//! version flow such as the confirmed MCPE 0.6.1 player writer.

use crate::storage::{StorageReadOptions, StorageVisitorControl};
use crate::error::{BedrockWorldError, Result};
use crate::item::{
    ClassicSavedItemCheckReport, LegacySavedItemBlockStateTables, LegacySavedItemIdTable,
    check_saved_items_for_classic, check_saved_items_for_classic_with_blocks,
};
use crate::nbt::{NbtTag, parse_root_nbt};
use crate::player::{PlayerData, read_level_dat_player, read_local_player_with_level};
use crate::world::{BedrockWorld, PlayerSavedItemStorage, WorldStorageHandle};
use bytes::Bytes;

/// One physical player record whose saved items are not fully proven for exact Classic format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerClassicSavedItemCheckEntry {
    /// Exact physical player storage record.
    pub storage: PlayerSavedItemStorage,
    /// Per-record Classic representation evidence.
    pub report: ClassicSavedItemCheckReport,
}

/// Aggregate exact Classic preflight across all physical player records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorldPlayerClassicSavedItemCheckReport {
    /// Number of physical player records inspected.
    pub records_checked: usize,
    /// Number of saved items inspected.
    pub items_seen: usize,
    /// Number of numeric saved-item sources.
    pub numeric_sources: usize,
    /// Number of string-form saved-item sources.
    pub string_sources: usize,
    /// Number of items exactly representable in Classic.
    pub representable: usize,
    /// Number of items with no proven Classic mapping.
    pub missing: usize,
    /// Number of items with ambiguous Classic mappings.
    pub ambiguous: usize,
    /// Number of Classic numeric IDs outside the target representation.
    pub id_out_of_range: usize,
    /// Number of metadata values outside the target representation.
    pub metadata_out_of_range: usize,
    /// Number of block-item states proven by supplied reverse tables.
    pub block_states_proven: usize,
    /// Number of block items that require reverse BlockState evidence.
    pub block_states_required: usize,
    /// Number of block items whose BlockState is incompatible with the Classic identity/meta.
    pub block_states_incompatible: usize,
    /// Physical records that are not fully representable.
    pub unresolved_records: Vec<PlayerClassicSavedItemCheckEntry>,
}

impl WorldPlayerClassicSavedItemCheckReport {
    /// Returns whether every inspected saved item has a fully proven Classic representation.
    #[must_use]
    pub fn is_fully_proven(&self) -> bool {
        self.missing == 0
            && self.ambiguous == 0
            && self.id_out_of_range == 0
            && self.metadata_out_of_range == 0
            && self.block_states_required == 0
            && self.block_states_incompatible == 0
    }

    fn add_counts(&mut self, report: &ClassicSavedItemCheckReport) {
        self.records_checked = self.records_checked.saturating_add(1);
        self.items_seen = self.items_seen.saturating_add(report.items_seen);
        self.numeric_sources = self.numeric_sources.saturating_add(report.numeric_sources);
        self.string_sources = self.string_sources.saturating_add(report.string_sources);
        self.representable = self.representable.saturating_add(report.representable);
        self.missing = self.missing.saturating_add(report.missing);
        self.ambiguous = self.ambiguous.saturating_add(report.ambiguous);
        self.id_out_of_range = self.id_out_of_range.saturating_add(report.id_out_of_range);
        self.metadata_out_of_range = self
            .metadata_out_of_range
            .saturating_add(report.metadata_out_of_range);
        self.block_states_proven = self
            .block_states_proven
            .saturating_add(report.block_states_proven);
        self.block_states_required = self
            .block_states_required
            .saturating_add(report.block_states_required);
        self.block_states_incompatible = self
            .block_states_incompatible
            .saturating_add(report.block_states_incompatible);
    }

    fn record(&mut self, storage: PlayerSavedItemStorage, report: ClassicSavedItemCheckReport) {
        self.add_counts(&report);
        if !report.is_fully_proven() {
            self.unresolved_records
                .push(PlayerClassicSavedItemCheckEntry { storage, report });
        }
    }
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Checks every persisted player record for exact MCPE <= 1.5 saved-item representation.
    pub fn check_player_saved_items_for_classic_blocking(
        &self,
        table: &LegacySavedItemIdTable,
    ) -> Result<WorldPlayerClassicSavedItemCheckReport> {
        self.check_player_saved_items_for_classic_inner(table, None)
    }

    /// Checks every player for exact Classic representation including blockitem BlockStates.
    pub fn check_player_saved_items_for_classic_with_blocks_blocking(
        &self,
        table: &LegacySavedItemIdTable,
        blocks: &LegacySavedItemBlockStateTables<'_>,
    ) -> Result<WorldPlayerClassicSavedItemCheckReport> {
        self.check_player_saved_items_for_classic_inner(table, Some(blocks))
    }

    fn check_player_saved_items_for_classic_inner(
        &self,
        table: &LegacySavedItemIdTable,
        blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
    ) -> Result<WorldPlayerClassicSavedItemCheckReport> {
        let level = self.read_level_dat_blocking()?;
        let mut aggregate = WorldPlayerClassicSavedItemCheckReport::default();
        if let Some(player) = read_level_dat_player(&level)? {
            aggregate.record(
                PlayerSavedItemStorage::LevelDatPlayer,
                check_player(&player, table, blocks)?,
            );
        }
        if let Some(player) = read_local_player_with_level(self.storage(), &level)? {
            aggregate.record(
                PlayerSavedItemStorage::LocalPlayer,
                check_player(&player, table, blocks)?,
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
                let report = check_nbt(&nbt, table, blocks)?;
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
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
) -> Result<ClassicSavedItemCheckReport> {
    match blocks {
        Some(blocks) => player.check_saved_items_for_classic_with_blocks(table, blocks),
        None => player.check_saved_items_for_classic(table),
    }
}

fn check_nbt(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
) -> Result<ClassicSavedItemCheckReport> {
    match blocks {
        Some(blocks) => check_saved_items_for_classic_with_blocks(nbt, table, blocks),
        None => check_saved_items_for_classic(nbt, table),
    }
}
