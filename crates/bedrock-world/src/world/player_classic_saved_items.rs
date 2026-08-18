//! MCPE <= 1.5 exact Classic saved-item checks and same-record writes for Bedrock player storage.

use crate::database::{StorageReadOptions, StorageVisitorControl};
use crate::error::{BedrockWorldError, Result};
use crate::item::{
    ClassicSavedItemCheckReport, ClassicSavedItemConversionOutcome, ClassicSavedItemConversionReport,
    LegacySavedItemBlockStateTables, LegacySavedItemIdTable, check_saved_items_for_classic,
    check_saved_items_for_classic_with_blocks, convert_saved_items_to_classic,
    convert_saved_items_to_classic_with_blocks,
};
use crate::nbt::{NbtTag, parse_root_nbt};
use crate::player::{
    PlayerData, read_level_dat_player, read_local_player_with_level, read_player_key,
    write_level_dat_player, write_local_player, write_player_key,
};
use crate::world::{BedrockWorld, PlayerSavedItemStorage, WorldStorageHandle};
use bytes::Bytes;

/// One physical player record whose saved items are not fully proven for exact Classic format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerClassicSavedItemCheckEntry {
    pub storage: PlayerSavedItemStorage,
    pub report: ClassicSavedItemCheckReport,
}

/// Aggregate exact Classic preflight across all physical player records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorldPlayerClassicSavedItemCheckReport {
    pub records_checked: usize,
    pub items_seen: usize,
    pub numeric_sources: usize,
    pub string_sources: usize,
    pub representable: usize,
    pub missing: usize,
    pub ambiguous: usize,
    pub id_out_of_range: usize,
    pub metadata_out_of_range: usize,
    pub block_states_proven: usize,
    pub block_states_required: usize,
    pub block_states_incompatible: usize,
    pub unresolved_records: Vec<PlayerClassicSavedItemCheckEntry>,
}

impl WorldPlayerClassicSavedItemCheckReport {
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

    /// Converts exactly one physical player record to exact Classic saved-item representation.
    pub fn convert_player_saved_items_to_classic_blocking(
        &self,
        storage: &PlayerSavedItemStorage,
        table: &LegacySavedItemIdTable,
    ) -> Result<Option<ClassicSavedItemConversionReport>> {
        self.convert_player_saved_items_to_classic_inner(storage, table, None)
    }

    /// Converts exactly one physical player record to exact Classic format with blockitem proof.
    pub fn convert_player_saved_items_to_classic_with_blocks_blocking(
        &self,
        storage: &PlayerSavedItemStorage,
        table: &LegacySavedItemIdTable,
        blocks: &LegacySavedItemBlockStateTables<'_>,
    ) -> Result<Option<ClassicSavedItemConversionReport>> {
        self.convert_player_saved_items_to_classic_inner(storage, table, Some(blocks))
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

    fn convert_player_saved_items_to_classic_inner(
        &self,
        storage: &PlayerSavedItemStorage,
        table: &LegacySavedItemIdTable,
        blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
    ) -> Result<Option<ClassicSavedItemConversionReport>> {
        match storage {
            PlayerSavedItemStorage::LevelDatPlayer => {
                let mut level = self.read_level_dat_blocking()?;
                let Some(mut player) = read_level_dat_player(&level)? else {
                    return Ok(None);
                };
                let report = convert_player(&mut player, table, blocks)?;
                write_level_dat_player(&mut level, &player)?;
                self.write_level_dat_blocking(&level)?;
                Ok(Some(report))
            }
            PlayerSavedItemStorage::LocalPlayer => {
                let level = self.read_level_dat_blocking()?;
                let Some(mut player) = read_local_player_with_level(self.storage(), &level)? else {
                    return Ok(None);
                };
                let report = convert_player(&mut player, table, blocks)?;
                write_local_player(self.storage(), &player)?;
                Ok(Some(report))
            }
            PlayerSavedItemStorage::PlayerKey(key) => {
                let Some(mut player) = read_player_key(self.storage(), key)? else {
                    return Ok(None);
                };
                let outcome = convert_nbt(&player.nbt, table, blocks)?;
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

fn convert_player(
    player: &mut PlayerData,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
) -> Result<ClassicSavedItemConversionReport> {
    match blocks {
        Some(blocks) => player.convert_saved_items_to_classic_with_blocks(table, blocks),
        None => player.convert_saved_items_to_classic(table),
    }
}

fn convert_nbt(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
) -> Result<ClassicSavedItemConversionOutcome> {
    match blocks {
        Some(blocks) => convert_saved_items_to_classic_with_blocks(nbt, table, blocks),
        None => convert_saved_items_to_classic(nbt, table),
    }
}
