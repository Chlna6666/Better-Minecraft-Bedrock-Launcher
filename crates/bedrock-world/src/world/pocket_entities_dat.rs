//! Pre-LevelDB Minecraft Pocket Edition `entities.dat` reading, writing and record import.
//!
//! MCPE 0.6.x stores world entities outside `chunks.dat`: a 12-byte `ENT\0` header followed by one
//! little-endian NBT root containing `Entities` and `TileEntities` lists. This module keeps that real
//! file shape explicit. Ordinary reads/writes stay in `entities.dat`; importing to LevelDB is a
//! separate caller-requested operation.

use crate::chunk::{ChunkKey, ChunkPos, ChunkRecordTag, Dimension};
use crate::storage::{StorageBatch, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtReader, NbtTag, serialize_root_nbt};
use bytes::Bytes;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::Path;

const POCKET_ENTITIES_DAT_HEADER_LEN: usize = 12;
const POCKET_ENTITIES_DAT_VERSION: i32 = 1;

/// Parsed pre-LevelDB Pocket Edition `entities.dat` document.
///
/// The source bytes and parsed NBT are both retained. An unchanged document serializes to the exact
/// original bytes, including any bytes after the declared NBT payload.
#[derive(Debug, Clone, PartialEq)]
pub struct PocketEntitiesDatDocument {
    version: i32,
    original_version: i32,
    root: NbtTag,
    original_root: NbtTag,
    raw: Bytes,
    trailing: Bytes,
}

impl PocketEntitiesDatDocument {
    /// Parses one complete `entities.dat` byte buffer.
    pub fn from_raw(raw: Bytes) -> Result<Self> {
        if raw.len() < POCKET_ENTITIES_DAT_HEADER_LEN {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "entities.dat is too short: {} bytes",
                raw.len()
            )));
        }
        if raw.get(..4) != Some(b"ENT\0".as_slice()) {
            return Err(BedrockWorldError::CorruptWorld(
                "entities.dat does not start with ENT\\0".to_string(),
            ));
        }

        let version = i32::from_le_bytes(raw[4..8].try_into().expect("four checked bytes"));
        if version < 0 {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "entities.dat has negative file version {version}"
            )));
        }
        let declared_len = i32::from_le_bytes(raw[8..12].try_into().expect("four checked bytes"));
        let declared_len = usize::try_from(declared_len).map_err(|_| {
            BedrockWorldError::CorruptWorld("entities.dat has negative NBT byte length".to_string())
        })?;
        if declared_len == 0 {
            return Err(BedrockWorldError::CorruptWorld(
                "entities.dat declares an empty NBT payload".to_string(),
            ));
        }
        let payload_end = POCKET_ENTITIES_DAT_HEADER_LEN
            .checked_add(declared_len)
            .ok_or_else(|| {
                BedrockWorldError::CorruptWorld(
                    "entities.dat NBT byte length overflows file offset".to_string(),
                )
            })?;
        if payload_end > raw.len() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "entities.dat declares {declared_len} NBT bytes but only {} are available",
                raw.len().saturating_sub(POCKET_ENTITIES_DAT_HEADER_LEN)
            )));
        }

        let payload = raw.slice(POCKET_ENTITIES_DAT_HEADER_LEN..payload_end);
        let (root, consumed) = NbtReader::new(payload.as_ref()).parse_root_with_consumed()?;
        if consumed != payload.len() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "entities.dat NBT root consumed {consumed} of {} declared bytes",
                payload.len()
            )));
        }
        ensure_root_compound(&root)?;
        let trailing = raw.slice(payload_end..);
        Ok(Self {
            version,
            original_version: version,
            original_root: root.clone(),
            root,
            raw,
            trailing,
        })
    }

    /// Creates an `entities.dat` document from structured Bedrock NBT.
    pub fn new(version: i32, root: NbtTag) -> Result<Self> {
        if version < 0 {
            return Err(BedrockWorldError::Validation(
                "entities.dat version cannot be negative".to_string(),
            ));
        }
        ensure_root_compound(&root)?;
        let raw = encode_document(version, &root, &[])?;
        Ok(Self {
            version,
            original_version: version,
            original_root: root.clone(),
            root,
            raw,
            trailing: Bytes::new(),
        })
    }

    /// Returns the exact file version from the `entities.dat` header.
    #[must_use]
    pub const fn version(&self) -> i32 {
        self.version
    }

    /// Changes the explicit `entities.dat` file version written by [`Self::to_raw`].
    pub fn set_version(&mut self, version: i32) -> Result<()> {
        if version < 0 {
            return Err(BedrockWorldError::Validation(
                "entities.dat version cannot be negative".to_string(),
            ));
        }
        self.version = version;
        Ok(())
    }

    /// Returns the complete root compound without normalising either entity list.
    #[must_use]
    pub const fn root(&self) -> &NbtTag {
        &self.root
    }

    /// Returns the complete mutable root compound.
    ///
    /// Callers remain responsible for retaining the real `Entities`/`TileEntities` list shapes when
    /// the document must be readable by the old game.
    pub fn root_mut(&mut self) -> &mut NbtTag {
        &mut self.root
    }

    /// Returns the persisted `Entities` list.
    pub fn entities(&self) -> Result<&[NbtTag]> {
        root_list(&self.root, "Entities")
    }

    /// Returns the persisted `TileEntities` list.
    ///
    /// `TileEntities` is the historical file field name; these values become Bedrock `BlockEntity`
    /// chunk records when explicitly imported into LevelDB.
    pub fn tile_entities(&self) -> Result<&[NbtTag]> {
        root_list(&self.root, "TileEntities")
    }

    /// Returns whether the header version or structured NBT differs from the source document.
    #[must_use]
    pub fn is_modified(&self) -> bool {
        self.version != self.original_version || self.root != self.original_root
    }

    /// Serializes the current document.
    ///
    /// Unchanged documents return the exact source bytes. Edited documents retain any bytes that were
    /// present after the source's declared NBT payload.
    pub fn to_raw(&self) -> Result<Bytes> {
        if !self.is_modified() {
            return Ok(self.raw.clone());
        }
        encode_document(self.version, &self.root, self.trailing.as_ref())
    }
}

