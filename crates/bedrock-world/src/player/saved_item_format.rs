//! Explicit player saved-item operations for concrete Bedrock storage generations.

use crate::error::Result;
use crate::item::{
    LegacySavedItemBlockStateTables, LegacySavedItemIdTable, MedievalSavedItemCheckReport,
    MedievalSavedItemConversionReport, check_saved_items_for_medieval,
    check_saved_items_for_medieval_with_blocks, convert_saved_items_to_medieval,
    convert_saved_items_to_medieval_with_blocks,
};
use crate::player::PlayerData;

impl PlayerData {
    /// Checks whether every saved item in this player has a proven MCPE 1.6-1.8 representation.
    ///
    /// This is non-mutating. Modern blockitems carrying a `Block` payload remain unresolved until the
    /// `_with_blocks` variant is supplied authoritative block reverse tables.
    pub fn check_saved_items_for_medieval(
        &self,
        table: &LegacySavedItemIdTable,
    ) -> Result<MedievalSavedItemCheckReport> {
        check_saved_items_for_medieval(&self.nbt, table)
    }

    /// Checks the MCPE 1.6-1.8 representation including modern blockitem `Block` payloads.
    pub fn check_saved_items_for_medieval_with_blocks(
        &self,
        table: &LegacySavedItemIdTable,
        blocks: &LegacySavedItemBlockStateTables<'_>,
    ) -> Result<MedievalSavedItemCheckReport> {
        check_saved_items_for_medieval_with_blocks(&self.nbt, table, blocks)
    }

    /// Explicitly rewrites this player's saved items to MCPE 1.6-1.8 string-ID representation.
    ///
    /// The complete owned NBT tree is converted first; `self.nbt` changes only after success. Classic
    /// numeric sources become 1.6 endpoint names and every target `Damage` is TAG_Short. A modern
    /// `Block` payload makes this variant refuse the conversion rather than drop it.
    pub fn convert_saved_items_to_medieval(
        &mut self,
        table: &LegacySavedItemIdTable,
    ) -> Result<MedievalSavedItemConversionReport> {
        let outcome = convert_saved_items_to_medieval(&self.nbt, table)?;
        self.nbt = outcome.nbt;
        self.finish_edit();
        Ok(outcome.report)
    }

    /// Explicitly rewrites this player's saved items to Medieval format with blockitem proof.
    ///
    /// Proven modern `Block` payloads are removed only after item and block mappings agree on the
    /// same historical identity and metadata. The player remains unchanged on any error.
    pub fn convert_saved_items_to_medieval_with_blocks(
        &mut self,
        table: &LegacySavedItemIdTable,
        blocks: &LegacySavedItemBlockStateTables<'_>,
    ) -> Result<MedievalSavedItemConversionReport> {
        let outcome = convert_saved_items_to_medieval_with_blocks(&self.nbt, table, blocks)?;
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
    fn player_classic_item_becomes_medieval_only_after_success() {
        let table = LegacySavedItemIdTable::from_sources(
            r#"{"minecraft:nametag":421}"#,
            "{}",
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:nametag":"minecraft:name_tag"}}"#,
            }],
        )
        .unwrap();
        let nbt = NbtTag::Compound(IndexMap::from([(
            "Inventory".to_string(),
            NbtTag::List(vec![NbtTag::Compound(IndexMap::from([
                ("id".to_string(), NbtTag::Short(421)),
                ("Damage".to_string(), NbtTag::Short(0)),
                ("Count".to_string(), NbtTag::Byte(1)),
            ]))]),
        )]));
        let mut player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        let report = player.convert_saved_items_to_medieval(&table).unwrap();
        assert_eq!(report.check.items_seen, 1);
        assert!(player.is_modified());
        let NbtTag::Compound(root) = &player.nbt else { panic!("player") };
        let Some(NbtTag::List(items)) = root.get("Inventory") else { panic!("inventory") };
        let NbtTag::Compound(item) = &items[0] else { panic!("item") };
        assert_eq!(
            item.get("Name"),
            Some(&NbtTag::String("minecraft:name_tag".to_string()))
        );
        assert!(!item.contains_key("id"));
    }
}
