//! MCPE 1.6-1.8 saved-item checks and same-record writes for Bedrock player storage.

use crate::database::{StorageReadOptions, StorageVisitorControl};
use crate::error::{BedrockWorldError, Result};
use crate::item::{
    LegacySavedItemBlockStateTables, LegacySavedItemIdTable, MedievalSavedItemCheckReport,
    MedievalSavedItemConversionOutcome, MedievalSavedItemConversionReport,
    check_saved_items_for_medieval, check_saved_items_for_medieval_with_blocks,
    convert_saved_items_to_medieval, convert_saved_items_to_medieval_with_blocks,
};
use crate::nbt::{NbtTag, parse_root_nbt};
use crate::player::{
    PlayerData, read_level_dat_player, read_local_player_with_level, read_player_key,
    write_level_dat_player, write_local_player, write_player_key,
};
use crate::world::{BedrockWorld, PlayerSavedItemStorage, WorldStorageHandle};
use bytes::Bytes;

/// One physical player record whose saved items are not fully proven for Medieval format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerMedievalSavedItemCheckEntry {
    pub storage: PlayerSavedItemStorage,
    pub report: MedievalSavedItemCheckReport,
}

/// Aggregate Medieval saved-item preflight across all physical player records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorldPlayerMedievalSavedItemCheckReport {
    pub records_checked: usize,
    pub items_seen: usize,
    pub classic_sources: usize,
    pub string_sources: usize,
    pub representable: usize,
    pub missing: usize,
    pub ambiguous: usize,
    pub metadata_out_of_range: usize,
    pub block_states_proven: usize,
    pub block_states_required: usize,
    pub block_states_incompatible: usize,
    pub unresolved_records: Vec<PlayerMedievalSavedItemCheckEntry>,
}

impl WorldPlayerMedievalSavedItemCheckReport {
    #[must_use]
    pub fn is_fully_proven(&self) -> bool {
        self.missing == 0
            && self.ambiguous == 0
            && self.metadata_out_of_range == 0
            && self.block_states_required == 0
            && self.block_states_incompatible == 0
    }

    fn add_counts(&mut self, report: &MedievalSavedItemCheckReport) {
        self.records_checked = self.records_checked.saturating_add(1);
        self.items_seen = self.items_seen.saturating_add(report.items_seen);
        self.classic_sources = self.classic_sources.saturating_add(report.classic_sources);
        self.string_sources = self.string_sources.saturating_add(report.string_sources);
        self.representable = self.representable.saturating_add(report.representable);
        self.missing = self.missing.saturating_add(report.missing);
        self.ambiguous = self.ambiguous.saturating_add(report.ambiguous);
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

    fn record(&mut self, storage: PlayerSavedItemStorage, report: MedievalSavedItemCheckReport) {
        self.add_counts(&report);
        if !report.is_fully_proven() {
            self.unresolved_records
                .push(PlayerMedievalSavedItemCheckEntry { storage, report });
        }
    }
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Checks every persisted player record for exact MCPE 1.6-1.8 saved-item representation.
    ///
    /// The scan is non-mutating. Matching duplicate local-player records are physical records and are
    /// therefore counted independently. Raw `player_*` keys are copied only for unresolved records.
    pub fn check_player_saved_items_for_medieval_blocking(
        &self,
        table: &LegacySavedItemIdTable,
    ) -> Result<WorldPlayerMedievalSavedItemCheckReport> {
        self.check_player_saved_items_for_medieval_inner(table, None)
    }

    /// Checks every player for Medieval representation including modern blockitem BlockStates.
    pub fn check_player_saved_items_for_medieval_with_blocks_blocking(
        &self,
        table: &LegacySavedItemIdTable,
        blocks: &LegacySavedItemBlockStateTables<'_>,
    ) -> Result<WorldPlayerMedievalSavedItemCheckReport> {
        self.check_player_saved_items_for_medieval_inner(table, Some(blocks))
    }

    /// Converts exactly one physical player record to Medieval saved-item representation.
    ///
    /// A modern `Block` payload makes this variant refuse conversion. Missing records return None.
    pub fn convert_player_saved_items_to_medieval_blocking(
        &self,
        storage: &PlayerSavedItemStorage,
        table: &LegacySavedItemIdTable,
    ) -> Result<Option<MedievalSavedItemConversionReport>> {
        self.convert_player_saved_items_to_medieval_inner(storage, table, None)
    }

    /// Converts exactly one physical player record to Medieval representation with blockitem proof.
    pub fn convert_player_saved_items_to_medieval_with_blocks_blocking(
        &self,
        storage: &PlayerSavedItemStorage,
        table: &LegacySavedItemIdTable,
        blocks: &LegacySavedItemBlockStateTables<'_>,
    ) -> Result<Option<MedievalSavedItemConversionReport>> {
        self.convert_player_saved_items_to_medieval_inner(storage, table, Some(blocks))
    }

    fn check_player_saved_items_for_medieval_inner(
        &self,
        table: &LegacySavedItemIdTable,
        blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
    ) -> Result<WorldPlayerMedievalSavedItemCheckReport> {
        let level = self.read_level_dat_blocking()?;
        let mut aggregate = WorldPlayerMedievalSavedItemCheckReport::default();

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

    fn convert_player_saved_items_to_medieval_inner(
        &self,
        storage: &PlayerSavedItemStorage,
        table: &LegacySavedItemIdTable,
        blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
    ) -> Result<Option<MedievalSavedItemConversionReport>> {
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
) -> Result<MedievalSavedItemCheckReport> {
    match blocks {
        Some(blocks) => player.check_saved_items_for_medieval_with_blocks(table, blocks),
        None => player.check_saved_items_for_medieval(table),
    }
}

fn check_nbt(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
) -> Result<MedievalSavedItemCheckReport> {
    match blocks {
        Some(blocks) => check_saved_items_for_medieval_with_blocks(nbt, table, blocks),
        None => check_saved_items_for_medieval(nbt, table),
    }
}

fn convert_player(
    player: &mut PlayerData,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
) -> Result<MedievalSavedItemConversionReport> {
    match blocks {
        Some(blocks) => player.convert_saved_items_to_medieval_with_blocks(table, blocks),
        None => player.convert_saved_items_to_medieval(table),
    }
}

fn convert_nbt(
    nbt: &NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
) -> Result<MedievalSavedItemConversionOutcome> {
    match blocks {
        Some(blocks) => convert_saved_items_to_medieval_with_blocks(nbt, table, blocks),
        None => convert_saved_items_to_medieval(nbt, table),
    }
}
