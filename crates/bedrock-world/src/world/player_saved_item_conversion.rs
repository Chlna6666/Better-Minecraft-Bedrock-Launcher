//! Explicit same-record saved-item conversion for persisted Minecraft Bedrock player storage.

use crate::error::Result;
use crate::item::{
    LegacySavedItemBlockStateTables, LegacySavedItemConversionOutcome,
    LegacySavedItemConversionReport, LegacySavedItemIdTable,
    convert_saved_items_to_legacy_numeric, convert_saved_items_to_legacy_numeric_with_blocks,
};
use crate::player::{
    PlayerData, read_level_dat_player, read_local_player_with_level, read_player_key,
    write_level_dat_player, write_local_player, write_player_key,
};
use crate::world::{BedrockWorld, PlayerSavedItemStorage, WorldStorageHandle};

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Converts named saved items in one exact persisted player record to historical numeric form.
    ///
    /// Only the selected physical record is written. A `Block` payload makes this variant refuse the
    /// conversion because no BlockState proof was supplied. Missing records return `Ok(None)`.
    pub fn convert_player_saved_items_to_legacy_numeric_blocking(
        &self,
        storage: &PlayerSavedItemStorage,
        table: &LegacySavedItemIdTable,
    ) -> Result<Option<LegacySavedItemConversionReport>> {
        self.convert_player_saved_items_to_legacy_numeric_inner(storage, table, None)
    }

    /// Converts named saved items in one exact persisted player record with blockitem proof.
    ///
    /// The selected record is preflighted completely before it is written. Proven modern `Block`
    /// payloads are removed only when item and block reverse mappings agree on the same historical
    /// identifier and metadata. No other player record is touched.
    pub fn convert_player_saved_items_to_legacy_numeric_with_blocks_blocking(
        &self,
        storage: &PlayerSavedItemStorage,
        table: &LegacySavedItemIdTable,
        blocks: &LegacySavedItemBlockStateTables<'_>,
    ) -> Result<Option<LegacySavedItemConversionReport>> {
        self.convert_player_saved_items_to_legacy_numeric_inner(storage, table, Some(blocks))
    }

    fn convert_player_saved_items_to_legacy_numeric_inner(
        &self,
        storage: &PlayerSavedItemStorage,
        table: &LegacySavedItemIdTable,
        blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
    ) -> Result<Option<LegacySavedItemConversionReport>> {
        match storage {
            PlayerSavedItemStorage::LevelDatPlayer => {
                let mut level = self.read_level_dat_blocking()?;
                let Some(mut player) = read_level_dat_player(&level)? else {
                    return Ok(None);
                };
                let report = convert_player_data(&mut player, table, blocks)?;
                write_level_dat_player(&mut level, &player)?;
                self.write_level_dat_blocking(&level)?;
                Ok(Some(report))
            }
            PlayerSavedItemStorage::LocalPlayer => {
                let level = self.read_level_dat_blocking()?;
                let Some(mut player) = read_local_player_with_level(self.storage(), &level)? else {
                    return Ok(None);
                };
                let report = convert_player_data(&mut player, table, blocks)?;
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

fn convert_player_data(
    player: &mut PlayerData,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
) -> Result<LegacySavedItemConversionReport> {
    match blocks {
        Some(blocks) => player.convert_saved_items_to_legacy_numeric_with_blocks(table, blocks),
        None => player.convert_saved_items_to_legacy_numeric(table),
    }
}

fn convert_nbt(
    nbt: &crate::nbt::NbtTag,
    table: &LegacySavedItemIdTable,
    blocks: Option<&LegacySavedItemBlockStateTables<'_>>,
) -> Result<LegacySavedItemConversionOutcome> {
    match blocks {
        Some(blocks) => convert_saved_items_to_legacy_numeric_with_blocks(nbt, table, blocks),
        None => convert_saved_items_to_legacy_numeric(nbt, table),
    }
}
