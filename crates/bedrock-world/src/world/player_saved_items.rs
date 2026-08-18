//! World-level historical saved-item compatibility checks across all Bedrock player record families.

use crate::database::{StorageReadOptions, StorageVisitorControl};
use crate::error::{BedrockWorldError, Result};
use crate::item::{LegacySavedItemCheckReport, LegacySavedItemIdTable};
use crate::nbt::{NbtTag, parse_root_nbt};
use crate::player::{read_level_dat_player, read_local_player_with_level};
use crate::world::{BedrockWorld, WorldStorageHandle};
use bytes::Bytes;

/// Physical Bedrock player record that contains saved items checked by a world preflight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerSavedItemStorage {
    /// Historical `level.dat.Player` compound.
    LevelDatPlayer,
    /// Exact `~local_player` LevelDB record.
    LocalPlayer,
    /// Exact raw LevelDB key beginning with `player_`.
    PlayerKey(Bytes),
}

/// One persisted player record whose saved items are not fully proven writable as historical numeric data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSavedItemCheckEntry {
    /// Physical record containing the problematic saved items.
    pub storage: PlayerSavedItemStorage,
    /// Item-level detailed report for this record.
    pub report: LegacySavedItemCheckReport,
}

/// Aggregate historical numeric saved-item preflight for every persisted player record in a world.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorldPlayerSavedItemCheckReport {
    /// Number of physical player records checked. Matching duplicate local-player copies count twice.
    pub records_checked: usize,
    /// Recognised saved-item compounds visited across all checked records.
    pub items_seen: usize,
    /// Items already stored with numeric IDs.
    pub already_numeric: usize,
    /// Named items with one unique numeric ID/meta representation.
    pub named_unique: usize,
    /// Named items with no numeric representation.
    pub named_missing: usize,
    /// Named items with multiple numeric representations.
    pub named_ambiguous: usize,
    /// Unique block items still requiring BlockState reverse validation.
    pub block_states_required: usize,
    /// Only records with unresolved saved-item compatibility are retained here.
    pub unresolved_records: Vec<PlayerSavedItemCheckEntry>,
}

impl WorldPlayerSavedItemCheckReport {
    /// Returns whether every checked item ID/meta is exact.
    #[must_use]
    pub fn item_ids_are_exact(&self) -> bool {
        self.named_missing == 0 && self.named_ambiguous == 0
    }

    /// Returns whether item IDs are exact and no block item still needs BlockState reverse proof.
    #[must_use]
    pub fn is_fully_proven(&self) -> bool {
        self.item_ids_are_exact() && self.block_states_required == 0
    }

    fn add_counts(&mut self, report: &LegacySavedItemCheckReport) {
        self.records_checked = self.records_checked.saturating_add(1);
        self.items_seen = self.items_seen.saturating_add(report.items_seen);
        self.already_numeric = self.already_numeric.saturating_add(report.already_numeric);
        self.named_unique = self.named_unique.saturating_add(report.named_unique);
        self.named_missing = self.named_missing.saturating_add(report.named_missing);
        self.named_ambiguous = self.named_ambiguous.saturating_add(report.named_ambiguous);
        self.block_states_required = self
            .block_states_required
            .saturating_add(report.block_states_required);
    }

