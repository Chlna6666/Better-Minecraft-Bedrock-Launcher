//! Minecraft Bedrock player `Inventory`, `EnderChestInventory` and selected hotbar slot fields.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use crate::player::PlayerData;
use indexmap::IndexMap;

/// Raw `Slot` byte stored in a Bedrock saved-item compound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlayerInventorySlot(i8);

impl PlayerInventorySlot {
    /// Creates a slot from the exact persisted `Slot` byte.
    #[must_use]
    pub const fn from_raw(raw: i8) -> Self {
        Self(raw)
    }

    /// Returns the exact persisted `Slot` byte.
    #[must_use]
    pub const fn raw(self) -> i8 {
        self.0
    }

    /// Returns the historical hotbar index for raw slots `0..=8`.
    #[must_use]
    pub const fn historical_hotbar_index(self) -> Option<u8> {
        if self.0 >= 0 && self.0 <= 8 {
            Some(self.0 as u8)
        } else {
            None
        }
    }

    /// Returns the main-inventory index for raw slots `9..=44`.
    #[must_use]
    pub const fn inventory_index(self) -> Option<u8> {
        if self.0 >= 9 && self.0 <= 44 {
            Some((self.0 - 9) as u8)
        } else {
            None
        }
    }

    /// Returns the armor index for raw slots `100..=103`.
    #[must_use]
    pub const fn armor_index(self) -> Option<u8> {
        if self.0 >= 100 && self.0 <= 103 {
            Some((self.0 - 100) as u8)
        } else {
            None
        }
    }
}

/// Borrowed saved item found in a player inventory list.
#[derive(Debug, Clone, Copy)]
pub struct PlayerInventoryEntry<'a> {
    /// Exact persisted slot, when the item compound contains `Slot`.
    pub slot: Option<PlayerInventorySlot>,
    /// Complete item NBT compound.
    pub nbt: &'a IndexMap<String, NbtTag>,
}

impl PlayerData {
    /// Returns all compounds from the player's `Inventory` list.
    ///
    /// Unknown fields inside each item are retained. A malformed non-compound entry is reported
    /// instead of being silently skipped.
    pub fn inventory(&self) -> Result<Vec<PlayerInventoryEntry<'_>>> {
        read_item_list(self.root()?, "Inventory")
    }

    /// Returns one `Inventory` entry with the requested exact persisted `Slot`.
    pub fn inventory_item(
        &self,
        slot: PlayerInventorySlot,
    ) -> Result<Option<PlayerInventoryEntry<'_>>> {
        find_item(self.root()?, "Inventory", slot)
    }

    /// Inserts or replaces one `Inventory` item at an exact persisted `Slot`.
    ///
    /// The supplied compound is retained as-is except that its `Slot` field is set to the requested
    /// Bedrock slot byte. No item-id or metadata version rewrite is performed.
    pub fn set_inventory_item(&mut self, slot: PlayerInventorySlot, item: NbtTag) -> Result<()> {
        set_item(self, "Inventory", slot, item)
    }

    /// Removes one `Inventory` item at an exact persisted `Slot`.
    pub fn remove_inventory_item(&mut self, slot: PlayerInventorySlot) -> Result<bool> {
        remove_item(self, "Inventory", slot)
    }

    /// Returns all compounds from `EnderChestInventory`.
    pub fn ender_chest_inventory(&self) -> Result<Vec<PlayerInventoryEntry<'_>>> {
        read_item_list(self.root()?, "EnderChestInventory")
    }

    /// Returns one `EnderChestInventory` entry by its exact persisted `Slot`.
    pub fn ender_chest_item(
        &self,
        slot: PlayerInventorySlot,
    ) -> Result<Option<PlayerInventoryEntry<'_>>> {
        find_item(self.root()?, "EnderChestInventory", slot)
    }

    /// Inserts or replaces one `EnderChestInventory` item.
    pub fn set_ender_chest_item(&mut self, slot: PlayerInventorySlot, item: NbtTag) -> Result<()> {
        if !(0..=26).contains(&slot.raw()) {
            return Err(BedrockWorldError::Validation(format!(
                "EnderChestInventory Slot must be 0..=26, got {}",
                slot.raw()
            )));
        }
        set_item(self, "EnderChestInventory", slot, item)
    }

    /// Removes one `EnderChestInventory` item.
    pub fn remove_ender_chest_item(&mut self, slot: PlayerInventorySlot) -> Result<bool> {
        if !(0..=26).contains(&slot.raw()) {
            return Err(BedrockWorldError::Validation(format!(
                "EnderChestInventory Slot must be 0..=26, got {}",
                slot.raw()
            )));
        }
        remove_item(self, "EnderChestInventory", slot)
    }

    /// Returns the exact `SelectedInventorySlot` integer when present.
    pub fn selected_inventory_slot(&self) -> Result<Option<i32>> {
        integer_tag(
            self.root()?.get("SelectedInventorySlot"),
            "SelectedInventorySlot",
        )
    }

    /// Sets `SelectedInventorySlot`.
    pub fn set_selected_inventory_slot(&mut self, slot: i32) -> Result<()> {
        if !(0..=8).contains(&slot) {
            return Err(BedrockWorldError::Validation(format!(
                "SelectedInventorySlot must be 0..=8, got {slot}"
            )));
        }
        let root = self.root_mut()?;
        set_integer_preserving_type(root, "SelectedInventorySlot", slot)?;
        self.finish_edit();
        Ok(())
    }
}

