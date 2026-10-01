//! Bedrock `map_<id>` saved data.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use bytes::Bytes;
use indexmap::IndexMap;
use std::collections::BTreeMap;

mod bundle;
mod id;
pub use bundle::MapBundle;
pub use id::MapItemId;

/// Resolves a Bedrock `minecraft:filled_map` item's `tag.map_uuid` to its `map_<id>` key.
///
/// This only reads the item NBT; it does not access or modify LevelDB or a player's inventory.
/// Other items and maps without a Long `map_uuid` return `None`. The map record may still be
/// absent, so callers must look it up separately.
#[must_use]
pub fn filled_map_id(item: &NbtTag) -> Option<MapItemId> {
    let NbtTag::Compound(root) = item else {
        return None;
    };
    if !matches!(root.get("Name"), Some(NbtTag::String(name)) if name == "minecraft:filled_map") {
        return None;
    }
    let Some(NbtTag::Compound(tag)) = root.get("tag") else {
        return None;
    };
    let Some(NbtTag::Long(uuid)) = tag.get("map_uuid") else {
        return None;
    };
    MapItemId::new(uuid.to_string()).ok()
}

/// Builds a single Bedrock `minecraft:filled_map` inventory item for a stored map id.
///
/// The item fields and NBT types match filled maps observed in a Bedrock 1.26 player inventory:
/// `Name`, `Count`, `Damage`, `Slot`, `WasPickedUp`, `tag.map_name_index` and `tag.map_uuid`.
/// The item contains no
/// embedded pixels; its `map_uuid` must resolve to a separately staged `map_<id>` record.
/// This only constructs NBT, with no player or world write and no format conversion. The caller
/// must choose a valid free slot and stage the edited LevelDB player through a transaction; a
/// `level.dat.Player` update cannot share that transaction's atomic LevelDB batch.
///
/// # Errors
///
/// Returns a validation error for a negative map id or slot number.
pub fn new_filled_map_item(id: i64, slot: i8) -> Result<NbtTag> {
    if id < 0 || slot < 0 {
        return Err(BedrockWorldError::Validation(
            "filled map id and slot must be nonnegative".to_string(),
        ));
    }
    Ok(NbtTag::Compound(IndexMap::from([
        ("Count".to_string(), NbtTag::Byte(1)),
        ("Damage".to_string(), NbtTag::Short(0)),
        (
            "Name".to_string(),
            NbtTag::String("minecraft:filled_map".to_string()),
        ),
        ("Slot".to_string(), NbtTag::Byte(slot)),
        ("WasPickedUp".to_string(), NbtTag::Byte(0)),
        (
            "tag".to_string(),
            NbtTag::Compound(IndexMap::from([
                ("map_name_index".to_string(), NbtTag::Int(0)),
                ("map_uuid".to_string(), NbtTag::Long(id)),
            ])),
        ),
    ])))
}