    fn record(
        &mut self,
        storage: PlayerSavedItemStorage,
        report: LegacySavedItemCheckReport,
    ) {
        self.add_counts(&report);
        if !report.is_fully_proven() {
            self.unresolved_records
                .push(PlayerSavedItemCheckEntry { storage, report });
        }
    }
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Checks historical numeric saved-item representability for every persisted player record.
    ///
    /// This does not mutate players. It checks `level.dat.Player`, `~local_player`, and every raw
    /// `player_*` LevelDB record. Raw server-player values are parsed directly from borrowed scan
    /// buffers; their key bytes are copied only if that record has unresolved compatibility.
    pub fn check_player_legacy_numeric_saved_items_blocking(
        &self,
        table: &LegacySavedItemIdTable,
    ) -> Result<WorldPlayerSavedItemCheckReport> {
        let level = self.read_level_dat_blocking()?;
        let mut report = WorldPlayerSavedItemCheckReport::default();

        if let Some(player) = read_level_dat_player(&level)? {
            report.record(
                PlayerSavedItemStorage::LevelDatPlayer,
                player.check_legacy_numeric_saved_items(table)?,
            );
        }
        if let Some(player) = read_local_player_with_level(self.storage(), &level)? {
            report.record(
                PlayerSavedItemStorage::LocalPlayer,
                player.check_legacy_numeric_saved_items(table)?,
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
                let item_report = crate::item::check_legacy_numeric_saved_items(&nbt, table)?;
                if item_report.is_fully_proven() {
                    report.add_counts(&item_report);
                } else {
                    report.record(
                        PlayerSavedItemStorage::PlayerKey(Bytes::copy_from_slice(entry.key)),
                        item_report,
                    );
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{MemoryStorage, WorldStorage};
    use crate::item::{SavedItemUpgradeSource, check_legacy_numeric_saved_items};
    use crate::nbt::{NbtTag, serialize_root_nbt};
    use indexmap::IndexMap;

    fn item(name: &str) -> NbtTag {
        NbtTag::Compound(IndexMap::from([
            ("Name".to_string(), NbtTag::String(name.to_string())),
            ("Count".to_string(), NbtTag::Byte(1)),
        ]))
    }

    fn player_with_item(name: &str) -> NbtTag {
        NbtTag::Compound(IndexMap::from([(
            "Inventory".to_string(),
            NbtTag::List(vec![item(name)]),
        )]))
    }

    #[test]
    fn aggregate_copies_raw_key_only_for_unresolved_server_player() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old":1}"#,
            "{}",
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:old":"minecraft:new"}}"#,
            }],
        )
        .unwrap();
        let storage = MemoryStorage::new();
        storage
            .put(
                b"player_good",
                &serialize_root_nbt(&player_with_item("minecraft:new")).unwrap(),
            )
            .unwrap();
        storage
            .put(
                b"player_\xff",
                &serialize_root_nbt(&player_with_item("minecraft:missing")).unwrap(),
            )
            .unwrap();

        let mut aggregate = WorldPlayerSavedItemCheckReport::default();
        storage
            .for_each_prefix_ref(
                b"player_",
                StorageReadOptions::default(),
                &mut |entry| {
                    let nbt = parse_root_nbt(entry.value)?;
                    let item_report = check_legacy_numeric_saved_items(&nbt, &table)?;
                    if item_report.is_fully_proven() {
                        aggregate.add_counts(&item_report);
                    } else {
                        aggregate.record(
                            PlayerSavedItemStorage::PlayerKey(Bytes::copy_from_slice(entry.key)),
                            item_report,
                        );
                    }
                    Ok(StorageVisitorControl::Continue)
                },
            )
            .unwrap();

        assert_eq!(aggregate.records_checked, 2);
        assert_eq!(aggregate.unresolved_records.len(), 1);
        assert_eq!(
            aggregate.unresolved_records[0].storage,
            PlayerSavedItemStorage::PlayerKey(Bytes::from_static(b"player_\xff"))
        );
    }

    #[test]
    fn report_counts_matching_physical_records_independently() {
        let mut report = WorldPlayerSavedItemCheckReport::default();
        report.record(
            PlayerSavedItemStorage::LevelDatPlayer,
            LegacySavedItemCheckReport {
                items_seen: 1,
                named_unique: 1,
                ..Default::default()
            },
        );
        report.record(
            PlayerSavedItemStorage::LocalPlayer,
            LegacySavedItemCheckReport {
                items_seen: 1,
                already_numeric: 1,
                ..Default::default()
            },
        );
        assert_eq!(report.records_checked, 2);
        assert_eq!(report.items_seen, 2);
        assert!(report.is_fully_proven());
    }
}
