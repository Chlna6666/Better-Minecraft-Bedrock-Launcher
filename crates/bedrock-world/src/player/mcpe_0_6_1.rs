//! Exact MCPE 0.6.1 `level.dat.Player` compatibility checks and writes.
//!
//! This module does not infer or invent old values. Callers must first prepare saved items and any
//! other historical fields explicitly. The writer only accepts the concrete NBT shapes emitted by the
//! confirmed MCPE 0.6.1 player save path, while retaining unrelated fields verbatim.

use crate::error::{BedrockWorldError, Result};
use crate::level::LevelDatDocument;
use crate::nbt::NbtTag;
use crate::player::PlayerData;
use indexmap::IndexMap;

const MCPE_0_6_1_LEVEL_VERSION: u32 = 3;
const MCPE_0_6_1_STORAGE_VERSION: i32 = 3;

/// Compatibility report for writing one player as MCPE 0.6.1 `level.dat.Player`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mcpe061PlayerCheckReport {
    /// Human-readable persisted-shape violations. An empty list means the player can be written.
    pub issues: Vec<String>,
}

impl Mcpe061PlayerCheckReport {
    /// Returns whether no incompatible persisted shape was found.
    #[must_use]
    pub fn is_compatible(&self) -> bool {
        self.issues.is_empty()
    }
}

impl PlayerData {
    /// Checks the exact player fields required by the confirmed MCPE 0.6.1 save representation.
    ///
    /// Unknown fields are intentionally ignored and therefore preserved by the writer. Known newer
    /// storage such as `Offhand`/`OffHandItem` is rejected because MCPE 0.6.1 has no equivalent slot.
    pub fn check_for_mcpe_0_6_1(&self) -> Result<Mcpe061PlayerCheckReport> {
        let root = self.root()?;
        let mut report = Mcpe061PlayerCheckReport::default();

        require_float_list(root, "Pos", 3, &mut report);
        require_float_list(root, "Motion", 3, &mut report);
        require_float_list(root, "Rotation", 2, &mut report);
        require_type(root, "Health", is_short, "Short", &mut report);
        require_type(root, "Sleeping", is_byte, "Byte", &mut report);
        require_type(root, "SleepTimer", is_short, "Short", &mut report);
        for field in [
            "BedPositionX",
            "BedPositionY",
            "BedPositionZ",
            "SpawnX",
            "SpawnY",
            "SpawnZ",
        ] {
            require_type(root, field, is_int, "Int", &mut report);
        }

        check_inventory(root, &mut report);
        check_armor(root, &mut report);
        reject_newer_equipment(root, &mut report);
        Ok(report)
    }
}

/// Writes a player into a level already configured for the concrete MCPE 0.6.1 world format.
///
/// The target must use level header version `3` and `StorageVersion=3`. The player is preflighted
/// completely before the target document is changed. This function does not alter world terrain,
/// saved-item IDs, player source storage, or any unrelated `level.dat` field.
pub fn write_mcpe_0_6_1_level_dat_player(
    document: &mut LevelDatDocument,
    player: &PlayerData,
) -> Result<()> {
    if document.header.version != MCPE_0_6_1_LEVEL_VERSION {
        return Err(BedrockWorldError::Validation(format!(
            "MCPE 0.6.1 player target requires level.dat header version {MCPE_0_6_1_LEVEL_VERSION}, got {}",
            document.header.version
        )));
    }
    let NbtTag::Compound(level) = &document.root else {
        return Err(BedrockWorldError::CorruptWorld(
            "level.dat root is not an NBT compound".to_string(),
        ));
    };
    match level.get("StorageVersion") {
        Some(NbtTag::Int(MCPE_0_6_1_STORAGE_VERSION)) => {}
        Some(value) => {
            return Err(BedrockWorldError::Validation(format!(
                "MCPE 0.6.1 player target requires level.dat StorageVersion=3 Int, got {value:?}"
            )));
        }
        None => {
            return Err(BedrockWorldError::Validation(
                "MCPE 0.6.1 player target requires level.dat StorageVersion=3".to_string(),
            ));
        }
    }

    let report = player.check_for_mcpe_0_6_1()?;
    if !report.is_compatible() {
        return Err(BedrockWorldError::Validation(format!(
            "player is not writable as MCPE 0.6.1 level.dat.Player: {}",
            report.issues.join("; ")
        )));
    }

    let NbtTag::Compound(level) = &mut document.root else {
        unreachable!("validated above");
    };
    level.insert("Player".to_string(), player.nbt.clone());
    Ok(())
}