/// Rewrites `tag.map_uuid` on filled maps inside an item or container NBT tree.
///
/// The caller supplies old and newly allocated numeric Bedrock map ids. Only ids present in
/// `ids` are changed; unrelated maps and all unknown NBT fields remain intact. This only edits
/// the supplied in-memory NBT and does not write LevelDB or `level.dat`. Callers must stage the
/// matching `map_<id>` records and the edited container/player record in the appropriate world
/// transaction; `level.dat.Player` cannot share a LevelDB atomic batch.
///
/// Returns the number of filled-map references changed. No format conversion occurs.
pub fn remap_filled_map_ids(item: &mut NbtTag, ids: &BTreeMap<i64, i64>) -> usize {
    match item {
        NbtTag::Compound(fields) => {
            let changed = if matches!(fields.get("Name"), Some(NbtTag::String(name)) if name == "minecraft:filled_map")
            {
                fields
                    .get_mut("tag")
                    .and_then(|tag| match tag {
                        NbtTag::Compound(fields) => fields.get_mut("map_uuid"),
                        _ => None,
                    })
                    .and_then(|uuid| match uuid {
                        NbtTag::Long(uuid) => ids.get(uuid).map(|new_id| {
                            *uuid = *new_id;
                            1
                        }),
                        _ => None,
                    })
                    .unwrap_or(0)
            } else {
                0
            };
            changed
                + fields
                    .values_mut()
                    .map(|value| remap_filled_map_ids(value, ids))
                    .sum::<usize>()
        }
        NbtTag::List(values) => values
            .iter_mut()
            .map(|value| remap_filled_map_ids(value, ids))
            .sum(),
        _ => 0,
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Bedrock `map_<id>` value with decoded NBT roots and optional RGBA pixels.
///
/// `roots` are authoritative for writes; `known_fields` and `pixels` are read projections.
/// Keeping unknown tags in `roots` preserves them when an edited record is serialized.
pub struct SavedData {
    /// Validated storage id without the `map_` prefix.
    pub id: MapItemId,
    /// Consecutive NBT roots stored in the map value.
    pub roots: Vec<NbtTag>,
    /// Common map fields extracted from NBT when present.
    pub known_fields: KnownFields,
    /// Decoded RGBA map pixels when width, height, and color bytes are present.
    pub pixels: Option<Pixels>,
    /// Source LevelDB bytes used to reject stale writes; empty for a newly created record.
    pub raw: Bytes,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Common map NBT fields recognized by this crate.
pub struct KnownFields {
    /// Dimension id containing the map center.
    pub dimension: Option<i32>,
    /// World X coordinate of the map center.
    pub center_x: Option<i32>,
    /// World Z coordinate of the map center.
    pub center_z: Option<i32>,
    /// Bedrock map scale.
    pub scale: Option<i32>,
    /// Pixel width recorded in NBT.
    pub width: Option<i32>,
    /// Pixel height recorded in NBT.
    pub height: Option<i32>,
    /// Lock state when recorded by the map NBT.
    pub locked: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Bedrock `colors` array decoded as row-major RGBA pixels.
pub struct Pixels {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Four RGBA bytes per pixel in row-major order, as stored in the Bedrock `colors` tag.
    pub rgba: Vec<u8>,
}

impl SavedData {
    /// Copies a map record for import under a newly allocated numeric id.
    ///
    /// The observed Bedrock `mapId` Long is rewritten to match the new `map_<id>` key. A
    /// `parentMapId` pointing to another imported map is remapped through `ids`; an external
    /// parent becomes `-1` so it cannot accidentally refer to an unrelated target-world map.
    /// Unknown NBT fields and RGBA bytes are preserved. The returned record has empty `raw`,
    /// requiring its key to be absent when a [`crate::world::WorldTransaction`] commits it.
    /// This operation reads and writes no world storage and performs no version conversion.
    /// Its companion [`remap_filled_map_ids`] updates the contained item references separately.
    ///
    /// # Errors
    ///
    /// Returns validation errors for missing or mismatched numeric source `mapId`, a missing
    /// destination mapping, a negative destination id, or malformed root NBT.
    pub fn rekey_for_import(&self, ids: &BTreeMap<i64, i64>) -> Result<Self> {
        let source_id = self.id.as_str().parse::<i64>().map_err(|_| {
            BedrockWorldError::Validation("source map id is not numeric".to_string())
        })?;
        let destination_id = *ids.get(&source_id).ok_or_else(|| {
            BedrockWorldError::Validation("map import id mapping is incomplete".to_string())
        })?;
        if destination_id < 0 {
            return Err(BedrockWorldError::Validation(
                "destination map id must be nonnegative".to_string(),
            ));
        }
        let mut imported = self.clone();
        let Some(NbtTag::Compound(fields)) = imported.roots.first_mut() else {
            return Err(BedrockWorldError::Validation(
                "map record has no compound NBT root".to_string(),
            ));
        };
        if fields.get("mapId") != Some(&NbtTag::Long(source_id)) {
            return Err(BedrockWorldError::Validation(
                "mapId does not match source map key".to_string(),
            ));
        }
        let Some(NbtTag::Long(parent_id)) = fields.get_mut("parentMapId") else {
            return Err(BedrockWorldError::Validation(
                "map record has no parentMapId Long".to_string(),
            ));
        };
        if *parent_id != -1 {
            *parent_id = ids.get(parent_id).copied().unwrap_or(-1);
        }
        fields.insert("mapId".to_string(), NbtTag::Long(destination_id));
        imported.id = MapItemId::new(destination_id.to_string())?;
        imported.raw = Bytes::new();
        Ok(imported)
    }

    /// Creates a standalone locked Bedrock `map_<id>` record with direct RGBA pixels.
    ///
    /// The field names and numeric NBT types follow the 128×128 records observed in a Bedrock
    /// 1.26 world: `mapId` equals the numeric key suffix, `parentMapId` is `-1`, and `colors`
    /// contains row-major RGBA bytes. `mapLocked` is set so the game should not redraw the supplied
    /// pixels from terrain. This creates only in-memory NBT; no `filled_map` item, player record,
    /// `level.dat`, or LevelDB data is changed. `raw` is empty, so a later world transaction
    /// requires the new key to be absent at commit. This does not migrate storage versions.
    ///
    /// # Errors
    ///
    /// Returns validation errors for a negative id, scale outside `0..=4`, or pixels that are
    /// not exactly 128×128 RGBA.
    pub fn new_locked_pixels(
        id: i64,
        pixels: Pixels,
        dimension: i8,
        center_x: i32,
        center_z: i32,
        scale: i8,
    ) -> Result<Self> {
        if id < 0 || !(0..=4).contains(&scale) {
            return Err(BedrockWorldError::Validation(
                "map id must be nonnegative and scale must be 0..=4".to_string(),
            ));
        }
        validate_pixels(&pixels)?;
        let map_id = id;
        let id = MapItemId::new(id.to_string())?;
        let colors = pixels.rgba.iter().map(|byte| *byte as i8).collect();
        let roots = vec![NbtTag::Compound(IndexMap::from([
            ("colors".to_string(), NbtTag::ByteArray(colors)),
            ("decorations".to_string(), NbtTag::List(Vec::new())),
            ("dimension".to_string(), NbtTag::Byte(dimension)),
            ("fullyExplored".to_string(), NbtTag::Byte(1)),
            ("height".to_string(), NbtTag::Short(128)),
            ("mapId".to_string(), NbtTag::Long(map_id)),
            ("mapLocked".to_string(), NbtTag::Byte(1)),
            ("parentMapId".to_string(), NbtTag::Long(-1)),
            ("scale".to_string(), NbtTag::Byte(scale)),
            ("unlimitedTracking".to_string(), NbtTag::Byte(0)),
            ("width".to_string(), NbtTag::Short(128)),
            ("xCenter".to_string(), NbtTag::Int(center_x)),
            ("zCenter".to_string(), NbtTag::Int(center_z)),
        ]))];
        Ok(Self {
            id,
            roots,
            known_fields: KnownFields {
                dimension: Some(i32::from(dimension)),
                center_x: Some(center_x),
                center_z: Some(center_z),
                scale: Some(i32::from(scale)),
                width: Some(128),
                height: Some(128),
                locked: Some(true),
            },
            pixels: Some(pixels),
            raw: Bytes::new(),
        })
    }

    /// Replaces the Bedrock `colors` array in an existing map record with 128×128 RGBA pixels.
    ///
    /// The first NBT root must contain the map's `colors` byte array. All other fields and roots,
    /// including unknown future tags, are retained. `raw` remains the original source snapshot,
    /// so [`crate::world::WorldTransaction::save_map_item`] can reject a stale update at commit.
    /// This method performs no LevelDB write or map format conversion.
    ///
    /// # Errors
    ///
    /// Returns validation errors for dimensions, RGBA length, or a record without a `colors`
    /// byte array in its first compound root.
    pub fn replace_pixels(&mut self, pixels: Pixels) -> Result<()> {
        validate_pixels(&pixels)?;
        let Some(NbtTag::Compound(fields)) = self.roots.first_mut() else {
            return Err(BedrockWorldError::Validation(
                "map record has no compound NBT root".to_string(),
            ));
        };
        let Some(NbtTag::ByteArray(colors)) = fields.get_mut("colors") else {
            return Err(BedrockWorldError::Validation(
                "map record has no colors byte array".to_string(),
            ));
        };
        *colors = pixels.rgba.iter().map(|byte| *byte as i8).collect();
        self.pixels = Some(pixels);
        Ok(())
    }
}

fn validate_pixels(pixels: &Pixels) -> Result<()> {
    if pixels.width != 128 || pixels.height != 128 || pixels.rgba.len() != 128 * 128 * 4 {
        return Err(BedrockWorldError::Validation(
            "Bedrock map pixels must be 128x128 RGBA".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn filled_map_id_reads_nested_bedrock_uuid_only() {
        let mut item = NbtTag::Compound(IndexMap::from([
            (
                "Name".to_string(),
                NbtTag::String("minecraft:filled_map".to_string()),
            ),
            (
                "tag".to_string(),
                NbtTag::Compound(IndexMap::from([("map_uuid".to_string(), NbtTag::Long(42))])),
            ),
        ]));
        assert_eq!(
            filled_map_id(&item).as_ref().map(MapItemId::as_str),
            Some("42")
        );
        if let NbtTag::Compound(root) = &mut item {
            root.insert(
                "Name".to_string(),
                NbtTag::String("minecraft:stone".to_string()),
            );
        }
        assert_eq!(filled_map_id(&item), None);
    }

    #[test]
    fn remaps_maps_in_nested_container_without_touching_other_tags() {
        let map = || {
            NbtTag::Compound(IndexMap::from([
                (
                    "Name".to_string(),
                    NbtTag::String("minecraft:filled_map".to_string()),
                ),
                (
                    "tag".to_string(),
                    NbtTag::Compound(IndexMap::from([
                        ("map_uuid".to_string(), NbtTag::Long(42)),
                        ("future".to_string(), NbtTag::String("keep".to_string())),
                    ])),
                ),
            ]))
        };
        let mut chest = NbtTag::Compound(IndexMap::from([(
            "Items".to_string(),
            NbtTag::List(vec![NbtTag::Compound(IndexMap::from([(
                "tag".to_string(),
                NbtTag::Compound(IndexMap::from([(
                    "Items".to_string(),
                    NbtTag::List(vec![map()]),
                )])),
            )]))]),
        )]));
        let original = chest.clone();
        assert_eq!(
            remap_filled_map_ids(&mut chest, &BTreeMap::from([(42, 91)])),
            1
        );
        assert_ne!(chest, original);
        assert_eq!(
            remap_filled_map_ids(&mut chest, &BTreeMap::from([(42, 99)])),
            0
        );
        let NbtTag::Compound(root) = chest else {
            panic!("chest compound")
        };
        let NbtTag::List(items) = &root["Items"] else {
            panic!("chest items")
        };
        let NbtTag::Compound(shulker) = &items[0] else {
            panic!("shulker")
        };
        let NbtTag::Compound(tag) = &shulker["tag"] else {
            panic!("shulker tag")
        };
        let NbtTag::List(maps) = &tag["Items"] else {
            panic!("maps")
        };
        assert_eq!(
            filled_map_id(&maps[0]).as_ref().map(MapItemId::as_str),
            Some("91")
        );
        let NbtTag::Compound(map) = &maps[0] else {
            panic!("map")
        };
        let NbtTag::Compound(fields) = &map["tag"] else {
            panic!("map tag")
        };
        assert_eq!(fields["future"], NbtTag::String("keep".to_string()));
    }

    #[test]
    fn replacing_pixels_retains_unknown_fields_and_source_snapshot() {
        let original = Bytes::from_static(b"original source bytes");
        let mut record = SavedData {
            id: MapItemId::new("42").expect("id"),
            roots: vec![NbtTag::Compound(IndexMap::from([
                (
                    "colors".to_string(),
                    NbtTag::ByteArray(vec![0; 128 * 128 * 4]),
                ),
                ("future".to_string(), NbtTag::String("keep".to_string())),
            ]))],
            known_fields: KnownFields::default(),
            pixels: None,
            raw: original.clone(),
        };
        let pixels = Pixels {
            width: 128,
            height: 128,
            rgba: vec![255; 128 * 128 * 4],
        };
        record.replace_pixels(pixels.clone()).expect("replace");
        assert_eq!(record.pixels, Some(pixels));
        assert_eq!(record.raw, original);
        let NbtTag::Compound(fields) = &record.roots[0] else {
            panic!("root")
        };
        assert_eq!(fields["future"], NbtTag::String("keep".to_string()));
        assert!(matches!(&fields["colors"], NbtTag::ByteArray(bytes) if bytes[0] == -1));
    }

    #[test]
    fn new_locked_map_roundtrips_with_numeric_key_and_independent_parent() {
        let pixels = Pixels {
            width: 128,
            height: 128,
            rgba: vec![17; 128 * 128 * 4],
        };
        let record =
            SavedData::new_locked_pixels(92, pixels.clone(), 0, 128, -256, 2).expect("new map");
        assert!(record.raw.is_empty());
        let bytes = crate::scan::encode_map_item(&record).expect("encode");
        let decoded = crate::scan::decode_map_item(record.id.clone(), bytes).expect("decode");
        assert_eq!(decoded.pixels, Some(pixels));
        assert_eq!(decoded.known_fields.center_z, Some(-256));
        assert_eq!(decoded.known_fields.locked, Some(true));
        let NbtTag::Compound(fields) = &decoded.roots[0] else {
            panic!("root")
        };
        assert_eq!(fields["mapId"], NbtTag::Long(92));
        assert_eq!(fields["parentMapId"], NbtTag::Long(-1));
    }

    #[test]
    fn constructed_filled_map_resolves_to_matching_map_record() {
        let item = new_filled_map_item(92, 14).expect("map item");
        let NbtTag::Compound(root) = &item else {
            panic!("item")
        };
        let NbtTag::Compound(tag) = &root["tag"] else {
            panic!("tag")
        };
        assert_eq!(tag["map_name_index"], NbtTag::Int(0));
        assert_eq!(tag["map_uuid"], NbtTag::Long(92));
        assert_eq!(
            filled_map_id(&item).as_ref().map(MapItemId::as_str),
            Some("92")
        );
        assert!(new_filled_map_item(92, -1).is_err());
    }

    #[test]
    fn imported_map_remaps_parent_and_preserves_future_fields() {
        let pixels = Pixels {
            width: 128,
            height: 128,
            rgba: vec![31; 128 * 128 * 4],
        };
        let mut source =
            SavedData::new_locked_pixels(3, pixels.clone(), 0, 0, 0, 0).expect("source");
        let NbtTag::Compound(fields) = &mut source.roots[0] else {
            panic!("root")
        };
        fields.insert("parentMapId".to_string(), NbtTag::Long(2));
        fields.insert("future".to_string(), NbtTag::String("keep".to_string()));
        source.raw = Bytes::from_static(b"source snapshot");
        let imported = source
            .rekey_for_import(&BTreeMap::from([(2, 90), (3, 91)]))
            .expect("rekey");
        assert_eq!(imported.id.as_str(), "91");
        assert!(imported.raw.is_empty());
        assert_eq!(imported.pixels, Some(pixels));
        let NbtTag::Compound(fields) = &imported.roots[0] else {
            panic!("root")
        };
        assert_eq!(fields["mapId"], NbtTag::Long(91));
        assert_eq!(fields["parentMapId"], NbtTag::Long(90));
        assert_eq!(fields["future"], NbtTag::String("keep".to_string()));
    }
}
