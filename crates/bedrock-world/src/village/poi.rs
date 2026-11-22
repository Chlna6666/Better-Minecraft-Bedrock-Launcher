//! Village point-of-interest identity shared by Bedrock village records.
//!
//! Bedrock stores interests **per villager**: a `VILLAGE_*_POI` record holds a
//! list keyed by `VillagerID`, and each entry lists the interest instances that
//! villager may claim (`Name`, `Type`, `OwnerCount`, coordinates). Java stores
//! the same concepts **per block position** with a ticket budget, so a converter
//! needs a version-independent identity for the interest itself.
//!
//! This module owns that identity and the Bedrock↔vanilla-name mapping. It does
//! not read or write storage; the village record reader supplies parsed NBT and
//! callers decide how to persist the result.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;

/// Bedrock interest instance kind, from the record's numeric `Type` field.
///
/// The three values are the complete set vanilla writes: `Type` 0 is a bed,
/// 1 a village gathering site, and 2 a workstation. Verified against a real
/// 1.20.12 save where every bed record is `Name=villager Type=0`, every bell
/// `Name=villager Type=1`, and every profession point `Name=<profession>
/// Type=2`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum InterestKind {
    /// A bed (`Type` 0).
    Bed,
    /// A village gathering site such as a bell (`Type` 1).
    MeetingPoint,
    /// A villager workstation (`Type` 2).
    Workstation,
}

impl InterestKind {
    /// Parses the record's numeric `Type` field.
    ///
    /// Returns `None` for values outside the documented set so callers can
    /// preserve the raw instance instead of mislabelling it.
    #[must_use]
    pub const fn from_record_type(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Bed),
            1 => Some(Self::MeetingPoint),
            2 => Some(Self::Workstation),
            _ => None,
        }
    }

    /// The numeric `Type` field written back to a Bedrock record.
    #[must_use]
    pub const fn record_type(self) -> i32 {
        match self {
            Self::Bed => 0,
            Self::MeetingPoint => 1,
            Self::Workstation => 2,
        }
    }
}

/// One interest instance inside a `VILLAGE_*_POI` record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterestInstance {
    /// Instance name as Bedrock stores it (`villager` for beds and bells, the
    /// profession for workstations).
    pub name: String,
    /// Kind derived from the record's `Type` field, when recognised.
    pub kind: Option<InterestKind>,
    /// Block position.
    pub position: [i32; 3],
    /// `OwnerCount`: how many villagers currently hold this instance.
    pub owner_count: i64,
    /// `Capacity`: how many villagers may hold it at once.
    pub capacity: i64,
    /// `Skip` flag marking a reserved but unused slot.
    pub skipped: bool,
}

impl InterestInstance {
    /// True when the instance is a real interest rather than a reserved slot.
    #[must_use]
    pub const fn is_live(&self) -> bool {
        !self.skipped
    }
}

/// One villager's interest list from a `VILLAGE_*_POI` record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VillagerInterests {
    /// `VillagerID` as stored in the record.
    pub villager_id: i64,
    /// Instances in record order, including skipped slots.
    pub instances: Vec<InterestInstance>,
}

impl VillagerInterests {
    /// Live instances only.
    pub fn live(&self) -> impl Iterator<Item = &InterestInstance> {
        self.instances.iter().filter(|instance| instance.is_live())
    }
}