/// Reads `entities.dat` from one pre-LevelDB Pocket Edition world folder.
pub fn read_pocket_entities_dat(world_path: impl AsRef<Path>) -> Result<PocketEntitiesDatDocument> {
    let raw = Bytes::from(fs::read(world_path.as_ref().join("entities.dat"))?);
    PocketEntitiesDatDocument::from_raw(raw)
}

/// Writes one pre-LevelDB Pocket Edition `entities.dat` document using the game's `_new`/`_old`
/// sidecar pattern.
pub fn write_pocket_entities_dat_atomic(
    world_path: impl AsRef<Path>,
    document: &PocketEntitiesDatDocument,
) -> Result<()> {
    let world_path = world_path.as_ref();
    let path = world_path.join("entities.dat");
    let new_path = world_path.join("entities.dat_new");
    let old_path = world_path.join("entities.dat_old");
    let raw = document.to_raw()?;

    {
        let mut file = fs::File::create(&new_path)?;
        file.write_all(raw.as_ref())?;
        file.sync_all()?;
    }
    if old_path.exists() {
        fs::remove_file(&old_path)?;
    }
    if path.exists() {
        fs::rename(&path, &old_path)?;
    }
    if let Err(error) = fs::rename(&new_path, &path) {
        if old_path.exists() && !path.exists() {
            let _ = fs::rename(&old_path, &path);
        }
        return Err(BedrockWorldError::Io(error));
    }
    Ok(())
}

/// Settings for explicitly importing historical `entities.dat` lists into LevelDB chunk records.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PocketEntitiesDatImportOptions {
    /// Allows replacing an existing target `Entity` or `BlockEntity` chunk record.
    pub overwrite_existing: bool,
    /// Allows source entities without a usable position to be skipped rather than rejecting import.
    pub skip_unpositioned: bool,
}

/// Result of importing one historical `entities.dat` file into LevelDB chunk records.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PocketEntitiesDatImportReport {
    /// Source `entities.dat` header version.
    pub source_version: i32,
    /// Entity compounds written from the `Entities` list.
    pub entities: usize,
    /// Tile-entity compounds written from the `TileEntities` list.
    pub tile_entities: usize,
    /// Source entities skipped because `skip_unpositioned` was enabled.
    pub skipped_unpositioned_entities: usize,
    /// Source tile entities skipped because `skip_unpositioned` was enabled.
    pub skipped_unpositioned_tile_entities: usize,
    /// Generated legacy chunk `Entity` records.
    pub entity_chunk_records: usize,
    /// Generated chunk `BlockEntity` records.
    pub block_entity_chunk_records: usize,
    /// Number of generated LevelDB value bytes written.
    pub bytes_written: usize,
}