fn read_item_list<'a>(
    root: &'a IndexMap<String, NbtTag>,
    field: &str,
) -> Result<Vec<PlayerInventoryEntry<'a>>> {
    let Some(value) = root.get(field) else {
        return Ok(Vec::new());
    };
    let NbtTag::List(values) = value else {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "player {field} has unexpected NBT type: {value:?}"
        )));
    };
    let mut items = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let NbtTag::Compound(item) = value else {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "player {field}[{index}] is not an NBT compound"
            )));
        };
        items.push(PlayerInventoryEntry {
            slot: item_slot(item, field, index)?,
            nbt: item,
        });
    }
    Ok(items)
}

fn find_item<'a>(
    root: &'a IndexMap<String, NbtTag>,
    field: &str,
    slot: PlayerInventorySlot,
) -> Result<Option<PlayerInventoryEntry<'a>>> {
    let mut found = None;
    for entry in read_item_list(root, field)? {
        if entry.slot == Some(slot) {
            if found.is_some() {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "player {field} contains duplicate Slot {}",
                    slot.raw()
                )));
            }
            found = Some(entry);
        }
    }
    Ok(found)
}

fn set_item(
    player: &mut PlayerData,
    field: &str,
    slot: PlayerInventorySlot,
    item: NbtTag,
) -> Result<()> {
    let NbtTag::Compound(mut item) = item else {
        return Err(BedrockWorldError::Validation(format!(
            "{field} item must be an NBT compound"
        )));
    };
    item.insert("Slot".to_string(), NbtTag::Byte(slot.raw()));

    {
        let root = player.root_mut()?;
        let value = root
            .entry(field.to_string())
            .or_insert_with(|| NbtTag::List(Vec::new()));
        let NbtTag::List(values) = value else {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "player {field} has unexpected NBT type"
            )));
        };

        let mut found = None;
        for (index, value) in values.iter().enumerate() {
            let NbtTag::Compound(existing) = value else {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "player {field}[{index}] is not an NBT compound"
                )));
            };
            if item_slot(existing, field, index)? == Some(slot) {
                if found.replace(index).is_some() {
                    return Err(BedrockWorldError::CorruptWorld(format!(
                        "player {field} contains duplicate Slot {}",
                        slot.raw()
                    )));
                }
            }
        }
        if let Some(index) = found {
            values[index] = NbtTag::Compound(item);
        } else {
            values.push(NbtTag::Compound(item));
        }
    }
    player.finish_edit();
    Ok(())
}

fn remove_item(player: &mut PlayerData, field: &str, slot: PlayerInventorySlot) -> Result<bool> {
    let removed = {
        let root = player.root_mut()?;
        let Some(value) = root.get_mut(field) else {
            return Ok(false);
        };
        let NbtTag::List(values) = value else {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "player {field} has unexpected NBT type"
            )));
        };

        let mut found = None;
        for (index, value) in values.iter().enumerate() {
            let NbtTag::Compound(existing) = value else {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "player {field}[{index}] is not an NBT compound"
                )));
            };
            if item_slot(existing, field, index)? == Some(slot) {
                if found.replace(index).is_some() {
                    return Err(BedrockWorldError::CorruptWorld(format!(
                        "player {field} contains duplicate Slot {}",
                        slot.raw()
                    )));
                }
            }
        }
        if let Some(index) = found {
            values.remove(index);
            true
        } else {
            false
        }
    };
    if removed {
        player.finish_edit();
    }
    Ok(removed)
}

