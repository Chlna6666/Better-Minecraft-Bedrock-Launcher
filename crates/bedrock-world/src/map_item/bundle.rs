//! Portable BMCBL NBT bundle for map records and their container NBT.

use super::{MapItemId, SavedData, filled_map_id, remap_filled_map_ids};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, parse_root_nbt, serialize_root_nbt};
use crate::scan::{decode_map_item, encode_map_item};
use bytes::Bytes;
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet};

const FORMAT: &str = "BMCBLBedrockMapBundle";
const VERSION: i32 = 1;
const MAX_MAPS: usize = 1024;
const MAX_BYTES: usize = 256 * 1024 * 1024;

/// Self-contained Bedrock map records and one item/container NBT tree.
///
/// A `filled_map` item stores a `map_uuid` reference, not the `map_<id>` RGBA pixels.
/// This BMCBL-specific NBT envelope includes both so it can be imported into another world.
/// The envelope is not a vanilla `mcstructure` or directly placeable chest. Parsing, serializing
/// and remapping are in-memory operations; none modify LevelDB, a player or `level.dat`.
#[derive(Debug, Clone)]
pub struct MapBundle {
    records: Vec<SavedData>,
    container: NbtTag,
    grid: Option<(u32, u32)>,
}

impl MapBundle {
    /// Returns the validated map records without allowing mutation of their ids or NBT roots.
    #[must_use]
    pub fn records(&self) -> &[SavedData] {
        &self.records
    }

    /// Returns the item or container NBT tree holding the bundled filled-map references.
    #[must_use]
    pub fn container(&self) -> &NbtTag {
        &self.container
    }

    /// Returns the numbered maps in this BMCBL export's flat `Items` list.
    ///
    /// This does not treat the envelope as a placeable Minecraft container. Each included map
    /// record must have exactly one `filled_map` reference in the list; nested container layouts
    /// are rejected here because they require a separate, verified game-item import path.
    /// No world data is read or written.
    ///
    /// # Errors
    ///
    /// Returns a validation error for a missing list, non-map entries, duplicate references or
    /// records without a matching item.
    pub fn flat_items(&self) -> Result<&[NbtTag]> {
        let NbtTag::Compound(container) = &self.container else {
            return Err(BedrockWorldError::Validation(
                "map bundle container must be a compound".to_string(),
            ));
        };
        let Some(NbtTag::List(items)) = container.get("Items") else {
            return Err(BedrockWorldError::Validation(
                "map bundle has no flat Items list".to_string(),
            ));
        };
        if items.len() != self.records.len() {
            return Err(BedrockWorldError::Validation(
                "map bundle item and record counts differ".to_string(),
            ));
        }
        let mut ids = BTreeSet::new();
        for item in items {
            let id = filled_map_id(item).ok_or_else(|| {
                BedrockWorldError::Validation("map bundle Items contains a non-map".to_string())
            })?;
            if !ids.insert(id.as_str().to_owned()) {
                return Err(BedrockWorldError::Validation(
                    "map bundle Items contains a duplicate map".to_string(),
                ));
            }
        }
        Ok(items)
    }
    /// Validates a map bundle before it is exported or imported.
    ///
    /// All filled-map references must resolve to one of the included records. Unrelated NBT
    /// fields remain in the container and records. No world storage is read or written.
    ///
    /// # Errors
    ///
    /// Returns validation errors for an empty/oversized set, duplicate or inconsistent map ids,
    /// malformed map record roots, or unresolved filled-map references.
    pub fn new(records: Vec<SavedData>, container: NbtTag) -> Result<Self> {
        if records.is_empty() || records.len() > MAX_MAPS {
            return Err(BedrockWorldError::Validation(format!(
                "map bundle must contain 1 to {MAX_MAPS} records"
            )));
        }
        if !matches!(container, NbtTag::Compound(_)) {
            return Err(BedrockWorldError::Validation(
                "map bundle container must be a compound".to_string(),
            ));
        }
        let mut ids = BTreeSet::new();
        for record in &records {
            let id = record.id.as_str().parse::<i64>().map_err(|_| {
                BedrockWorldError::Validation("map bundle id is not numeric".to_string())
            })?;
            if !ids.insert(id) {
                return Err(BedrockWorldError::Validation(
                    "map bundle contains a duplicate id".to_string(),
                ));
            }
            let Some(NbtTag::Compound(fields)) = record.roots.first() else {
                return Err(BedrockWorldError::Validation(
                    "map bundle record has no compound root".to_string(),
                ));
            };
            if fields.get("mapId") != Some(&NbtTag::Long(id)) {
                return Err(BedrockWorldError::Validation(
                    "map bundle mapId differs from its key".to_string(),
                ));
            }
        }
        validate_refs(&container, &ids)?;
        let grid = infer_grid(&container, records.len());
        Ok(Self {
            records,
            container,
            grid,
        })
    }