/// Imports a pre-LevelDB Pocket Edition `entities.dat` into legacy inline `Entity` and `BlockEntity`
/// chunk records using one atomic target storage batch.
///
/// The operation does not jump directly to `digp`/`actorprefix`: that is a later Bedrock storage
/// generation and requires a separate explicit actor-storage write. Every source position, serialized
/// target value and target collision is checked before the single [`WorldStorage::write_batch`] call.
/// Therefore a backend batch failure cannot leave only a prefix of the source entities imported.
pub fn import_pocket_entities_dat_records_blocking(
    source_world_path: impl AsRef<Path>,
    target: &dyn WorldStorage,
    options: PocketEntitiesDatImportOptions,
) -> Result<PocketEntitiesDatImportReport> {
    let document = read_pocket_entities_dat(source_world_path)?;
    if document.version() != POCKET_ENTITIES_DAT_VERSION {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "entities.dat version {} is not the confirmed Pocket Edition version {}",
            document.version(),
            POCKET_ENTITIES_DAT_VERSION
        )));
    }

    let mut entity_groups = BTreeMap::<ChunkPos, Vec<&NbtTag>>::new();
    let mut tile_entity_groups = BTreeMap::<ChunkPos, Vec<&NbtTag>>::new();
    let mut report = PocketEntitiesDatImportReport {
        source_version: document.version(),
        ..PocketEntitiesDatImportReport::default()
    };

    for (index, entity) in document.entities()?.iter().enumerate() {
        match entity_chunk_pos(entity)? {
            Some(pos) => {
                entity_groups.entry(pos).or_default().push(entity);
                report.entities = report.entities.saturating_add(1);
            }
            None if options.skip_unpositioned => {
                report.skipped_unpositioned_entities =
                    report.skipped_unpositioned_entities.saturating_add(1);
            }
            None => {
                return Err(BedrockWorldError::Validation(format!(
                    "entities.dat Entities[{index}] has no usable Pos; set skip_unpositioned explicitly to omit it"
                )));
            }
        }
    }
    for (index, tile_entity) in document.tile_entities()?.iter().enumerate() {
        match tile_entity_chunk_pos(tile_entity)? {
            Some(pos) => {
                tile_entity_groups.entry(pos).or_default().push(tile_entity);
                report.tile_entities = report.tile_entities.saturating_add(1);
            }
            None if options.skip_unpositioned => {
                report.skipped_unpositioned_tile_entities =
                    report.skipped_unpositioned_tile_entities.saturating_add(1);
            }
            None => {
                return Err(BedrockWorldError::Validation(format!(
                    "entities.dat TileEntities[{index}] has no usable x/z; set skip_unpositioned explicitly to omit it"
                )));
            }
        }
    }

    let mut records = Vec::<(Bytes, Bytes)>::with_capacity(
        entity_groups.len().saturating_add(tile_entity_groups.len()),
    );
    for (pos, roots) in entity_groups {
        let value = serialize_consecutive_roots(&roots)?;
        records.push((ChunkKey::new(pos, ChunkRecordTag::Entity).encode(), value));
        report.entity_chunk_records = report.entity_chunk_records.saturating_add(1);
    }
    for (pos, roots) in tile_entity_groups {
        let value = serialize_consecutive_roots(&roots)?;
        records.push((
            ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode(),
            value,
        ));
        report.block_entity_chunk_records = report.block_entity_chunk_records.saturating_add(1);
    }

    if !options.overwrite_existing {
        for (key, _) in &records {
            if target.get(key.as_ref())?.is_some() {
                return Err(BedrockWorldError::Validation(format!(
                    "Pocket entities.dat import target already contains key {:02x?}",
                    key.as_ref()
                )));
            }
        }
    }

    let mut batch = StorageBatch::new();
    for (key, value) in records {
        report.bytes_written = report.bytes_written.saturating_add(value.len());
        batch.put(key, value);
    }
    if !batch.is_empty() {
        target.write_batch(&batch)?;
    }
    Ok(report)
}

