//! Minecraft Bedrock player `Armor`, `Offhand` and `OffHandItem` fields.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use crate::player::{PlayerData, PlayerInventoryEntry};
use indexmap::IndexMap;

/// Borrowed item from an ordered Bedrock player equipment list.
#[derive(Debug, Clone, Copy)]
pub struct PlayerEquipmentEntry<'a> {
    /// Zero-based position in the persisted list.
    pub index: usize,
    /// Complete item compound.
    pub nbt: &'a IndexMap<String, NbtTag>,
}

/// Actual armor representation observed in the player NBT.
#[derive(Debug)]
pub enum PlayerArmor<'a> {
    /// No armor representation was found.
    None,
    /// Armor is persisted in the `Armor` list.
    ArmorTag(Vec<PlayerEquipmentEntry<'a>>),
    /// Armor is persisted as `Inventory` items using slots `100..=103`.
    InventorySlots(Vec<PlayerInventoryEntry<'a>>),
    /// Both representations exist and are retained independently.
    Both {
        /// Entries from the `Armor` list.
        armor: Vec<PlayerEquipmentEntry<'a>>,
        /// Entries from `Inventory` slots `100..=103`.
        inventory: Vec<PlayerInventoryEntry<'a>>,
    },
}

/// Actual offhand representation observed in the player NBT.
#[derive(Debug)]
pub enum PlayerOffhand<'a> {
    /// No offhand representation was found.
    None,
    /// Offhand data is persisted in the `Offhand` list.
    OffhandTag(Vec<PlayerEquipmentEntry<'a>>),
    /// Offhand data is persisted as the single `OffHandItem` compound.
    OffHandItem(&'a IndexMap<String, NbtTag>),
    /// Both persisted forms exist and are retained independently.
    Both {
        /// Entries from the `Offhand` list.
        offhand: Vec<PlayerEquipmentEntry<'a>>,
        /// The `OffHandItem` compound.
        off_hand_item: &'a IndexMap<String, NbtTag>,
    },
}

impl PlayerData {
    /// Detects the player's actual armor representation without rewriting either form.
    pub fn armor(&self) -> Result<PlayerArmor<'_>> {
        let armor = read_equipment_list(self.root()?, "Armor")?;
        let inventory = self
            .inventory()?
            .into_iter()
            .filter(|entry| entry.slot.is_some_and(|slot| slot.armor_index().is_some()))
            .collect::<Vec<_>>();
        Ok(match (armor.is_empty(), inventory.is_empty()) {
            (true, true) => PlayerArmor::None,
            (false, true) => PlayerArmor::ArmorTag(armor),
            (true, false) => PlayerArmor::InventorySlots(inventory),
            (false, false) => PlayerArmor::Both { armor, inventory },
        })
    }

    /// Detects the player's actual `Offhand`/`OffHandItem` representation.
    pub fn offhand(&self) -> Result<PlayerOffhand<'_>> {
        let offhand = read_equipment_list(self.root()?, "Offhand")?;
        let off_hand_item = match self.root()?.get("OffHandItem") {
            Some(NbtTag::Compound(item)) => Some(item),
            Some(other) => {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "player OffHandItem has unexpected NBT type: {other:?}"
                )));
            }
            None => None,
        };
        Ok(match (offhand.is_empty(), off_hand_item) {
            (true, None) => PlayerOffhand::None,
            (false, None) => PlayerOffhand::OffhandTag(offhand),
            (true, Some(item)) => PlayerOffhand::OffHandItem(item),
            (false, Some(item)) => PlayerOffhand::Both {
                offhand,
                off_hand_item: item,
            },
        })
    }

    /// Replaces the exact `Armor` list. This does not touch armor items in `Inventory`.
    pub fn set_armor_tag(&mut self, items: Vec<NbtTag>) -> Result<()> {
        validate_compound_list("Armor", &items)?;
        self.root_mut()?
            .insert("Armor".to_string(), NbtTag::List(items));
        self.finish_edit();
        Ok(())
    }

    /// Replaces the exact `Offhand` list. This does not touch `OffHandItem`.
    pub fn set_offhand_tag(&mut self, items: Vec<NbtTag>) -> Result<()> {
        validate_compound_list("Offhand", &items)?;
        self.root_mut()?
            .insert("Offhand".to_string(), NbtTag::List(items));
        self.finish_edit();
        Ok(())
    }

    /// Replaces the exact `OffHandItem` compound. This does not touch the `Offhand` list.
    pub fn set_off_hand_item(&mut self, item: NbtTag) -> Result<()> {
        if !matches!(item, NbtTag::Compound(_)) {
            return Err(BedrockWorldError::Validation(
                "OffHandItem must be an NBT compound".to_string(),
            ));
        }
        self.root_mut()?.insert("OffHandItem".to_string(), item);
        self.finish_edit();
        Ok(())
    }

    /// Removes only the `Armor` list, leaving `Inventory` armor slots untouched.
    pub fn remove_armor_tag(&mut self) -> Result<bool> {
        let removed = self.root_mut()?.shift_remove("Armor").is_some();
        if removed {
            self.finish_edit();
        }
        Ok(removed)
    }

    /// Removes only the `Offhand` list, leaving `OffHandItem` untouched.
    pub fn remove_offhand_tag(&mut self) -> Result<bool> {
        let removed = self.root_mut()?.shift_remove("Offhand").is_some();
        if removed {
            self.finish_edit();
        }
        Ok(removed)
    }

    /// Removes only `OffHandItem`, leaving the `Offhand` list untouched.
    pub fn remove_off_hand_item(&mut self) -> Result<bool> {
        let removed = self.root_mut()?.shift_remove("OffHandItem").is_some();
        if removed {
            self.finish_edit();
        }
        Ok(removed)
    }
}

fn read_equipment_list<'a>(
    root: &'a IndexMap<String, NbtTag>,
    field: &str,
) -> Result<Vec<PlayerEquipmentEntry<'a>>> {
    let Some(value) = root.get(field) else {
        return Ok(Vec::new());
    };
    let NbtTag::List(values) = value else {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "player {field} has unexpected NBT type: {value:?}"
        )));
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| match value {
            NbtTag::Compound(nbt) => Ok(PlayerEquipmentEntry { index, nbt }),
            _ => Err(BedrockWorldError::CorruptWorld(format!(
                "player {field}[{index}] is not an NBT compound"
            ))),
        })
        .collect()
}

fn validate_compound_list(field: &str, values: &[NbtTag]) -> Result<()> {
    for (index, value) in values.iter().enumerate() {
        if !matches!(value, NbtTag::Compound(_)) {
            return Err(BedrockWorldError::Validation(format!(
                "{field}[{index}] must be an NBT compound"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::{PlayerId, PlayerInventorySlot};

    fn item(name: &str) -> NbtTag {
        NbtTag::Compound(IndexMap::from([
            ("Name".to_string(), NbtTag::String(name.to_string())),
            ("Count".to_string(), NbtTag::Byte(1)),
        ]))
    }

    #[test]
    fn armor_keeps_both_historical_representations() {
        let mut player = PlayerData::from_nbt(
            PlayerId::Local,
            NbtTag::Compound(IndexMap::from([(
                "Armor".to_string(),
                NbtTag::List(vec![item("minecraft:iron_helmet")]),
            )])),
        )
        .unwrap();
        player
            .set_inventory_item(
                PlayerInventorySlot::from_raw(100),
                item("minecraft:leather_boots"),
            )
            .unwrap();
        assert!(matches!(player.armor().unwrap(), PlayerArmor::Both { .. }));
    }
}