    /// Associates a north-up tile grid with this flat row-major map list.
    ///
    /// The dimensions are stored in the BMCBL envelope and survive import-id remapping. This
    /// changes only the in-memory bundle; it does not write a world, player, or file.
    ///
    /// # Errors
    ///
    /// Returns a validation error when either dimension is zero, exceeds 64, or their product
    /// differs from the number of map records.
    pub fn with_grid(mut self, columns: u32, rows: u32) -> Result<Self> {
        if !(1..=64).contains(&columns)
            || !(1..=64).contains(&rows)
            || columns.checked_mul(rows) != Some(self.records.len() as u32)
        {
            return Err(BedrockWorldError::Validation(
                "map grid dimensions must match the bundle's 1 to 1024 records".to_string(),
            ));
        }
        self.grid = Some((columns, rows));
        Ok(self)
    }

    /// Returns the north-up row-major tile grid when the bundle carries verified layout data.
    ///
    /// Older BMCBL exports infer this from their per-map coordinate Lore. Arbitrary flat map
    /// bundles without layout metadata return `None`; callers must not assume a square layout.
    #[must_use]
    pub const fn grid(&self) -> Option<(u32, u32)> {
        self.grid
    }

    /// Serializes this BMCBL-specific envelope as one little-endian Bedrock NBT root.
    ///
    /// Each included `map_<id>` record is stored as an encoded byte array, preserving unknown
    /// tags and all source roots. This does not export a vanilla structure or write a file.
    ///
    /// # Errors
    ///
    /// Returns NBT serialization errors or a validation error if the encoded bundle exceeds
    /// 256 MiB.
    pub fn to_nbt_bytes(&self) -> Result<Vec<u8>> {
        let mut maps = Vec::with_capacity(self.records.len());
        for record in &self.records {
            let id = record.id.as_str().parse::<i64>().map_err(|_| {
                BedrockWorldError::Validation("map bundle id is not numeric".to_string())
            })?;
            let data = encode_map_item(record)?;
            maps.push(NbtTag::Compound(IndexMap::from([
                ("id".to_string(), NbtTag::Long(id)),
                (
                    "data".to_string(),
                    NbtTag::ByteArray(data.iter().map(|byte| *byte as i8).collect()),
                ),
            ])));
        }
        let mut root = IndexMap::from([
            ("format".to_string(), NbtTag::String(FORMAT.to_string())),
            ("version".to_string(), NbtTag::Int(VERSION)),
            ("maps".to_string(), NbtTag::List(maps)),
            ("container".to_string(), self.container.clone()),
        ]);
        if let Some((columns, rows)) = self.grid {
            root.insert("grid_columns".to_string(), NbtTag::Int(columns as i32));
            root.insert("grid_rows".to_string(), NbtTag::Int(rows as i32));
        }
        let root = NbtTag::Compound(root);
        let bytes = serialize_root_nbt(&root)?;
        if bytes.len() > MAX_BYTES {
            return Err(BedrockWorldError::Validation(
                "map bundle exceeds 256 MiB".to_string(),
            ));
        }
        Ok(bytes)
    }