fn ensure_root_compound(root: &NbtTag) -> Result<()> {
    if matches!(root, NbtTag::Compound(_)) {
        Ok(())
    } else {
        Err(BedrockWorldError::CorruptWorld(
            "entities.dat NBT root is not a compound".to_string(),
        ))
    }
}

fn root_list<'a>(root: &'a NbtTag, field: &str) -> Result<&'a [NbtTag]> {
    let NbtTag::Compound(root) = root else {
        return Err(BedrockWorldError::CorruptWorld(
            "entities.dat NBT root is not a compound".to_string(),
        ));
    };
    let Some(value) = root.get(field) else {
        return Ok(&[]);
    };
    let NbtTag::List(values) = value else {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "entities.dat {field} is not an NBT list"
        )));
    };
    for (index, value) in values.iter().enumerate() {
        if !matches!(value, NbtTag::Compound(_)) {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "entities.dat {field}[{index}] is not an NBT compound"
            )));
        }
    }
    Ok(values)
}

fn encode_document(version: i32, root: &NbtTag, trailing: &[u8]) -> Result<Bytes> {
    ensure_root_compound(root)?;
    let payload = serialize_root_nbt(root)?;
    let payload_len = i32::try_from(payload.len()).map_err(|_| {
        BedrockWorldError::Validation("entities.dat NBT payload exceeds i32 length".to_string())
    })?;
    let mut raw = Vec::with_capacity(
        POCKET_ENTITIES_DAT_HEADER_LEN
            .saturating_add(payload.len())
            .saturating_add(trailing.len()),
    );
    raw.extend_from_slice(b"ENT\0");
    raw.extend_from_slice(&version.to_le_bytes());
    raw.extend_from_slice(&payload_len.to_le_bytes());
    raw.extend_from_slice(&payload);
    raw.extend_from_slice(trailing);
    Ok(Bytes::from(raw))
}

fn entity_chunk_pos(entity: &NbtTag) -> Result<Option<ChunkPos>> {
    let NbtTag::Compound(root) = entity else {
        return Err(BedrockWorldError::CorruptWorld(
            "entities.dat entity is not an NBT compound".to_string(),
        ));
    };
    let Some(value) = root.get("Pos") else {
        return Ok(None);
    };
    let NbtTag::List(values) = value else {
        return Err(BedrockWorldError::CorruptWorld(
            "entities.dat entity Pos is not an NBT list".to_string(),
        ));
    };
    if values.len() < 3 {
        return Ok(None);
    }
    let Some(x) = numeric_value(&values[0]) else {
        return Ok(None);
    };
    let Some(z) = numeric_value(&values[2]) else {
        return Ok(None);
    };
    let Some(x) = floor_i32(x) else {
        return Ok(None);
    };
    let Some(z) = floor_i32(z) else {
        return Ok(None);
    };
    Ok(Some(ChunkPos {
        x: x.div_euclid(16),
        z: z.div_euclid(16),
        dimension: Dimension::Overworld,
    }))
}

fn tile_entity_chunk_pos(tile_entity: &NbtTag) -> Result<Option<ChunkPos>> {
    let NbtTag::Compound(root) = tile_entity else {
        return Err(BedrockWorldError::CorruptWorld(
            "entities.dat tile entity is not an NBT compound".to_string(),
        ));
    };
    let Some(x) = integer_value(root.get("x")) else {
        return Ok(None);
    };
    let Some(z) = integer_value(root.get("z")) else {
        return Ok(None);
    };
    Ok(Some(ChunkPos {
        x: x.div_euclid(16),
        z: z.div_euclid(16),
        dimension: Dimension::Overworld,
    }))
}

