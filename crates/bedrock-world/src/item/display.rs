//! Saved-item custom name and lore fields.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use indexmap::IndexMap;

/// Writes an item's `tag.display.Name` and `tag.display.Lore` without changing its identity.
///
/// The NBT shape is used by Bedrock item implementations for custom names and lore. It is
/// suitable for labeling map tiles, chests and shulker-box items while leaving map pixels
/// unchanged. Unknown item, `tag` and `display` fields are preserved. This modifies only the
/// supplied NBT value; the caller owns any player, container, transaction or file write.
/// No saved-item version conversion occurs.
///
/// # Errors
///
/// Returns validation errors when the item is not a compound, its `tag`/`display` field has an
/// incompatible NBT type, or the name is empty.
pub fn set_item_display(item: &mut NbtTag, name: &str, lore: &[String]) -> Result<()> {
    if name.is_empty() {
        return Err(BedrockWorldError::Validation(
            "item display name is empty".to_string(),
        ));
    }
    let NbtTag::Compound(root) = item else {
        return Err(BedrockWorldError::Validation(
            "item is not an NBT compound".to_string(),
        ));
    };
    let tag = root
        .entry("tag".to_string())
        .or_insert_with(|| NbtTag::Compound(IndexMap::new()));
    let NbtTag::Compound(tag) = tag else {
        return Err(BedrockWorldError::Validation(
            "item tag is not an NBT compound".to_string(),
        ));
    };
    let display = tag
        .entry("display".to_string())
        .or_insert_with(|| NbtTag::Compound(IndexMap::new()));
    let NbtTag::Compound(display) = display else {
        return Err(BedrockWorldError::Validation(
            "item display is not an NBT compound".to_string(),
        ));
    };
    display.insert("Name".to_string(), NbtTag::String(name.to_string()));
    display.insert(
        "Lore".to_string(),
        NbtTag::List(lore.iter().cloned().map(NbtTag::String).collect()),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_item::{filled_map_id, new_filled_map_item};

    #[test]
    fn labeling_map_preserves_uuid_and_future_tags() {
        let mut item = new_filled_map_item(92, 4).expect("map");
        let NbtTag::Compound(root) = &mut item else {
            panic!("item")
        };
        let NbtTag::Compound(tag) = root.get_mut("tag").expect("tag") else {
            panic!("tag")
        };
        tag.insert("future".to_string(), NbtTag::Byte(7));
        set_item_display(&mut item, "BMCBL 1/2", &["Northwest tile".to_string()]).expect("label");
        assert_eq!(
            filled_map_id(&item).as_ref().map(|id| id.as_str()),
            Some("92")
        );
        let NbtTag::Compound(root) = item else {
            panic!("item")
        };
        let NbtTag::Compound(tag) = &root["tag"] else {
            panic!("tag")
        };
        assert_eq!(tag["future"], NbtTag::Byte(7));
        let NbtTag::Compound(display) = &tag["display"] else {
            panic!("display")
        };
        assert_eq!(display["Name"], NbtTag::String("BMCBL 1/2".to_string()));
        assert_eq!(
            display["Lore"],
            NbtTag::List(vec![NbtTag::String("Northwest tile".to_string())])
        );
    }
}