    /// Decodes a BMCBL map bundle from one Bedrock NBT root.
    ///
    /// The input is an application envelope rather than a vanilla Minecraft chest record.
    /// Unknown map-record fields are preserved; unknown envelope fields are ignored. No stored
    /// world record is read or written.
    ///
    /// # Errors
    ///
    /// Returns NBT or validation errors for oversized bytes, wrong format/version, malformed
    /// records or unresolved map-item references.
    pub fn from_nbt_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(BedrockWorldError::Validation(
                "map bundle exceeds 256 MiB".to_string(),
            ));
        }
        let NbtTag::Compound(mut root) = parse_root_nbt(bytes)? else {
            return Err(BedrockWorldError::Validation(
                "map bundle has no compound root".to_string(),
            ));
        };
        if root.get("format") != Some(&NbtTag::String(FORMAT.to_string()))
            || root.get("version") != Some(&NbtTag::Int(VERSION))
        {
            return Err(BedrockWorldError::Validation(
                "unsupported map bundle format or version".to_string(),
            ));
        }
        let Some(NbtTag::List(maps)) = root.swap_remove("maps") else {
            return Err(BedrockWorldError::Validation(
                "map bundle has no map list".to_string(),
            ));
        };
        if maps.len() > MAX_MAPS {
            return Err(BedrockWorldError::Validation(
                "map bundle contains too many records".to_string(),
            ));
        }
        let mut records = Vec::with_capacity(maps.len());
        for map in maps {
            let NbtTag::Compound(mut fields) = map else {
                return Err(BedrockWorldError::Validation(
                    "map bundle entry is not a compound".to_string(),
                ));
            };
            let Some(NbtTag::Long(id)) = fields.swap_remove("id") else {
                return Err(BedrockWorldError::Validation(
                    "map bundle entry has no numeric id".to_string(),
                ));
            };
            let Some(NbtTag::ByteArray(data)) = fields.swap_remove("data") else {
                return Err(BedrockWorldError::Validation(
                    "map bundle entry has no NBT bytes".to_string(),
                ));
            };
            let bytes = Bytes::from(data.into_iter().map(|byte| byte as u8).collect::<Vec<_>>());
            records.push(decode_map_item(MapItemId::new(id.to_string())?, bytes)?);
        }
        let grid = match (
            root.swap_remove("grid_columns"),
            root.swap_remove("grid_rows"),
        ) {
            (None, None) => None,
            (Some(NbtTag::Int(columns)), Some(NbtTag::Int(rows))) if columns > 0 && rows > 0 => {
                Some((columns as u32, rows as u32))
            }
            _ => {
                return Err(BedrockWorldError::Validation(
                    "map bundle has invalid grid dimensions".to_string(),
                ));
            }
        };
        let container = root.swap_remove("container").ok_or_else(|| {
            BedrockWorldError::Validation("map bundle has no container".to_string())
        })?;
        let bundle = Self::new(records, container)?;
        if let Some((columns, rows)) = grid {
            bundle.with_grid(columns, rows)
        } else {
            Ok(bundle)
        }
    }

    /// Rewrites all bundled records and filled-map references to fresh target-world ids.
    ///
    /// `ids` must map every source record id to a distinct nonnegative id already chosen for
    /// the target world. Every returned record has empty `raw`, so staging it in a world
    /// transaction requires its `map_<id>` key to be absent at commit. This function itself
    /// reads and writes no world data and cannot reserve ids against other processes.
    ///
    /// # Errors
    ///
    /// Returns validation errors for incomplete, duplicate or negative destination ids, or
    /// malformed source records.
    pub fn remap(&self, ids: &BTreeMap<i64, i64>) -> Result<Self> {
        if ids.len() != self.records.len()
            || ids.values().any(|id| *id < 0)
            || ids.values().copied().collect::<BTreeSet<_>>().len() != ids.len()
        {
            return Err(BedrockWorldError::Validation(
                "map import ids must be complete, unique and nonnegative".to_string(),
            ));
        }
        let records = self
            .records
            .iter()
            .map(|record| record.rekey_for_import(ids))
            .collect::<Result<Vec<_>>>()?;
        let mut container = self.container.clone();
        remap_filled_map_ids(&mut container, ids);
        let imported = Self::new(records, container)?;
        if let Some((columns, rows)) = self.grid {
            imported.with_grid(columns, rows)
        } else {
            Ok(imported)
        }
    }
}

fn infer_grid(container: &NbtTag, map_count: usize) -> Option<(u32, u32)> {
    let NbtTag::Compound(container) = container else {
        return None;
    };
    let NbtTag::List(items) = container.get("Items")? else {
        return None;
    };
    if items.len() != map_count {
        return None;
    }

    let mut dimensions = None;
    let mut coordinates = BTreeSet::new();
    for item in items {
        let NbtTag::Compound(item) = item else {
            return None;
        };
        let NbtTag::Compound(tag) = item.get("tag")? else {
            return None;
        };
        let NbtTag::Compound(display) = tag.get("display")? else {
            return None;
        };
        let NbtTag::List(lore) = display.get("Lore")? else {
            return None;
        };
        let line = lore.iter().find_map(|line| match line {
            NbtTag::String(line) if line.starts_with("BMCBL · 北向上 · 列 ") => Some(line),
            _ => None,
        })?;
        let mut parts = line.split('·').map(str::trim);
        parts.next()?;
        parts.next()?;
        let (column, columns) = parse_grid_axis(parts.next()?, "列")?;
        let (row, rows) = parse_grid_axis(parts.next()?, "行")?;
        if parts.next().is_some() || !(1..=64).contains(&columns) || !(1..=64).contains(&rows) {
            return None;
        }
        if dimensions.is_some_and(|expected| expected != (columns, rows))
            || !coordinates.insert((column, row))
        {
            return None;
        }
        dimensions = Some((columns, rows));
    }
    let (columns, rows) = dimensions?;
    let count = columns.checked_mul(rows)? as usize;
    if count != map_count
        || coordinates.len() != count
        || (0..rows)
            .any(|row| (0..columns).any(|column| !coordinates.contains(&(column + 1, row + 1))))
    {
        return None;
    }
    Some((columns, rows))
}

fn parse_grid_axis(value: &str, label: &str) -> Option<(u32, u32)> {
    let value = value.strip_prefix(label)?.trim();
    let (index, total) = value.split_once('/')?;
    let index = index.trim().parse().ok()?;
    let total = total.trim().parse().ok()?;
    if index == 0 || total == 0 || index > total {
        return None;
    }
    Some((index, total))
}