fn numeric_value(tag: &NbtTag) -> Option<f64> {
    let value = match tag {
        NbtTag::Byte(value) => f64::from(*value),
        NbtTag::Short(value) => f64::from(*value),
        NbtTag::Int(value) => f64::from(*value),
        NbtTag::Long(value) => *value as f64,
        NbtTag::Float(value) => f64::from(*value),
        NbtTag::Double(value) => *value,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

fn floor_i32(value: f64) -> Option<i32> {
    let value = value.floor();
    if value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        None
    } else {
        Some(value as i32)
    }
}

fn integer_value(tag: Option<&NbtTag>) -> Option<i32> {
    match tag? {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn serialize_consecutive_roots(roots: &[&NbtTag]) -> Result<Bytes> {
    let mut raw = Vec::new();
    for root in roots {
        raw.extend_from_slice(&serialize_root_nbt(root)?);
    }
    Ok(Bytes::from(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryStorage;
    use indexmap::IndexMap;

    fn entity(x: f32, z: f32) -> NbtTag {
        NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::Int(10)),
            (
                "Pos".to_string(),
                NbtTag::List(vec![
                    NbtTag::Float(x),
                    NbtTag::Float(64.0),
                    NbtTag::Float(z),
                ]),
            ),
        ]))
    }

    fn tile_entity(x: i32, z: i32) -> NbtTag {
        NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::String("Chest".to_string())),
            ("x".to_string(), NbtTag::Int(x)),
            ("y".to_string(), NbtTag::Int(64)),
            ("z".to_string(), NbtTag::Int(z)),
        ]))
    }

    fn document() -> PocketEntitiesDatDocument {
        PocketEntitiesDatDocument::new(
            1,
            NbtTag::Compound(IndexMap::from([
                (
                    "Entities".to_string(),
                    NbtTag::List(vec![entity(1.5, 2.5), entity(-1.0, -17.0)]),
                ),
                (
                    "TileEntities".to_string(),
                    NbtTag::List(vec![tile_entity(3, 4)]),
                ),
            ])),
        )
        .unwrap()
    }

    #[test]
    fn document_roundtrips_ent_header_and_nbt() {
        let document = document();
        let raw = document.to_raw().unwrap();
        assert_eq!(&raw[..4], b"ENT\0");
        assert_eq!(i32::from_le_bytes(raw[4..8].try_into().unwrap()), 1);
        let reparsed = PocketEntitiesDatDocument::from_raw(raw.clone()).unwrap();
        assert_eq!(reparsed.to_raw().unwrap(), raw);
        assert_eq!(reparsed.entities().unwrap().len(), 2);
        assert_eq!(reparsed.tile_entities().unwrap().len(), 1);
    }

    #[test]
    fn positions_use_floor_and_negative_chunk_coordinates() {
        let first = entity_chunk_pos(&entity(1.5, 2.5)).unwrap().unwrap();
        assert_eq!((first.x, first.z), (0, 0));
        let negative = entity_chunk_pos(&entity(-1.0, -17.0)).unwrap().unwrap();
        assert_eq!((negative.x, negative.z), (-1, -2));
    }

    #[test]
    fn edited_document_keeps_unknown_root_fields() {
        let mut document = document();
        let NbtTag::Compound(root) = document.root_mut() else {
            panic!("compound");
        };
        root.insert("FutureField".to_string(), NbtTag::Long(99));
        let raw = document.to_raw().unwrap();
        let reparsed = PocketEntitiesDatDocument::from_raw(raw).unwrap();
        let NbtTag::Compound(root) = reparsed.root() else {
            panic!("compound");
        };
        assert_eq!(root.get("FutureField"), Some(&NbtTag::Long(99)));
    }

    #[test]
    fn collision_preflight_happens_before_target_mutation() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::new(pos, ChunkRecordTag::Entity).encode();
        storage.put(key.as_ref(), b"existing").unwrap();

        let document = document();
        let root =
            std::env::temp_dir().join(format!("bedrock-pocket-entities-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("entities.dat"), document.to_raw().unwrap()).unwrap();
        let result = import_pocket_entities_dat_records_blocking(
            &root,
            &storage,
            PocketEntitiesDatImportOptions::default(),
        );
        assert!(result.is_err());
        assert_eq!(
            storage.get(key.as_ref()).unwrap().unwrap().as_ref(),
            b"existing"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn successful_import_commits_all_generated_records_together() {
        let storage = MemoryStorage::new();
        let document = document();
        let root = std::env::temp_dir().join(format!(
            "bedrock-pocket-entities-success-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("entities.dat"), document.to_raw().unwrap()).unwrap();

        let report = import_pocket_entities_dat_records_blocking(
            &root,
            &storage,
            PocketEntitiesDatImportOptions::default(),
        )
        .unwrap();
        assert_eq!(report.entities, 2);
        assert_eq!(report.tile_entities, 1);
        assert_eq!(report.entity_chunk_records, 2);
        assert_eq!(report.block_entity_chunk_records, 1);
        fs::remove_dir_all(root).unwrap();
    }
}
