//! Embedded item-id to item-icon mapping generated from the game resource packs.
//!
//! Bedrock resolves inventory icons from an internal item -> icon table; the icon names are not
//! always equal to the item id (legacy dye names, wood_*/gold_* tool names, music discs, ...).
//! `tools/generate_item_texture_map.py --game-dir <version dir>` extracts the mapping from the
//! creative item catalog plus every `textures/item_texture.json`/`textures/items` entry; this
//! module embeds the result so item rendering (3D editor item frames, OBJ export) does not have
//! to rescan the game directory.
use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

const ITEM_MAP_JSON: &str = include_str!("../generated/item_texture_map.json");

#[derive(Debug, Deserialize)]
struct ItemMapJson {
    items: BTreeMap<String, ItemEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ItemEntry {
    pub icon: String,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub definition_pack: Option<String>,
}

fn item_map() -> &'static BTreeMap<String, ItemEntry> {
    static ITEMS: OnceLock<BTreeMap<String, ItemEntry>> = OnceLock::new();
    ITEMS.get_or_init(|| {
        let parsed: ItemMapJson =
            serde_json::from_str(ITEM_MAP_JSON).expect("embedded item_texture_map.json must parse");
        parsed.items
    })
}

/// Icon name for an item id (with or without the `minecraft:` prefix).
#[must_use]
pub fn item_icon_name(item: &str) -> Option<&'static str> {
    let normalized = item.strip_prefix("minecraft:").unwrap_or(item);
    item_map().get(normalized).map(|entry| entry.icon.as_str())
}

/// Full mapping entry for an item id.
#[must_use]
pub fn item_icon_entry(item: &str) -> Option<&'static ItemEntry> {
    let normalized = item.strip_prefix("minecraft:").unwrap_or(item);
    item_map().get(normalized)
}

/// Number of embedded item mappings.
#[must_use]
pub fn item_icon_count() -> usize {
    item_map().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_covers_common_items() {
        assert!(item_icon_count() > 400);
        assert_eq!(item_icon_name("minecraft:apple"), Some("apple"));
        assert_eq!(
            item_icon_name("minecraft:prismarine_crystals"),
            Some("prismarine_crystals")
        );
        assert_eq!(item_icon_name("prismarine_shard"), Some("prismarine_shard"));
    }

    #[test]
    fn legacy_item_ids_map_to_renamed_icons() {
        assert_eq!(
            item_icon_name("minecraft:ink_sac"),
            Some("dye_powder_black_new")
        );
        assert_eq!(item_icon_name("minecraft:wooden_sword"), Some("wood_sword"));
        assert_eq!(item_icon_name("minecraft:golden_sword"), Some("gold_sword"));
    }
}