fn item_slot(
    item: &IndexMap<String, NbtTag>,
    field: &str,
    index: usize,
) -> Result<Option<PlayerInventorySlot>> {
    let Some(value) = integer_tag(item.get("Slot"), "Slot")? else {
        return Ok(None);
    };
    let raw = i8::try_from(value).map_err(|_| {
        BedrockWorldError::CorruptWorld(format!(
            "player {field}[{index}] Slot {value} does not fit a Bedrock byte"
        ))
    })?;
    Ok(Some(PlayerInventorySlot::from_raw(raw)))
}

pub(crate) fn integer_tag(tag: Option<&NbtTag>, field: &str) -> Result<Option<i32>> {
    let Some(tag) = tag else {
        return Ok(None);
    };
    let value = match tag {
        NbtTag::Byte(value) => i32::from(*value),
        NbtTag::Short(value) => i32::from(*value),
        NbtTag::Int(value) => *value,
        NbtTag::Long(value) => i32::try_from(*value).map_err(|_| {
            BedrockWorldError::CorruptWorld(format!("{field} value {value} does not fit i32"))
        })?,
        other => {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "{field} has unexpected NBT type: {other:?}"
            )));
        }
    };
    Ok(Some(value))
}

pub(crate) fn set_integer_preserving_type(
    root: &mut IndexMap<String, NbtTag>,
    field: &str,
    value: i32,
) -> Result<()> {
    let tag = match root.get(field) {
        Some(NbtTag::Byte(_)) => NbtTag::Byte(i8::try_from(value).map_err(|_| {
            BedrockWorldError::Validation(format!("{field} value {value} does not fit byte"))
        })?),
        Some(NbtTag::Short(_)) => NbtTag::Short(i16::try_from(value).map_err(|_| {
            BedrockWorldError::Validation(format!("{field} value {value} does not fit short"))
        })?),
        Some(NbtTag::Long(_)) => NbtTag::Long(i64::from(value)),
        Some(NbtTag::Int(_)) | None => NbtTag::Int(value),
        Some(other) => {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "{field} has unexpected NBT type: {other:?}"
            )));
        }
    };
    root.insert(field.to_string(), tag);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PlayerId;

    fn item(name: &str, slot: i8) -> NbtTag {
        NbtTag::Compound(IndexMap::from([
            ("Name".to_string(), NbtTag::String(name.to_string())),
            ("Count".to_string(), NbtTag::Byte(1)),
            ("Slot".to_string(), NbtTag::Byte(slot)),
            ("FutureField".to_string(), NbtTag::Long(99)),
        ]))
    }

    #[test]
    fn classifies_historical_slot_ranges_without_losing_raw_value() {
        let hotbar = PlayerInventorySlot::from_raw(3);
        let inventory = PlayerInventorySlot::from_raw(12);
        let armor = PlayerInventorySlot::from_raw(102);
        assert_eq!(hotbar.historical_hotbar_index(), Some(3));
        assert_eq!(inventory.inventory_index(), Some(3));
        assert_eq!(armor.armor_index(), Some(2));
        assert_eq!(armor.raw(), 102);
    }

    #[test]
    fn inventory_replace_preserves_supplied_unknown_item_fields() {
        let nbt = NbtTag::Compound(IndexMap::from([(
            "Inventory".to_string(),
            NbtTag::List(vec![item("minecraft:stone", 9)]),
        )]));
        let mut player = PlayerData::from_nbt(PlayerId::Local, nbt).unwrap();
        let replacement = item("minecraft:dirt", 0);
        player
            .set_inventory_item(PlayerInventorySlot::from_raw(9), replacement)
            .unwrap();
        let entry = player
            .inventory_item(PlayerInventorySlot::from_raw(9))
            .unwrap()
            .unwrap();
        assert_eq!(
            entry.nbt.get("Name"),
            Some(&NbtTag::String("minecraft:dirt".to_string()))
        );
        assert_eq!(entry.nbt.get("FutureField"), Some(&NbtTag::Long(99)));
        assert_eq!(entry.nbt.get("Slot"), Some(&NbtTag::Byte(9)));
        assert!(player.is_modified());
    }

    #[test]
    fn selected_inventory_slot_is_validated() {
        let mut player =
            PlayerData::from_nbt(PlayerId::Local, NbtTag::Compound(IndexMap::new())).unwrap();
        player.set_selected_inventory_slot(8).unwrap();
        assert_eq!(player.selected_inventory_slot().unwrap(), Some(8));
        assert!(player.set_selected_inventory_slot(9).is_err());
    }
}