fn validate_refs(value: &NbtTag, ids: &BTreeSet<i64>) -> Result<()> {
    match value {
        NbtTag::Compound(fields) => {
            if matches!(fields.get("Name"), Some(NbtTag::String(name)) if name == "minecraft:filled_map")
            {
                let id = filled_map_id(value)
                    .and_then(|id| id.as_str().parse::<i64>().ok())
                    .ok_or_else(|| {
                        BedrockWorldError::Validation(
                            "filled map has no numeric map_uuid Long".to_string(),
                        )
                    })?;
                if !ids.contains(&id) {
                    return Err(BedrockWorldError::Validation(
                        "filled map references a record outside its bundle".to_string(),
                    ));
                }
            }
            for child in fields.values() {
                validate_refs(child, ids)?;
            }
        }
        NbtTag::List(values) => {
            for child in values {
                validate_refs(child, ids)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_item::{Pixels, new_filled_map_item};

    #[test]
    fn nbt_bundle_roundtrip_and_import_remap_preserve_map_bytes_and_labels() {
        let map = SavedData::new_locked_pixels(
            7,
            Pixels {
                width: 128,
                height: 128,
                rgba: vec![42; 128 * 128 * 4],
            },
            0,
            0,
            0,
            0,
        )
        .expect("map");
        let item = new_filled_map_item(7, 0).expect("item");
        let container = NbtTag::Compound(IndexMap::from([(
            "Items".to_string(),
            NbtTag::List(vec![item]),
        )]));
        let bundle = MapBundle::new(vec![map], container)
            .expect("bundle")
            .with_grid(1, 1)
            .expect("grid");
        let bytes = bundle.to_nbt_bytes().expect("encode");
        let loaded = MapBundle::from_nbt_bytes(&bytes).expect("decode");
        assert_eq!(loaded.grid(), Some((1, 1)));
        let imported = loaded.remap(&BTreeMap::from([(7, 91)])).expect("remap");
        assert_eq!(imported.grid(), Some((1, 1)));
        assert_eq!(imported.flat_items().expect("flat maps").len(), 1);
        assert_eq!(imported.records[0].id.as_str(), "91");
        assert!(imported.records[0].raw.is_empty());
        assert_eq!(
            imported.records[0].pixels.as_ref().expect("pixels").rgba[0],
            42
        );
        let NbtTag::Compound(root) = &imported.container else {
            panic!("container")
        };
        let NbtTag::List(items) = &root["Items"] else {
            panic!("items")
        };
        assert_eq!(
            filled_map_id(&items[0]).as_ref().map(MapItemId::as_str),
            Some("91")
        );
        assert!(loaded.remap(&BTreeMap::from([(7, 91), (8, 92)])).is_err());
    }

    #[test]
    fn old_bundle_grid_is_inferred_from_coordinate_lore() {
        let records = (0..2)
            .map(|id| {
                SavedData::new_locked_pixels(
                    id,
                    Pixels {
                        width: 128,
                        height: 128,
                        rgba: vec![0; 128 * 128 * 4],
                    },
                    0,
                    0,
                    0,
                    0,
                )
                .expect("map")
            })
            .collect();
        let items = (0..2)
            .map(|column| {
                let mut item = new_filled_map_item(column, 0).expect("item");
                crate::item::set_item_display(
                    &mut item,
                    &format!("tile {}", column + 1),
                    &[format!("BMCBL · 北向上 · 列 {}/2 · 行 1/1", column + 1)],
                )
                .expect("lore");
                item
            })
            .collect();
        let bundle = MapBundle::new(
            records,
            NbtTag::Compound(IndexMap::from([("Items".to_string(), NbtTag::List(items))])),
        )
        .expect("old bundle");
        assert_eq!(bundle.grid(), Some((2, 1)));
    }

    #[test]
    fn flat_items_rejects_missing_or_duplicate_map_references() {
        let map = SavedData::new_locked_pixels(
            7,
            Pixels {
                width: 128,
                height: 128,
                rgba: vec![0; 128 * 128 * 4],
            },
            0,
            0,
            0,
            0,
        )
        .expect("map");
        let item = new_filled_map_item(7, 0).expect("item");
        let missing = MapBundle::new(
            vec![map.clone()],
            NbtTag::Compound(IndexMap::from([(
                "Items".to_string(),
                NbtTag::List(Vec::new()),
            )])),
        )
        .expect("bundle");
        assert!(missing.flat_items().is_err());
        let duplicate = MapBundle::new(
            vec![map],
            NbtTag::Compound(IndexMap::from([(
                "Items".to_string(),
                NbtTag::List(vec![item.clone(), item]),
            )])),
        )
        .expect("bundle");
        assert!(duplicate.flat_items().is_err());
    }
}