fn require_float_list(
    root: &IndexMap<String, NbtTag>,
    field: &str,
    expected_len: usize,
    report: &mut Mcpe061PlayerCheckReport,
) {
    let Some(value) = root.get(field) else {
        report.issues.push(format!("missing {field}"));
        return;
    };
    let NbtTag::List(values) = value else {
        report.issues.push(format!("{field} must be a List"));
        return;
    };
    if values.len() != expected_len {
        report.issues.push(format!(
            "{field} must contain {expected_len} Float values, got {}",
            values.len()
        ));
        return;
    }
    if values
        .iter()
        .any(|value| !matches!(value, NbtTag::Float(_)))
    {
        report
            .issues
            .push(format!("{field} must contain only Float values"));
    }
}

fn require_type(
    root: &IndexMap<String, NbtTag>,
    field: &str,
    matches_type: fn(&NbtTag) -> bool,
    expected: &str,
    report: &mut Mcpe061PlayerCheckReport,
) {
    match root.get(field) {
        Some(value) if matches_type(value) => {}
        Some(_) => report.issues.push(format!("{field} must be {expected}")),
        None => report.issues.push(format!("missing {field}")),
    }
}

fn check_inventory(root: &IndexMap<String, NbtTag>, report: &mut Mcpe061PlayerCheckReport) {
    let Some(value) = root.get("Inventory") else {
        report.issues.push("missing Inventory".to_string());
        return;
    };
    let NbtTag::List(items) = value else {
        report.issues.push("Inventory must be a List".to_string());
        return;
    };
    for (index, item) in items.iter().enumerate() {
        let NbtTag::Compound(item) = item else {
            report
                .issues
                .push(format!("Inventory[{index}] must be a Compound"));
            continue;
        };
        require_item_field(item, index, "Inventory", "Slot", is_byte, "Byte", report);
        require_item_field(item, index, "Inventory", "id", is_short, "Short", report);
        require_item_field(item, index, "Inventory", "Count", is_byte, "Byte", report);
        require_item_field(
            item,
            index,
            "Inventory",
            "Damage",
            is_short,
            "Short",
            report,
        );
        if item.contains_key("Name") || matches!(item.get("id"), Some(NbtTag::String(_))) {
            report.issues.push(format!(
                "Inventory[{index}] still contains a named saved-item representation"
            ));
        }
        if item.contains_key("Block") {
            report.issues.push(format!(
                "Inventory[{index}] still contains a newer BlockState payload"
            ));
        }
    }
}

fn check_armor(root: &IndexMap<String, NbtTag>, report: &mut Mcpe061PlayerCheckReport) {
    let Some(value) = root.get("Armor") else {
        report.issues.push("missing Armor".to_string());
        return;
    };
    let NbtTag::List(items) = value else {
        report.issues.push("Armor must be a List".to_string());
        return;
    };
    if items.len() != 4 {
        report
            .issues
            .push(format!("Armor must contain 4 entries, got {}", items.len()));
    }
    for (index, item) in items.iter().enumerate() {
        let NbtTag::Compound(item) = item else {
            report
                .issues
                .push(format!("Armor[{index}] must be a Compound"));
            continue;
        };
        require_item_field(item, index, "Armor", "id", is_short, "Short", report);
        require_item_field(item, index, "Armor", "Count", is_byte, "Byte", report);
        require_item_field(item, index, "Armor", "Damage", is_short, "Short", report);
        if item.contains_key("Name") || matches!(item.get("id"), Some(NbtTag::String(_))) {
            report.issues.push(format!(
                "Armor[{index}] still contains a named saved-item representation"
            ));
        }
        if item.contains_key("Block") {
            report.issues.push(format!(
                "Armor[{index}] still contains a newer BlockState payload"
            ));
        }
    }
}

fn reject_newer_equipment(root: &IndexMap<String, NbtTag>, report: &mut Mcpe061PlayerCheckReport) {
    if let Some(NbtTag::List(values)) = root.get("Offhand")
        && !values.is_empty()
    {
        report
            .issues
            .push("Offhand contains items that MCPE 0.6.1 cannot represent".to_string());
    } else if root
        .get("Offhand")
        .is_some_and(|value| !matches!(value, NbtTag::List(_)))
    {
        report
            .issues
            .push("Offhand must be a List when present".to_string());
    }
    if root.contains_key("OffHandItem") {
        report
            .issues
            .push("OffHandItem is not representable in MCPE 0.6.1".to_string());
    }
}