/// Maps a Bedrock interest to its vanilla Java POI type.
///
/// The record `Name` alone is not enough: beds (`Type` 0) and gathering sites
/// (`Type` 1) both carry `Name=villager`, but Java registers them as
/// `minecraft:home` and `minecraft:meeting`. Workstations (`Type` 2) use the
/// profession name in both editions.
///
/// Returns `None` for an unrecognised kind or an unmapped name so a caller can
/// report the interest instead of writing an unknown POI type.
#[must_use]
pub fn java_interest_type(name: &str, kind: InterestKind) -> Option<&'static str> {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    if kind == InterestKind::MeetingPoint {
        // Bells and other gathering sites; `Name` is `villager` in practice.
        return Some("minecraft:meeting");
    }
    if kind == InterestKind::Bed {
        return Some("minecraft:home");
    }
    Some(match name {
        "armorer" => "minecraft:armorer",
        "butcher" => "minecraft:butcher",
        "cartographer" => "minecraft:cartographer",
        "cleric" => "minecraft:cleric",
        "farmer" => "minecraft:farmer",
        "fisherman" => "minecraft:fisherman",
        "fletcher" => "minecraft:fletcher",
        "leatherworker" => "minecraft:leatherworker",
        "librarian" => "minecraft:librarian",
        "mason" => "minecraft:mason",
        "shepherd" => "minecraft:shepherd",
        "toolsmith" => "minecraft:toolsmith",
        "weaponsmith" => "minecraft:weaponsmith",
        _ => return None,
    })
}

/// Parses one `VILLAGE_*_POI` root into per-villager interest lists.
///
/// The record shape is `{ POI: [ { VillagerID: long, instances: [ ... ] } ] }`;
/// an instance is either `{ Skip: 1b }` or a full interest. Unrecognised `Type`
/// values keep `kind: None` so they can be reported rather than silently
/// relabelled.
///
/// # Errors
///
/// Returns a validation error when the root is not the expected compound.
pub fn parse_poi_record(root: &NbtTag) -> Result<Vec<VillagerInterests>> {
    let NbtTag::Compound(root) = root else {
        return Err(BedrockWorldError::Validation(
            "VILLAGE_*_POI record root is not an NBT compound".to_string(),
        ));
    };
    let Some(NbtTag::List(entries)) = root.get("POI") else {
        return Ok(Vec::new());
    };
    let mut villagers = Vec::with_capacity(entries.len());
    for entry in entries {
        let villager_id = integer_field(entry, "VillagerID").unwrap_or(0);
        let mut instances = Vec::new();
        if let Some(NbtTag::List(list)) = field(entry, "instances") {
            for instance in list {
                instances.push(InterestInstance {
                    name: text_field(instance, "Name").unwrap_or_default().to_string(),
                    kind: integer_field(instance, "Type")
                        .and_then(|value| i32::try_from(value).ok())
                        .and_then(InterestKind::from_record_type),
                    position: [
                        integer_field(instance, "X").unwrap_or(0) as i32,
                        integer_field(instance, "Y").unwrap_or(0) as i32,
                        integer_field(instance, "Z").unwrap_or(0) as i32,
                    ],
                    owner_count: integer_field(instance, "OwnerCount").unwrap_or(0),
                    capacity: integer_field(instance, "Capacity").unwrap_or(0),
                    skipped: matches!(field(instance, "Skip"), Some(NbtTag::Byte(1))),
                });
            }
        }
        villagers.push(VillagerInterests {
            villager_id,
            instances,
        });
    }
    Ok(villagers)
}

fn field<'a>(tag: &'a NbtTag, key: &str) -> Option<&'a NbtTag> {
    match tag {
        NbtTag::Compound(map) => map.get(key),
        _ => None,
    }
}

fn integer_field(tag: &NbtTag, key: &str) -> Option<i64> {
    match field(tag, key)? {
        NbtTag::Byte(value) => Some(i64::from(*value)),
        NbtTag::Short(value) => Some(i64::from(*value)),
        NbtTag::Int(value) => Some(i64::from(*value)),
        NbtTag::Long(value) => Some(*value),
        _ => None,
    }
}

