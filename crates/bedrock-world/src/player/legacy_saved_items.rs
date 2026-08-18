//! Historical saved-item representation checks for Minecraft Bedrock player NBT.

use crate::error::Result;
use crate::item::{
    LegacySavedItemBlockStateTables, LegacySavedItemCheckReport, LegacySavedItemIdTable,
    check_legacy_numeric_saved_items, check_legacy_numeric_saved_items_with_blocks,
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
}
