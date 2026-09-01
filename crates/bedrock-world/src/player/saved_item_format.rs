//! Explicit player saved-item operations for concrete Bedrock storage generations.

use crate::error::Result;
use crate::item::{
    ClassicSavedItemCheckReport, ClassicSavedItemConversionReport, MedievalSavedItemCheckReport,
    MedievalSavedItemConversionReport, SavedItemBlockStates, SavedItemFormatEvidence,
    SavedItemHistory, check_saved_items_for_classic, check_saved_items_for_medieval,
    convert_saved_items_to_classic, convert_saved_items_to_medieval, inspect_saved_item_formats,
};
use crate::player::PlayerData;

impl PlayerData {
    /// Inspects the actual saved-item storage forms present in this player without rewriting them.
    ///
    /// Plain string-ID items do not by themselves prove Medieval versus Modern source generation;
    /// callers can inspect `proven_format()` and `minimum_format()` on the returned evidence.
    pub fn saved_item_format_evidence(&self) -> Result<SavedItemFormatEvidence> {
        inspect_saved_item_formats(&self.nbt)
    }

    /// Checks whether every saved item has an exact MCPE <= 1.5 Classic representation.
    pub fn check_saved_items_for_classic(
        &self,
        table: &SavedItemHistory,
        blocks: Option<&SavedItemBlockStates<'_>>,
    ) -> Result<ClassicSavedItemCheckReport> {
        check_saved_items_for_classic(&self.nbt, table, blocks)
    }

    /// Explicitly rewrites every recognised saved item to exact Classic TAG_Short id + Damage.
    ///
    /// Existing numeric items are validated and normalized too; unknown numeric IDs, ID zero and
    /// values outside TAG_Short are refused. The complete owned NBT tree converts before `self.nbt`
    /// changes, so an error leaves the player untouched.
    pub fn convert_saved_items_to_classic(
        &mut self,
        table: &SavedItemHistory,
        blocks: Option<&SavedItemBlockStates<'_>>,
    ) -> Result<ClassicSavedItemConversionReport> {
        let outcome = convert_saved_items_to_classic(&self.nbt, table, blocks)?;
        self.nbt = outcome.nbt;
        self.finish_edit();
        Ok(outcome.report)
    }

    /// Checks whether every saved item in this player has a proven MCPE 1.6-1.8 representation.
    ///
    /// This is non-mutating. Pass authoritative block reverse tables when modern blockitems carry a
    /// `Block` payload; without them those payloads remain unresolved.
    pub fn check_saved_items_for_medieval(
        &self,
        table: &SavedItemHistory,
        blocks: Option<&SavedItemBlockStates<'_>>,
    ) -> Result<MedievalSavedItemCheckReport> {
        check_saved_items_for_medieval(&self.nbt, table, blocks)
    }

    /// Explicitly rewrites this player's saved items to MCPE 1.6-1.8 string-ID representation.
    ///
    /// The complete owned NBT tree is converted first; `self.nbt` changes only after success. Classic
    /// numeric sources become 1.6 endpoint names and every target `Damage` is TAG_Short. A modern
    /// `Block` payload makes this variant refuse the conversion rather than drop it.
    pub fn convert_saved_items_to_medieval(
        &mut self,
        table: &SavedItemHistory,
        blocks: Option<&SavedItemBlockStates<'_>>,
    ) -> Result<MedievalSavedItemConversionReport> {
        let outcome = convert_saved_items_to_medieval(&self.nbt, table, blocks)?;
        self.nbt = outcome.nbt;
        self.finish_edit();
        Ok(outcome.report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{SavedItemFormat, SavedItemUpgradeSource};
    use crate::nbt::NbtTag;
    use crate::player::PlayerId;
    use indexmap::IndexMap;

    fn player_with_item(item: NbtTag) -> PlayerData {
        PlayerData::from_nbt(
            PlayerId::Local,
            NbtTag::Compound(IndexMap::from([(
                "Inventory".to_string(),
                NbtTag::List(vec![item]),
            )])),
        )
        .unwrap()
    }

    #[test]
    fn player_reports_format_evidence_without_claiming_plain_string_source_generation() {
        let player = player_with_item(NbtTag::Compound(IndexMap::from([
            (
                "Name".to_string(),
                NbtTag::String("minecraft:apple".to_string()),
            ),
            ("Count".to_string(), NbtTag::Byte(1)),
        ])));
        let evidence = player.saved_item_format_evidence().unwrap();
        assert_eq!(evidence.minimum_format(), Some(SavedItemFormat::Medieval));
        assert_eq!(evidence.proven_format(), None);
        assert!(!player.is_modified());
    }

    #[test]
    fn player_exact_classic_normalizes_existing_numeric_width() {
        let table = SavedItemHistory::from_sources(r#"{"minecraft:stone":1}"#, "{}", &[]).unwrap();
        let item = NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::Int(1)),
            ("Damage".to_string(), NbtTag::Int(2)),
            ("Count".to_string(), NbtTag::Byte(1)),
        ]));
        let mut player = player_with_item(item);
        let report = player.convert_saved_items_to_classic(&table, None).unwrap();
        assert_eq!(report.items_changed, 1);
        assert!(player.is_modified());
    }

    #[test]
    fn player_classic_item_becomes_medieval_only_after_success() {
        let table = SavedItemHistory::from_sources(
            r#"{"minecraft:nametag":421}"#,
            "{}",
            &[SavedItemUpgradeSource {
                name: "0001_test.json",
                json: r#"{"renamedIds":{"minecraft:nametag":"minecraft:name_tag"}}"#,
            }],
        )
        .unwrap();
        let item = NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::Short(421)),
            ("Damage".to_string(), NbtTag::Short(0)),
            ("Count".to_string(), NbtTag::Byte(1)),
        ]));
        let mut player = player_with_item(item);
        let report = player
            .convert_saved_items_to_medieval(&table, None)
            .unwrap();
        assert_eq!(report.check.items_seen, 1);
        assert!(player.is_modified());
        let NbtTag::Compound(root) = &player.nbt else {
            panic!("player")
        };
        let Some(NbtTag::List(items)) = root.get("Inventory") else {
            panic!("inventory")
        };
        let NbtTag::Compound(item) = &items[0] else {
            panic!("item")
        };
        assert_eq!(
            item.get("Name"),
            Some(&NbtTag::String("minecraft:name_tag".to_string()))
        );
        assert!(!item.contains_key("id"));
    }
}
