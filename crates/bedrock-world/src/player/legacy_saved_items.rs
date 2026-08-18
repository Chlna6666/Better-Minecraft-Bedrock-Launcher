//! Historical saved-item representation checks and explicit conversion for Minecraft Bedrock player NBT.

use crate::error::Result;
use crate::item::{
    LegacySavedItemBlockStateTables, LegacySavedItemCheckReport, LegacySavedItemConversionReport,
    LegacySavedItemIdTable, check_legacy_numeric_saved_items,
    check_legacy_numeric_saved_items_with_blocks, convert_saved_items_to_legacy_numeric,
    convert_saved_items_to_legacy_numeric_with_blocks,
};
use crate::player::PlayerData;

impl PlayerData {
    /// Checks whether every named saved item in this player has an exact historical numeric ID/meta.
    ///
    /// This is non-mutating. Block items carrying a `Block` BlockState remain reported as requiring
    /// independent BlockState reverse validation even when their item ID/meta is unique.
    pub fn check_legacy_numeric_saved_items(
        &self,
        table: &LegacySavedItemIdTable,
    ) -> Result<LegacySavedItemCheckReport> {
        check_legacy_numeric_saved_items(&self.nbt, table)
    }

    /// Checks historical numeric saved-item representation including persisted blockitem BlockStates.
    ///
    /// The supplied block tables must represent the same historical numeric block corpus: the raw
    /// table provides the old block identity, while the upgraded table proves the modern BlockState by
    /// forward upgrade. This method never mutates player NBT.
    pub fn check_legacy_numeric_saved_items_with_blocks(
        &self,
        table: &LegacySavedItemIdTable,
        blocks: &LegacySavedItemBlockStateTables<'_>,
    ) -> Result<LegacySavedItemCheckReport> {
        check_legacy_numeric_saved_items_with_blocks(&self.nbt, table, blocks)
    }

    /// Explicitly converts named saved items in this player to historical numeric `id` + `Damage`.
    ///
    /// The player is changed only after the complete owned NBT tree passes preflight and conversion.
    /// Named blockitems are refused unless BlockState context is supplied through the `_with_blocks`
    /// variant. Existing numeric saved items remain untouched.
    pub fn convert_saved_items_to_legacy_numeric(
        &mut self,
        table: &LegacySavedItemIdTable,
    ) -> Result<LegacySavedItemConversionReport> {
        let outcome = convert_saved_items_to_legacy_numeric(&self.nbt, table)?;
        self.nbt = outcome.nbt;
        self.finish_edit();
        Ok(outcome.report)
    }

    /// Explicitly converts named saved items to historical numeric form with blockitem proof.
    ///
    /// Proven modern `Block` payloads are removed only when the historical item and block mappings
    /// agree on the same old block identifier and metadata. The player is unchanged on any error.
    pub fn convert_saved_items_to_legacy_numeric_with_blocks(
        &mut self,
        table: &LegacySavedItemIdTable,
        blocks: &LegacySavedItemBlockStateTables<'_>,
    ) -> Result<LegacySavedItemConversionReport> {
        let outcome = convert_saved_items_to_legacy_numeric_with_blocks(&self.nbt, table, blocks)?;
        self.nbt = outcome.nbt;
        self.finish_edit();
        Ok(outcome.report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::SavedItemUpgradeSource;
    use crate::nbt::NbtTag;
    use crate::player::PlayerId;
    use indexmap::IndexMap;

    #[test]
    fn player_method_checks_complete_owned_nbt_without_rewriting_it() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old":1}"#,
            "{}",
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:old":"minecraft:new"}}"#,
            }],
        )
        .unwrap();
        let nbt = NbtTag::Compound(IndexMap::from([(
            "Inventory".to_string(),
            NbtTag::List(vec![NbtTag::Compound(IndexMap::from([
                (
                    "Name".to_string(),
                    NbtTag::String("minecraft:new".to_string()),
                ),
                ("Count".to_string(), NbtTag::Byte(1)),
            ]))]),
        )]));
        let player = PlayerData::from_nbt(PlayerId::Local, nbt.clone()).unwrap();
        let report = player.check_legacy_numeric_saved_items(&table).unwrap();
        assert_eq!(report.items_seen, 1);
        assert_eq!(report.named_unique, 1);
        assert!(report.is_fully_proven());
        assert_eq!(player.nbt, nbt);
        assert!(!player.is_modified());
    }

    #[test]
    fn player_conversion_marks_record_modified_only_after_success() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:old":1}"#,
            "{}",
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:old":"minecraft:new"}}"#,
            }],
        )
        .unwrap();
        let nbt = NbtTag::Compound(IndexMap::from([(
            "Inventory".to_string(),
            NbtTag::List(vec![NbtTag::Compound(IndexMap::from([
                (
                    "Name".to_string(),
                    NbtTag::String("minecraft:new".to_string()),
                ),
                ("Count".to_string(), NbtTag::Byte(1)),
            ]))]),
        )]));
        let mut player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        let report = player.convert_saved_items_to_legacy_numeric(&table).unwrap();
        assert_eq!(report.converted, 1);
        assert!(player.is_modified());
        let NbtTag::Compound(root) = &player.nbt else {
            panic!("player root must be compound");
        };
        let Some(NbtTag::List(inventory)) = root.get("Inventory") else {
            panic!("Inventory must be a list");
        };
        let NbtTag::Compound(item) = &inventory[0] else {
            panic!("item must be compound");
        };
        assert_eq!(item.get("id"), Some(&NbtTag::Short(1)));
        assert!(!item.contains_key("Name"));
    }
}