fn text_field<'a>(tag: &'a NbtTag, key: &str) -> Option<&'a str> {
    match field(tag, key)? {
        NbtTag::String(value) => Some(value.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::NbtTag;
    use indexmap::IndexMap;

    fn compound(fields: impl IntoIterator<Item = (&'static str, NbtTag)>) -> NbtTag {
        NbtTag::Compound(
            fields
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect::<IndexMap<_, _>>(),
        )
    }

    fn list(items: Vec<NbtTag>) -> NbtTag {
        NbtTag::List(items)
    }

    fn instance(name: &str, kind: i32, x: i32, y: i32, z: i32) -> NbtTag {
        compound([
            ("Name", NbtTag::String(name.to_string())),
            ("Type", NbtTag::Int(kind)),
            ("X", NbtTag::Int(x)),
            ("Y", NbtTag::Int(y)),
            ("Z", NbtTag::Int(z)),
            ("OwnerCount", NbtTag::Long(1)),
            ("Capacity", NbtTag::Long(1)),
        ])
    }

    fn parse(root: &NbtTag) -> Vec<VillagerInterests> {
        parse_poi_record(root).expect("parse")
    }

    #[test]
    fn parses_per_villager_interest_lists() {
        let root = compound([(
            "POI",
            list(vec![
                compound([
                    ("VillagerID", NbtTag::Long(-760_208_920_752)),
                    (
                        "instances",
                        list(vec![
                            instance("villager", 0, 1919, 116, 1737),
                            compound([("Skip", NbtTag::Byte(1))]),
                            instance("farmer", 2, 1921, 116, 1740),
                        ]),
                    ),
                ]),
                compound([
                    ("VillagerID", NbtTag::Long(7)),
                    ("instances", list(Vec::new())),
                ]),
            ]),
        )]);
        let villagers = parse(&root);
        assert_eq!(villagers.len(), 2);
        assert_eq!(villagers[0].villager_id, -760_208_920_752);
        assert_eq!(villagers[0].instances.len(), 3);
        assert_eq!(villagers[0].live().count(), 2);
        assert_eq!(villagers[0].instances[0].kind, Some(InterestKind::Bed));
        assert_eq!(villagers[0].instances[0].position, [1919, 116, 1737]);
        assert!(villagers[0].instances[1].skipped);
        assert_eq!(
            villagers[0].instances[2].kind,
            Some(InterestKind::Workstation)
        );
        assert!(villagers[1].instances.is_empty());
    }

    #[test]
    fn unknown_record_type_is_preserved_as_none() {
        let root = compound([(
            "POI",
            list(vec![compound([
                ("VillagerID", NbtTag::Long(1)),
                ("instances", list(vec![instance("mystery", 9, 0, 0, 0)])),
            ])]),
        )]);
        let villagers = parse(&root);
        assert_eq!(villagers[0].instances[0].kind, None);
        assert_eq!(villagers[0].instances[0].name, "mystery");
    }

    #[test]
    fn interest_kind_round_trips_record_types() {
        for kind in [
            InterestKind::Bed,
            InterestKind::MeetingPoint,
            InterestKind::Workstation,
        ] {
            assert_eq!(InterestKind::from_record_type(kind.record_type()), Some(kind));
        }
        assert_eq!(InterestKind::from_record_type(3), None);
    }

    #[test]
    fn beds_and_gathering_sites_map_by_record_type_not_name() {
        // Real 1.20.12 saves store both as `Name=villager`; only `Type`
        // separates them, so one name must map to different Java types.
        assert_eq!(
            java_interest_type("villager", InterestKind::Bed),
            Some("minecraft:home")
        );
        assert_eq!(
            java_interest_type("villager", InterestKind::MeetingPoint),
            Some("minecraft:meeting")
        );
    }

    #[test]
    fn workstations_map_by_profession_name() {
        assert_eq!(
            java_interest_type("farmer", InterestKind::Workstation),
            Some("minecraft:farmer")
        );
        assert_eq!(
            java_interest_type("minecraft:weaponsmith", InterestKind::Workstation),
            Some("minecraft:weaponsmith")
        );
    }

    #[test]
    fn unmapped_workstation_name_is_reported_rather_than_guessed() {
        assert_eq!(
            java_interest_type("modded_profession", InterestKind::Workstation),
            None
        );
    }

    #[test]
    fn missing_poi_field_yields_no_villagers() {
        let root = compound([("Other", NbtTag::Int(1))]);
        assert!(parse(&root).is_empty());
    }

    #[test]
    fn non_compound_root_is_rejected() {
        let error = parse_poi_record(&NbtTag::Int(0)).expect_err("must reject");
        assert!(error.to_string().contains("not an NBT compound"));
    }
}