fn require_item_field(
    item: &IndexMap<String, NbtTag>,
    index: usize,
    list: &str,
    field: &str,
    matches_type: fn(&NbtTag) -> bool,
    expected: &str,
    report: &mut Mcpe061PlayerCheckReport,
) {
    match item.get(field) {
        Some(value) if matches_type(value) => {}
        Some(_) => report
            .issues
            .push(format!("{list}[{index}].{field} must be {expected}")),
        None => report
            .issues
            .push(format!("{list}[{index}] is missing {field}")),
    }
}

fn is_byte(value: &NbtTag) -> bool {
    matches!(value, NbtTag::Byte(_))
}

fn is_short(value: &NbtTag) -> bool {
    matches!(value, NbtTag::Short(_))
}

fn is_int(value: &NbtTag) -> bool {
    matches!(value, NbtTag::Int(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PlayerId;

    fn item(slot: Option<i8>) -> NbtTag {
        let mut item = IndexMap::from([
            ("id".to_string(), NbtTag::Short(1)),
            ("Count".to_string(), NbtTag::Byte(1)),
            ("Damage".to_string(), NbtTag::Short(0)),
        ]);
        if let Some(slot) = slot {
            item.insert("Slot".to_string(), NbtTag::Byte(slot));
        }
        NbtTag::Compound(item)
    }

    fn player() -> PlayerData {
        PlayerData::from_nbt(
            PlayerId::Local,
            NbtTag::Compound(IndexMap::from([
                (
                    "Pos".to_string(),
                    NbtTag::List(vec![
                        NbtTag::Float(1.0),
                        NbtTag::Float(64.0),
                        NbtTag::Float(2.0),
                    ]),
                ),
                (
                    "Motion".to_string(),
                    NbtTag::List(vec![
                        NbtTag::Float(0.0),
                        NbtTag::Float(0.0),
                        NbtTag::Float(0.0),
                    ]),
                ),
                (
                    "Rotation".to_string(),
                    NbtTag::List(vec![NbtTag::Float(0.0), NbtTag::Float(0.0)]),
                ),
                ("Health".to_string(), NbtTag::Short(20)),
                ("Sleeping".to_string(), NbtTag::Byte(0)),
                ("SleepTimer".to_string(), NbtTag::Short(0)),
                ("BedPositionX".to_string(), NbtTag::Int(0)),
                ("BedPositionY".to_string(), NbtTag::Int(0)),
                ("BedPositionZ".to_string(), NbtTag::Int(0)),
                ("SpawnX".to_string(), NbtTag::Int(0)),
                ("SpawnY".to_string(), NbtTag::Int(64)),
                ("SpawnZ".to_string(), NbtTag::Int(0)),
                ("Inventory".to_string(), NbtTag::List(vec![item(Some(0))])),
                (
                    "Armor".to_string(),
                    NbtTag::List(vec![item(None), item(None), item(None), item(None)]),
                ),
                ("FutureField".to_string(), NbtTag::Long(9)),
            ])),
        )
        .unwrap()
    }

    #[test]
    fn compatible_player_keeps_unknown_fields() {
        let player = player();
        assert!(player.check_for_mcpe_0_6_1().unwrap().is_compatible());
        let NbtTag::Compound(root) = &player.nbt else {
            panic!("compound");
        };
        assert_eq!(root.get("FutureField"), Some(&NbtTag::Long(9)));
    }

    #[test]
    fn named_item_and_offhand_are_rejected() {
        let mut player = player();
        let root = player.root_mut().unwrap();
        let Some(NbtTag::List(inventory)) = root.get_mut("Inventory") else {
            panic!("inventory");
        };
        let NbtTag::Compound(item) = &mut inventory[0] else {
            panic!("item");
        };
        item.insert(
            "Name".to_string(),
            NbtTag::String("minecraft:stone".to_string()),
        );
        root.insert("OffHandItem".to_string(), NbtTag::Compound(IndexMap::new()));
        let report = player.check_for_mcpe_0_6_1().unwrap();
        assert!(!report.is_compatible());
        assert!(report.issues.iter().any(|issue| issue.contains("named")));
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.contains("OffHandItem"))
        );
    }

    #[test]
    fn writer_requires_real_061_level_header_and_storage_version() {
        let player = player();
        let mut level = LevelDatDocument::new(
            3,
            NbtTag::Compound(IndexMap::from([(
                "StorageVersion".to_string(),
                NbtTag::Int(3),
            )])),
        );
        write_mcpe_0_6_1_level_dat_player(&mut level, &player).unwrap();
        let NbtTag::Compound(root) = level.root else {
            panic!("compound");
        };
        assert!(matches!(root.get("Player"), Some(NbtTag::Compound(_))));
    }
}
