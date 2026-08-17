//! Structured parsers layered above raw Bedrock LevelDB records.
//!
//! The parser family in this module is designed for inspection and tooling:
//! callers can choose summary-only scans, structured parsed entries, or raw
//! retention for offline debugging. Parse failures are accumulated in
//! [`WorldParseReport`] where possible so a single unknown record does not stop
//! a full-world scan.

use crate::chunk::{
    ActorUid, BedrockDbKey, ChunkPos, ChunkRecord, ChunkRecordTag, ChunkVersion, GlobalRecordKind,
    LegacyTerrain, MapRecordId, ParsedVillageKey, SubChunk, SubChunkDecodeMode, SubChunkFormat,
    parse_subchunk_with_mode,
};
use crate::error::{BedrockWorldError, Result as WorldResult};
use crate::level_dat::LevelDatDocument;
use crate::nbt::{NbtTag, parse_consecutive_root_nbt, parse_root_nbt, serialize_root_nbt};
use crate::storage::{StorageReadOptions, StorageVisitorControl, WorldStorage};
use bytes::Bytes;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const MAX_BIOME_PALETTE_LEN: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
/// Parsed view of a world scan.
pub struct ParsedWorld {
    /// Parsed `level.dat` document read before scanning storage.
    pub level_dat: LevelDatDocument,
    /// Parsed database entries retained according to [`RetentionMode`].
    pub entries: Vec<ParsedDbEntry>,
    /// Aggregate counters, warnings, and parse errors.
    pub report: WorldParseReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Options controlling scan breadth and retained output.
pub struct WorldParseOptions {
    /// Database record categories to parse.
    pub categories: WorldParseCategories,
    /// Raw/structured retention strategy.
    pub retention: RetentionMode,
    /// Subchunk decode strategy.
    pub subchunk_decode_mode: SubChunkDecodeMode,
    /// Actor lookup strategy for `digp` records.
    pub actor_resolution: ActorResolution,
}

impl WorldParseOptions {
    #[must_use]
    /// Returns summary-oriented parse options for large scans.
    pub const fn summary() -> Self {
        Self {
            categories: WorldParseCategories::all(),
            retention: RetentionMode::Summary,
            subchunk_decode_mode: SubChunkDecodeMode::CountsOnly,
            actor_resolution: ActorResolution::ResolveReferenced,
        }
    }

    #[must_use]
    /// Returns options that retain structured parsed entries without raw values.
    pub const fn structured() -> Self {
        Self {
            categories: WorldParseCategories::all(),
            retention: RetentionMode::Structured,
            subchunk_decode_mode: SubChunkDecodeMode::CountsOnly,
            actor_resolution: ActorResolution::ResolveReferenced,
        }
    }

    #[must_use]
    /// Returns options that retain structured entries, raw values, and full subchunk indices.
    pub const fn full_raw() -> Self {
        Self {
            categories: WorldParseCategories::all(),
            retention: RetentionMode::FullRaw,
            subchunk_decode_mode: SubChunkDecodeMode::FullIndices,
            actor_resolution: ActorResolution::ResolveAll,
        }
    }

    #[must_use]
    /// Returns the full raw parse options.
    pub const fn full() -> Self {
        Self::full_raw()
    }
}

impl Default for WorldParseOptions {
    fn default() -> Self {
        Self::summary()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Category toggles for world-level parsing.
pub struct WorldParseCategories {
    /// Parse chunk records.
    pub chunks: bool,
    /// Parse player records.
    pub players: bool,
    /// Parse actor and actor digest records.
    pub actors: bool,
    /// Parse map records.
    pub maps: bool,
    /// Parse village records.
    pub villages: bool,
    /// Parse known global records.
    pub globals: bool,
}

impl WorldParseCategories {
    #[must_use]
    /// Enables parsing for every supported record category.
    pub const fn all() -> Self {
        Self {
            chunks: true,
            players: true,
            actors: true,
            maps: true,
            villages: true,
            globals: true,
        }
    }

    #[must_use]
    /// Disables value parsing so scans retain only key classification and counters.
    pub const fn keys_only() -> Self {
        Self {
            chunks: false,
            players: false,
            actors: false,
            maps: false,
            villages: false,
            globals: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Controls how much raw data is kept in parse output.
pub enum RetentionMode {
    /// Keep counts and high-level summaries only.
    Summary,
    /// Keep structured parsed entries without raw values.
    Structured,
    /// Keep structured entries and raw values.
    FullRaw,
}

impl RetentionMode {
    #[must_use]
    /// Returns whether parsed entries are retained in output.
    pub const fn retains_entries(self) -> bool {
        matches!(self, Self::Structured | Self::FullRaw)
    }

    #[must_use]
    /// Returns whether raw value bytes are retained in output.
    pub const fn retains_raw(self) -> bool {
        matches!(self, Self::FullRaw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
/// Actor resolution strategy for `digp` actor digest records.
pub enum ActorResolution {
    /// Do not follow actor references.
    None,
    /// Keep actor digest ids without resolving actorprefix values.
    DigestOnly,
    /// Resolve actors referenced by digest records.
    ResolveReferenced,
    /// Resolve all actorprefix records encountered by a scan.
    ResolveAll,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
/// Aggregate counters and non-fatal diagnostics produced by parsers.
pub struct WorldParseReport {
    /// Number of storage entries visited during the scan.
    pub entry_count: usize,
    /// Number of chunks represented by these bounds.
    pub chunk_count: usize,
    /// Number of modern subchunk records parsed.
    pub subchunk_count: usize,
    /// Number of legacy subchunk records parsed.
    pub legacy_subchunk_count: usize,
    /// Number of legacy terrain records parsed.
    pub legacy_terrain_count: usize,
    /// Number of subchunk storage layers decoded.
    pub subchunk_storage_count: usize,
    /// Number of block palette states decoded from subchunks.
    pub palette_state_count: usize,
    /// Number of entity records parsed.
    pub entity_count: usize,
    /// Number of block entity records parsed.
    pub block_entity_count: usize,
    /// Number of item stacks found inside parsed NBT payloads.
    pub item_count: usize,
    /// Number of player records parsed.
    pub player_count: usize,
    /// Number of parsed NBT roots not classified as a known record family.
    pub other_nbt_root_count: usize,
    /// Number of entries whose raw bytes were retained.
    pub raw_entry_count: usize,
    /// Number of actor digest records parsed.
    pub actor_digest_count: usize,
    /// Number of actor digest references resolved to actor records.
    pub actor_digest_hit_count: usize,
    /// Number of actor digest references missing a matching actor record.
    pub actor_digest_missing_count: usize,
    /// Number of biome records parsed.
    pub biome_record_count: usize,
    /// Number of biome storage layers decoded.
    pub biome_layer_count: usize,
    /// Number of hardcoded spawn area records parsed.
    pub hardcoded_spawn_area_count: usize,
    /// Number of village records parsed.
    pub village_record_count: usize,
    /// Number of map records parsed.
    pub map_record_count: usize,
    /// Number of global records parsed.
    pub global_record_count: usize,
    /// Counts grouped by decoded key kind.
    pub key_kinds: BTreeMap<String, usize>,
    /// Non-fatal warnings collected while parsing.
    pub warnings: Vec<String>,
    /// Non-fatal parse errors collected while scanning.
    pub parse_errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed storage entry retained by a world parse.
pub struct ParsedDbEntry {
    /// Decoded storage key for this record.
    pub key: BedrockDbKey,
    /// Original storage key bytes.
    pub raw_key: Bytes,
    /// Length of the original storage value in bytes.
    pub raw_value_len: usize,
    /// Parsed value or retained raw bytes for this record.
    pub value: ParsedDbValue,
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed chunk result and per-chunk parse report.
pub struct ParsedChunkData {
    /// Chunk position represented by this parsed result.
    pub pos: ChunkPos,
    /// Records included in this result.
    pub records: Vec<ParsedChunkRecord>,
    /// Counters and non-fatal diagnostics collected while parsing.
    pub report: WorldParseReport,
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed high-level value for a classified storage entry.
pub enum ParsedDbValue {
    /// Chunk-scoped storage record.
    Chunk(ParsedChunkRecord),
    /// Player record value.
    Player(ParsedPlayer),
    /// Actor entities decoded from the value.
    ActorEntities(Vec<ParsedEntity>),
    /// Modern actor digest record.
    ActorDigest(ParsedActorDigest),
    /// Map record value.
    MapData(ParsedMapData),
    /// Village record value.
    VillageData(ParsedVillageData),
    /// Global record value.
    GlobalData(ParsedGlobalData),
    /// Consecutive NBT roots decoded from the value.
    NbtRoots(Vec<NbtTag>),
    /// Raw bytes preserved because the payload was not decoded.
    Raw(Bytes),
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed chunk record value paired with its decoded key.
pub struct ParsedChunkRecord {
    /// Decoded storage key for this record.
    pub key: crate::ChunkKey,
    /// Parsed payload or retained raw bytes for this record.
    pub value: ParsedChunkRecordValue,
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed payload stored under a chunk record tag.
pub enum ParsedChunkRecordValue {
    /// Data sourced from decoded subchunks.
    SubChunk(SubChunk),
    /// Old `LevelDB`-era terrain record.
    LegacyTerrain(LegacyTerrain),
    /// Consecutive entity NBT roots decoded from a chunk entity record.
    Entities(Vec<ParsedEntity>),
    /// Consecutive block-entity NBT roots decoded from a chunk record.
    BlockEntities(Vec<ParsedBlockEntity>),
    /// Pending tick NBT record.
    PendingTicks(Vec<NbtTag>),
    /// Current chunk version record.
    Version(u8),
    /// Finalized state record.
    FinalizedState(i32),
    /// Biome metadata record.
    BiomeData(ParsedBiomeData),
    /// Hardcoded spawn areas decoded from chunk storage.
    HardcodedSpawnAreas(Vec<ParsedHardcodedSpawnArea>),
    /// Raw bytes preserved because the payload was not decoded.
    Raw(Bytes),
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed modern actor digest and resolved actor payloads.
pub struct ParsedActorDigest {
    /// Chunk position whose digest referenced these actor ids.
    pub pos: crate::ChunkPos,
    /// Actor ids referenced by a digest record.
    pub actor_ids: Vec<i64>,
    /// Parsed entity records included in this value.
    pub entities: Vec<ParsedEntity>,
    /// Number of actor ids whose actorprefix record was missing.
    pub missing_actor_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
/// Source record used to load an actor.
pub enum ActorSource {
    /// Legacy inline `Entity` chunk record.
    InlineChunk(crate::ChunkKey),
    /// Modern `actorprefix<uid>` record referenced by a `digp` digest.
    ActorPrefix(ActorUid),
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed actor plus the storage record that produced it.
pub struct ActorRecord {
    /// Actor UID when one is available from the key or entity NBT.
    pub uid: Option<ActorUid>,
    /// Storage source for the actor payload.
    pub source: ActorSource,
    /// Parsed entity view.
    pub entity: ParsedEntity,
    /// Raw actor payload bytes for roundtrip preservation.
    pub raw: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Parsed biome payload with height map and storage layers.
pub struct ParsedBiomeData {
    /// Bedrock format or payload version.
    pub version: ChunkVersion,
    /// Height-map values in Bedrock `z * 16 + x` column order.
    pub height_map: Vec<i16>,
    /// Biome or block storages decoded from the record.
    pub storages: Vec<ParsedBiomeStorage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Parsed biome palette storage layer.
pub struct ParsedBiomeStorage {
    /// Vertical biome section index for 3D data, or `None` for 2D biome data.
    pub y: Option<i32>,
    /// Palette values referenced by packed indices.
    pub palette: Vec<u32>,
    /// Optional unpacked palette indices in Bedrock storage order.
    pub indices: Option<Vec<u16>>,
    /// Per-palette-entry usage counts collected while decoding.
    pub counts: Vec<u16>,
}

impl ParsedBiomeStorage {
    #[must_use]
    /// Returns a palette index at local subchunk coordinates.
    ///
    /// For 3D biome storage the index order is Bedrock's X-major
    /// `x * 256 + z * 16 + y` order. For old 2D storage the index is the
    /// horizontal `z * 16 + x` column order.
    pub fn palette_index_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u16> {
        if local_x >= 16 || local_y >= 16 || local_z >= 16 {
            return None;
        }
        let index = if self.y.is_some() {
            crate::block_storage_index(local_x, local_y, local_z)
        } else {
            usize::from(local_z) * 16 + usize::from(local_x)
        };
        self.indices.as_ref()?.get(index).copied()
    }

    #[must_use]
    /// Returns the biome id at local subchunk coordinates.
    pub fn biome_id_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u32> {
        let palette_index = usize::from(self.palette_index_at(local_x, local_y, local_z)?);
        self.palette.get(palette_index).copied()
    }
}

impl HeightMap2d {
    /// Creates a 16x16 column height map.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::Validation`] when `values` does not contain
    /// exactly 256 entries.
    pub fn new(values: Vec<i16>) -> WorldResult<Self> {
        if values.len() != 256 {
            return Err(BedrockWorldError::Validation(format!(
                "height map must contain 256 values, got {}",
                values.len()
            )));
        }
        Ok(Self { values })
    }

    /// Parses the 512-byte little-endian height map prefix used by Data2D/Data3D.
    ///
    /// # Errors
    ///
    /// Returns validation errors for truncated input.
    pub fn from_bytes(bytes: &[u8]) -> WorldResult<Self> {
        read_height_map(bytes)
            .map(|values| Self { values })
            .map_err(BedrockWorldError::Validation)
    }

    #[must_use]
    /// Serializes this height map as 256 little-endian `i16` values.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(512);
        for value in &self.values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    #[must_use]
    /// Returns a height at local chunk coordinates.
    pub fn get(&self, local_x: u8, local_z: u8) -> Option<i16> {
        if local_x >= 16 || local_z >= 16 {
            return None;
        }
        self.values
            .get(usize::from(local_z) * 16 + usize::from(local_x))
            .copied()
    }
}

impl Biome2d {
    /// Creates a legacy `Data2D` record model.
    ///
    /// # Errors
    ///
    /// Returns validation errors unless both the height map and biome map have
    /// exactly 256 entries.
    pub fn new(height_map: Vec<i16>, biomes: Vec<u8>) -> WorldResult<Self> {
        HeightMap2d::new(height_map.clone())?;
        if biomes.len() != 256 {
            return Err(BedrockWorldError::Validation(format!(
                "2D biome map must contain 256 values, got {}",
                biomes.len()
            )));
        }
        Ok(Self { height_map, biomes })
    }

    /// Parses the `Data2D` payload layout: 512 height-map bytes plus 256 biome ids.
    ///
    /// # Errors
    ///
    /// Returns validation errors for truncated or malformed input.
    pub fn parse(bytes: &[u8]) -> WorldResult<Self> {
        if bytes.len() < 768 {
            return Err(BedrockWorldError::Validation(format!(
                "Data2D is too short: {}",
                bytes.len()
            )));
        }
        Self::new(
            read_height_map(&bytes[..512]).map_err(BedrockWorldError::Validation)?,
            bytes[512..768].to_vec(),
        )
    }

    /// Serializes this model to the `Data2D` payload layout.
    ///
    /// # Errors
    ///
    /// Returns validation errors if the in-memory vectors have invalid lengths.
    pub fn encode(&self) -> WorldResult<Vec<u8>> {
        Self::new(self.height_map.clone(), self.biomes.clone())?;
        let mut bytes = HeightMap2d {
            values: self.height_map.clone(),
        }
        .to_bytes();
        bytes.extend_from_slice(&self.biomes);
        Ok(bytes)
    }
}

impl Biome3d {
    /// Creates a `Data3D` model from a height map and biome storages.
    ///
    /// # Errors
    ///
    /// Returns validation errors unless the height map has exactly 256 entries.
    pub fn new(height_map: Vec<i16>, storages: Vec<ParsedBiomeStorage>) -> WorldResult<Self> {
        HeightMap2d::new(height_map.clone())?;
        Ok(Self {
            height_map,
            storages,
        })
    }

    /// Parses a `Data3D` payload with a height map followed by biome storages.
    ///
    /// # Errors
    ///
    /// Returns validation errors for truncated or malformed biome storage data.
    pub fn parse(bytes: &[u8]) -> WorldResult<Self> {
        let parsed = parse_data3d(bytes).map_err(BedrockWorldError::Validation)?;
        Self::new(parsed.height_map, parsed.storages)
    }

    /// Serializes this model to a `Data3D` payload.
    ///
    /// # Errors
    ///
    /// Returns validation errors if height map or biome storage data is invalid.
    pub fn encode(&self) -> WorldResult<Vec<u8>> {
        Self::new(self.height_map.clone(), self.storages.clone())?;
        let mut bytes = HeightMap2d {
            values: self.height_map.clone(),
        }
        .to_bytes();
        for storage in &self.storages {
            bytes.extend_from_slice(&encode_biome_storage(storage)?);
        }
        Ok(bytes)
    }
}

impl HardcodedSpawnAreaKind {
    #[must_use]
    /// Returns the raw chunk record tag byte.
    pub const fn byte(self) -> u8 {
        match self {
            Self::NetherFortress => 1,
            Self::SwampHut => 2,
            Self::OceanMonument => 3,
            Self::PillagerOutpost => 5,
            Self::Unknown(value) => value,
        }
    }

    #[must_use]
    /// Decodes a raw chunk record tag byte.
    pub const fn from_byte(value: u8) -> Self {
        match value {
            1 => Self::NetherFortress,
            2 => Self::SwampHut,
            3 => Self::OceanMonument,
            5 => Self::PillagerOutpost,
            other => Self::Unknown(other),
        }
    }
}

impl ParsedHardcodedSpawnArea {
    /// Validates this value and returns a typed error on failure.
    pub fn validate(&self) -> WorldResult<()> {
        for axis in 0..3 {
            if self.min[axis] > self.max[axis] {
                return Err(BedrockWorldError::Validation(format!(
                    "HSA min axis {axis} exceeds max"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Hardcoded spawn area record decoded from chunk tag `0x39`.
pub struct ParsedHardcodedSpawnArea {
    /// Spawn area structure kind.
    pub kind: HardcodedSpawnAreaKind,
    /// Inclusive minimum `[x, y, z]` bounds.
    pub min: [i32; 3],
    /// Inclusive maximum `[x, y, z]` bounds.
    pub max: [i32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Known hardcoded spawn area kind, preserving unknown tag values.
pub enum HardcodedSpawnAreaKind {
    /// Nether fortress spawn area.
    NetherFortress,
    /// Swamp hut spawn area.
    SwampHut,
    /// Ocean monument spawn area.
    OceanMonument,
    /// Pillager outpost spawn area.
    PillagerOutpost,
    /// Unknown byte value preserved for roundtrip.
    Unknown(u8),
}

#[derive(Debug, Clone, PartialEq)]
/// Typed map record with decoded NBT roots and optional pixel buffer.
pub struct ParsedMapData {
    /// Legacy string id, kept for callers that consumed the v0.1 field.
    pub id: String,
    /// Validated storage id without the `map_` prefix.
    pub record_id: MapRecordId,
    /// Consecutive NBT roots stored in the map value.
    pub roots: Vec<NbtTag>,
    /// Common map fields extracted from NBT when present.
    pub known_fields: MapKnownFields,
    /// Decoded map color buffer when width, height, and color bytes are present.
    pub pixels: Option<MapPixels>,
    /// Raw value bytes for lossless preservation.
    pub raw: Bytes,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Common map NBT fields recognized by this crate.
pub struct MapKnownFields {
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
/// Raw map color buffer.
///
/// The core crate intentionally exposes bytes only and does not depend on PNG
/// or image encoders by default.
pub struct MapPixels {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Bedrock map color indices in row-major order.
    pub colors: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed village record with decoded NBT roots.
pub struct ParsedVillageData {
    /// Decoded storage key for this record.
    pub key: ParsedVillageKey,
    /// Consecutive Bedrock NBT roots decoded from the value.
    pub roots: Vec<NbtTag>,
    /// Original raw value retained for inspection or roundtrip preservation.
    pub raw: Bytes,
}

#[derive(Debug, Clone, PartialEq)]
/// Typed global record preserving the original NBT payload.
pub struct ParsedGlobalData {
    /// Canonical key name used for storage.
    pub name: String,
    /// Classified global record kind.
    pub kind: GlobalRecordKind,
    /// Consecutive NBT roots stored in the value.
    pub roots: Vec<NbtTag>,
    /// Raw value bytes for roundtrip preservation.
    pub raw: Bytes,
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed block entity with its chunk and order in the consecutive NBT payload.
pub struct BlockEntityRecord {
    /// Chunk containing the block entity.
    pub chunk: ChunkPos,
    /// Zero-based index in the chunk's `BlockEntity` payload.
    pub index: usize,
    /// Parsed block entity.
    pub entity: ParsedBlockEntity,
}

#[derive(Debug, Clone, PartialEq)]
/// Bedrock 16x16 height map decoded from Data2D/Data3D.
pub struct HeightMap2d {
    /// Heights in `z * 16 + x` column order.
    pub values: Vec<i16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Legacy `Data2D` height map plus 2D biome ids.
pub struct Biome2d {
    /// Heights in `z * 16 + x` column order.
    pub height_map: Vec<i16>,
    /// Biome ids in `z * 16 + x` column order.
    pub biomes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// `Data3D` height map plus one or more biome storages.
pub struct Biome3d {
    /// Heights in `z * 16 + x` column order.
    pub height_map: Vec<i16>,
    /// 3D biome storages preserving palette/index/count decode mode output.
    pub storages: Vec<ParsedBiomeStorage>,
}

#[derive(Debug, Clone, PartialEq)]
/// Raw chunk records grouped by tag for format-complete tooling.
pub struct ChunkRecordSet {
    /// Chunk represented by this record set.
    pub pos: ChunkPos,
    /// Records keyed by Bedrock chunk tag; repeated tags are preserved in order.
    pub records: BTreeMap<ChunkRecordTag, Vec<ChunkRecord>>,
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed chunk model retaining both structured and unknown/raw records.
pub struct ChunkModel {
    /// Chunk represented by this model.
    pub pos: ChunkPos,
    /// Structured records decoded by this crate.
    pub records: Vec<ParsedChunkRecord>,
    /// Records not decoded by the selected parser mode.
    pub unknown_records: Vec<ChunkRecord>,
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed player record with decoded NBT roots.
pub struct ParsedPlayer {
    /// Decoded storage key for this record.
    pub key: BedrockDbKey,
    /// Player unique id decoded from NBT, when present.
    pub unique_id: Option<i64>,
    /// World position `[x, y, z]` decoded from player NBT, when present.
    pub position: Option<[f64; 3]>,
    /// Dimension id decoded from player NBT, when present.
    pub dimension_id: Option<i32>,
    /// Item stacks decoded from this payload.
    pub items: Vec<ItemStack>,
    /// Original or parsed Bedrock NBT payload.
    pub nbt: NbtTag,
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed entity summary and NBT payload.
pub struct ParsedEntity {
    /// Entity identifier decoded from NBT, when present.
    pub identifier: Option<String>,
    /// Entity definition identifiers decoded from NBT.
    pub definitions: Vec<String>,
    /// Entity unique id decoded from NBT, when present.
    pub unique_id: Option<i64>,
    /// World position `[x, y, z]` decoded from NBT, when present.
    pub position: Option<[f64; 3]>,
    /// Entity rotation `[yaw, pitch]` decoded from NBT, when present.
    pub rotation: Option<[f32; 2]>,
    /// Entity velocity `[x, y, z]` decoded from NBT, when present.
    pub motion: Option<[f32; 3]>,
    /// Item stacks decoded from this payload.
    pub items: Vec<ItemStack>,
    /// Original or parsed Bedrock NBT payload.
    pub nbt: NbtTag,
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed block entity summary and NBT payload.
pub struct ParsedBlockEntity {
    /// Identifier value decoded from storage or NBT.
    pub id: Option<String>,
    /// World block position `[x, y, z]` decoded from NBT, when present.
    pub position: Option<[i32; 3]>,
    /// Whether the block entity reports itself as movable.
    pub is_movable: Option<bool>,
    /// Custom display name decoded from NBT, when present.
    pub custom_name: Option<String>,
    /// Item stacks decoded from this payload.
    pub items: Vec<ItemStack>,
    /// Original or parsed Bedrock NBT payload.
    pub nbt: NbtTag,
}

#[derive(Debug, Clone, PartialEq)]
/// Item stack extracted from entity, block entity, or player NBT.
pub struct ItemStack {
    /// Named Bedrock value or identifier.
    pub name: Option<String>,
    /// Stack count decoded from NBT, when present.
    pub count: Option<i32>,
    /// Item damage value decoded from NBT, when present.
    pub damage: Option<i32>,
    /// Whether the item stack was marked as picked up.
    pub was_picked_up: Option<bool>,
    /// Whether this item stack contains a nested block payload.
    pub has_block: bool,
    /// Whether this item stack contains a nested tag payload.
    pub has_tag: bool,
    /// Original or parsed Bedrock NBT payload.
    pub nbt: NbtTag,
}

/// Parses world storage using the selected options and accumulates non-fatal diagnostics.
pub fn parse_world_storage(
    level_dat: LevelDatDocument,
    storage: &dyn WorldStorage,
    options: WorldParseOptions,
) -> WorldResult<ParsedWorld> {
    let actor_records = options
        .retention
        .retains_entries()
        .then(|| load_actor_records(storage, options))
        .transpose()?
        .unwrap_or_default();

    let mut report = WorldParseReport::default();
    let mut chunk_positions = BTreeSet::new();
    let mut parsed_entries = Vec::new();

    if options.retention.retains_entries() {
        storage.for_each_entry(StorageReadOptions::default(), &mut |raw_key, raw_value| {
            let key = record_world_key(raw_key, &mut report, &mut chunk_positions);
            if should_parse_key(&key, options.categories) {
                let value =
                    parse_entry_value(&key, raw_value, &actor_records, &mut report, options);
                parsed_entries.push(ParsedDbEntry {
                    key,
                    raw_key: Bytes::copy_from_slice(raw_key),
                    raw_value_len: raw_value.len(),
                    value,
                });
            }
            Ok(StorageVisitorControl::Continue)
        })?;
    } else {
        storage.for_each_key(StorageReadOptions::default(), &mut |raw_key| {
            record_world_key(raw_key, &mut report, &mut chunk_positions);
            Ok(StorageVisitorControl::Continue)
        })?;
    }

    report.chunk_count = chunk_positions.len();
    Ok(ParsedWorld {
        level_dat,
        entries: parsed_entries,
        report,
    })
}

fn record_world_key(
    raw_key: &[u8],
    report: &mut WorldParseReport,
    chunk_positions: &mut BTreeSet<String>,
) -> BedrockDbKey {
    report.entry_count = report.entry_count.saturating_add(1);
    let key = BedrockDbKey::decode(raw_key);
    *report.key_kinds.entry(key.summary_kind()).or_default() += 1;
    if let BedrockDbKey::Chunk(chunk_key) = &key {
        chunk_positions.insert(format!(
            "{}:{}:{}",
            chunk_key.pos.x,
            chunk_key.pos.z,
            chunk_key.pos.dimension.id()
        ));
    }
    key
}

#[must_use]
/// Parse chunk records.
pub fn parse_chunk_records(pos: ChunkPos, records: Vec<ChunkRecord>) -> ParsedChunkData {
    parse_chunk_records_ref(pos, &records)
}

#[must_use]
/// Parse chunk records with options.
pub fn parse_chunk_records_with_options(
    pos: ChunkPos,
    records: Vec<ChunkRecord>,
    options: WorldParseOptions,
) -> ParsedChunkData {
    parse_chunk_records_ref_with_options(pos, &records, options)
}

#[must_use]
/// Parse borrowed chunk records without consuming or cloning the source records.
pub fn parse_chunk_records_ref(pos: ChunkPos, records: &[ChunkRecord]) -> ParsedChunkData {
    parse_chunk_records_ref_with_options(pos, records, WorldParseOptions::full())
}

#[must_use]
/// Parse borrowed chunk records with options without consuming or cloning the source records.
pub fn parse_chunk_records_ref_with_options(
    pos: ChunkPos,
    records: &[ChunkRecord],
    options: WorldParseOptions,
) -> ParsedChunkData {
    let mut report = WorldParseReport::default();
    let parsed_records = records
        .iter()
        .map(|record| {
            *report
                .key_kinds
                .entry(format!("Chunk::{:?}", record.key.tag))
                .or_default() += 1;
            ParsedChunkRecord {
                key: record.key.clone(),
                value: parse_chunk_record_value(&record.key, &record.value, &mut report, options),
            }
        })
        .collect::<Vec<_>>();
    report.entry_count = parsed_records.len();
    report.chunk_count = usize::from(!parsed_records.is_empty());
    ParsedChunkData {
        pos,
        records: parsed_records,
        report,
    }
}

/// Parse global storage entries.
pub fn parse_global_storage_entries(
    storage: &dyn WorldStorage,
    options: WorldParseOptions,
) -> WorldResult<Vec<ParsedDbEntry>> {
    let actor_records = HashMap::new();
    let mut report = WorldParseReport::default();
    let mut entries = Vec::new();
    storage.for_each_entry(StorageReadOptions::default(), &mut |raw_key, raw_value| {
        let key = BedrockDbKey::decode(raw_key);
        if matches!(
            key,
            BedrockDbKey::Chunk(_)
                | BedrockDbKey::ActorPrefix { .. }
                | BedrockDbKey::ActorDigest { .. }
        ) {
            return Ok(StorageVisitorControl::Continue);
        }
        let value = parse_entry_value(&key, raw_value, &actor_records, &mut report, options);
        entries.push(ParsedDbEntry {
            key,
            raw_key: Bytes::copy_from_slice(raw_key),
            raw_value_len: raw_value.len(),
            value,
        });
        Ok(StorageVisitorControl::Continue)
    })?;
    Ok(entries)
}

fn load_actor_records(
    storage: &dyn WorldStorage,
    options: WorldParseOptions,
) -> WorldResult<HashMap<i64, Bytes>> {
    match options.actor_resolution {
        ActorResolution::None | ActorResolution::DigestOnly => Ok(HashMap::new()),
        ActorResolution::ResolveAll => {
            let mut actor_records = HashMap::new();
            storage.for_each_entry(StorageReadOptions::default(), &mut |key, value| {
                if let BedrockDbKey::ActorPrefix { actor_id } = BedrockDbKey::decode(key) {
                    actor_records.insert(actor_id, value.clone());
                }
                Ok(StorageVisitorControl::Continue)
            })?;
            Ok(actor_records)
        }
        ActorResolution::ResolveReferenced => {
            let mut actor_ids = BTreeSet::new();
            storage.for_each_entry(StorageReadOptions::default(), &mut |key, value| {
                if matches!(BedrockDbKey::decode(key), BedrockDbKey::ActorDigest { .. }) {
                    for actor_id_bytes in value.chunks_exact(8) {
                        let mut actor_id_array = [0_u8; 8];
                        actor_id_array.copy_from_slice(actor_id_bytes);
                        actor_ids.insert(i64::from_le_bytes(actor_id_array));
                    }
                }
                Ok(StorageVisitorControl::Continue)
            })?;
            let mut actor_records = HashMap::new();
            for actor_id in actor_ids {
                if let Some(value) = storage.get(&actor_prefix_key(actor_id))? {
                    actor_records.insert(actor_id, value);
                }
            }
            Ok(actor_records)
        }
    }
}

fn actor_prefix_key(actor_id: i64) -> Vec<u8> {
    let mut key = Vec::with_capacity("actorprefix".len() + 8);
    key.extend_from_slice(b"actorprefix");
    key.extend_from_slice(&actor_id.to_le_bytes());
    key
}

fn should_parse_key(key: &BedrockDbKey, categories: WorldParseCategories) -> bool {
    match key {
        BedrockDbKey::Chunk(_) => categories.chunks,
        BedrockDbKey::LocalPlayer | BedrockDbKey::RemotePlayer(_) => categories.players,
        BedrockDbKey::ActorPrefix { .. } | BedrockDbKey::ActorDigest { .. } => categories.actors,
        BedrockDbKey::Map(_) => categories.maps,
        BedrockDbKey::Village(_) => categories.villages,
        BedrockDbKey::Global(_) => categories.globals,
        BedrockDbKey::PlainString(name) if should_try_nbt_plain_key(name) => categories.globals,
        _ => false,
    }
}

fn parse_entry_value(
    key: &BedrockDbKey,
    value: &Bytes,
    actor_records: &HashMap<i64, Bytes>,
    report: &mut WorldParseReport,
    options: WorldParseOptions,
) -> ParsedDbValue {
    match key {
        BedrockDbKey::Chunk(chunk_key) => ParsedDbValue::Chunk(ParsedChunkRecord {
            key: chunk_key.clone(),
            value: parse_chunk_record_value(chunk_key, value, report, options),
        }),
        BedrockDbKey::LocalPlayer | BedrockDbKey::RemotePlayer(_) => {
            parse_player_value(key.clone(), value, report)
        }
        BedrockDbKey::ActorPrefix { .. } => parse_actor_value(value, report),
        BedrockDbKey::ActorDigest { pos } => {
            parse_actor_digest_value(*pos, value, actor_records, report, options)
        }
        BedrockDbKey::Map(id) => parse_map_value(id, value, report),
        BedrockDbKey::Village(village) => parse_village_value(village, value, report),
        BedrockDbKey::Global(kind) => parse_global_value(&kind.name(), value, report),
        BedrockDbKey::PlainString(name) if should_try_nbt_plain_key(name) => {
            parse_global_value(name, value, report)
        }
        BedrockDbKey::GameFlatWorldLayers
        | BedrockDbKey::Portals
        | BedrockDbKey::SchedulerWt
        | BedrockDbKey::StructureTemplate(_)
        | BedrockDbKey::TickingArea(_)
        | BedrockDbKey::PlainString(_)
        | BedrockDbKey::Unknown(_) => {
            report.raw_entry_count += 1;
            raw_db_value(value, options)
        }
    }
}

fn parse_chunk_record_value(
    chunk_key: &crate::ChunkKey,
    value: &Bytes,
    report: &mut WorldParseReport,
    options: WorldParseOptions,
) -> ParsedChunkRecordValue {
    match chunk_key.tag {
        ChunkRecordTag::SubChunkPrefix => {
            match parse_subchunk_with_mode(
                chunk_key.subchunk_y.unwrap_or_default(),
                value.clone(),
                options.subchunk_decode_mode,
            ) {
                Ok(subchunk) => {
                    report.subchunk_count += 1;
                    match &subchunk.format {
                        SubChunkFormat::Paletted { storages, .. } => {
                            report.subchunk_storage_count += storages.len();
                            report.palette_state_count += storages
                                .iter()
                                .map(|storage| storage.states.len())
                                .sum::<usize>();
                        }
                        SubChunkFormat::LegacySubChunk(_) => {
                            report.legacy_subchunk_count += 1;
                            report.subchunk_storage_count += 1;
                        }
                        SubChunkFormat::LegacyTerrain
                        | SubChunkFormat::FixedArrayV1
                        | SubChunkFormat::Raw { .. } => {}
                    }
                    ParsedChunkRecordValue::SubChunk(subchunk)
                }
                Err(error) => {
                    report.warnings.push(format!(
                        "subchunk {:?} kept raw: {error}",
                        chunk_key.subchunk_y
                    ));
                    report.raw_entry_count += 1;
                    ParsedChunkRecordValue::Raw(value.clone())
                }
            }
        }
        ChunkRecordTag::BlockEntity => parse_block_entities(value, report),
        ChunkRecordTag::Entity => parse_entities_chunk_record(value, report),
        ChunkRecordTag::PendingTicks => parse_pending_ticks(value, report),
        ChunkRecordTag::Version | ChunkRecordTag::VersionOld | ChunkRecordTag::LegacyVersion => {
            value.first().copied().map_or_else(
                || ParsedChunkRecordValue::Raw(value.clone()),
                ParsedChunkRecordValue::Version,
            )
        }
        ChunkRecordTag::FinalizedState => read_i32(value).map_or_else(
            || ParsedChunkRecordValue::Raw(value.clone()),
            ParsedChunkRecordValue::FinalizedState,
        ),
        ChunkRecordTag::Data3D => parse_biome_data(value, ChunkVersion::New, report),
        ChunkRecordTag::Data2D | ChunkRecordTag::Data2DLegacy => {
            parse_biome_data(value, ChunkVersion::Old, report)
        }
        ChunkRecordTag::HardcodedSpawners => parse_hardcoded_spawn_areas(value, report),
        ChunkRecordTag::LegacyTerrain => parse_legacy_terrain(value, report),
        ChunkRecordTag::BlockExtraData
        | ChunkRecordTag::BiomeState
        | ChunkRecordTag::ConversionData
        | ChunkRecordTag::BorderBlocks
        | ChunkRecordTag::RandomTicks
        | ChunkRecordTag::Checksums
        | ChunkRecordTag::GenerationSeed
        | ChunkRecordTag::MetaDataHash
        | ChunkRecordTag::GeneratedPreCavesAndCliffsBlending
        | ChunkRecordTag::BlendingBiomeHeight
        | ChunkRecordTag::BlendingData
        | ChunkRecordTag::ActorDigestVersion
        | ChunkRecordTag::Unknown(_) => {
            report.raw_entry_count += 1;
            raw_chunk_value(value, options)
        }
    }
}

fn parse_legacy_terrain(value: &Bytes, report: &mut WorldParseReport) -> ParsedChunkRecordValue {
    match LegacyTerrain::parse(value.clone()) {
        Ok(terrain) => {
            report.legacy_terrain_count += 1;
            ParsedChunkRecordValue::LegacyTerrain(terrain)
        }
        Err(error) => {
            report
                .warnings
                .push(format!("LegacyTerrain kept raw: {error}"));
            report.raw_entry_count += 1;
            ParsedChunkRecordValue::Raw(value.clone())
        }
    }
}

fn parse_actor_digest_value(
    pos: crate::ChunkPos,
    value: &Bytes,
    actor_records: &HashMap<i64, Bytes>,
    report: &mut WorldParseReport,
    options: WorldParseOptions,
) -> ParsedDbValue {
    report.actor_digest_count += 1;
    if !value.len().is_multiple_of(8) {
        report
            .warnings
            .push(format!("actor digest for {pos:?} kept raw: invalid length"));
        report.raw_entry_count += 1;
        return raw_db_value(value, options);
    }
    let mut actor_ids = Vec::with_capacity(value.len() / 8);
    let mut entities = Vec::new();
    let mut missing_actor_count = 0;
    for actor_id_bytes in value.chunks_exact(8) {
        let mut actor_id_array = [0_u8; 8];
        actor_id_array.copy_from_slice(actor_id_bytes);
        let actor_id = i64::from_le_bytes(actor_id_array);
        actor_ids.push(actor_id);
        let Some(actor_value) = actor_records.get(&actor_id) else {
            missing_actor_count += 1;
            continue;
        };
        report.actor_digest_hit_count += 1;
        match parse_actor_value(actor_value, report) {
            ParsedDbValue::ActorEntities(mut parsed_entities) => {
                entities.append(&mut parsed_entities);
            }
            ParsedDbValue::Raw(_)
            | ParsedDbValue::Chunk(_)
            | ParsedDbValue::Player(_)
            | ParsedDbValue::ActorDigest(_)
            | ParsedDbValue::MapData(_)
            | ParsedDbValue::VillageData(_)
            | ParsedDbValue::GlobalData(_)
            | ParsedDbValue::NbtRoots(_) => {}
        }
    }
    report.actor_digest_missing_count += missing_actor_count;
    ParsedDbValue::ActorDigest(ParsedActorDigest {
        pos,
        actor_ids,
        entities,
        missing_actor_count,
    })
}

fn raw_db_value(value: &Bytes, options: WorldParseOptions) -> ParsedDbValue {
    if options.retention.retains_raw() {
        ParsedDbValue::Raw(value.clone())
    } else {
        ParsedDbValue::Raw(Bytes::new())
    }
}

fn raw_chunk_value(value: &Bytes, options: WorldParseOptions) -> ParsedChunkRecordValue {
    if options.retention.retains_raw() {
        ParsedChunkRecordValue::Raw(value.clone())
    } else {
        ParsedChunkRecordValue::Raw(Bytes::new())
    }
}

fn parse_map_value(id: &str, value: &Bytes, report: &mut WorldParseReport) -> ParsedDbValue {
    report.map_record_count += 1;
    let roots = parse_consecutive_root_nbt(value).unwrap_or_else(|error| {
        report.warnings.push(format!("map_{id} kept raw: {error}"));
        Vec::new()
    });
    let known_fields = map_known_fields(&roots);
    let pixels = map_pixels(&roots);
    ParsedDbValue::MapData(ParsedMapData {
        id: id.to_string(),
        record_id: MapRecordId::unchecked(id.to_string()),
        roots,
        known_fields,
        pixels,
        raw: value.clone(),
    })
}

fn parse_village_value(
    key: &ParsedVillageKey,
    value: &Bytes,
    report: &mut WorldParseReport,
) -> ParsedDbValue {
    report.village_record_count += 1;
    let roots = parse_consecutive_root_nbt(value).unwrap_or_else(|error| {
        report
            .warnings
            .push(format!("{} kept raw: {error}", key.raw));
        Vec::new()
    });
    ParsedDbValue::VillageData(ParsedVillageData {
        key: key.clone(),
        roots,
        raw: value.clone(),
    })
}

fn parse_global_value(name: &str, value: &Bytes, report: &mut WorldParseReport) -> ParsedDbValue {
    report.global_record_count += 1;
    match parse_consecutive_root_nbt(value) {
        Ok(tags) => {
            report.other_nbt_root_count += tags.len();
            ParsedDbValue::GlobalData(ParsedGlobalData {
                name: name.to_string(),
                kind: GlobalRecordKind::from_key(name.as_bytes())
                    .unwrap_or_else(|| GlobalRecordKind::Other(name.to_string())),
                roots: tags,
                raw: value.clone(),
            })
        }
        Err(error) => {
            report.warnings.push(format!("{name} kept raw: {error}"));
            report.raw_entry_count += 1;
            ParsedDbValue::Raw(value.clone())
        }
    }
}

/// Parse map record.
pub fn parse_map_record(id: MapRecordId, value: Bytes) -> WorldResult<ParsedMapData> {
    let roots = parse_consecutive_root_nbt(&value)?;
    Ok(ParsedMapData {
        id: id.to_string(),
        record_id: id,
        known_fields: map_known_fields(&roots),
        pixels: map_pixels(&roots),
        roots,
        raw: value,
    })
}

/// Encode map record.
pub fn encode_map_record(record: &ParsedMapData) -> WorldResult<Bytes> {
    encode_consecutive_roots(&record.roots)
}

/// Parse global record.
pub fn parse_global_record(
    kind: GlobalRecordKind,
    name: String,
    value: Bytes,
) -> WorldResult<ParsedGlobalData> {
    let roots = parse_consecutive_root_nbt(&value)?;
    Ok(ParsedGlobalData {
        name,
        kind,
        roots,
        raw: value,
    })
}

/// Encode global record.
pub fn encode_global_record(record: &ParsedGlobalData) -> WorldResult<Bytes> {
    encode_consecutive_roots(&record.roots)
}

/// Parse actor digest ids.
pub fn parse_actor_digest_ids(value: &[u8]) -> WorldResult<Vec<ActorUid>> {
    if !value.len().is_multiple_of(8) {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "actor digest value length {} is not a multiple of 8",
            value.len()
        )));
    }
    let mut actor_ids = Vec::with_capacity(value.len() / 8);
    for actor_id_bytes in value.chunks_exact(8) {
        let mut actor_id_array = [0_u8; 8];
        actor_id_array.copy_from_slice(actor_id_bytes);
        actor_ids.push(ActorUid(i64::from_le_bytes(actor_id_array)));
    }
    Ok(actor_ids)
}

/// Encode actor digest ids.
pub fn encode_actor_digest_ids(actor_ids: &[ActorUid]) -> Bytes {
    let mut bytes = Vec::with_capacity(actor_ids.len() * 8);
    for actor_id in actor_ids {
        bytes.extend_from_slice(&actor_id.0.to_le_bytes());
    }
    Bytes::from(bytes)
}

/// Parse hardcoded spawn area records.
pub fn parse_hardcoded_spawn_area_records(
    value: &[u8],
) -> WorldResult<Vec<ParsedHardcodedSpawnArea>> {
    read_hardcoded_spawn_areas(value).map_err(BedrockWorldError::Validation)
}

/// Encode hardcoded spawn area records.
pub fn encode_hardcoded_spawn_area_records(
    areas: &[ParsedHardcodedSpawnArea],
) -> WorldResult<Bytes> {
    let count = i32::try_from(areas.len())
        .map_err(|_| BedrockWorldError::Validation("too many hardcoded spawn areas".to_string()))?;
    let mut bytes = Vec::with_capacity(4 + areas.len() * 25);
    bytes.extend_from_slice(&count.to_le_bytes());
    for area in areas {
        area.validate()?;
        for value in area.min {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for value in area.max {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.push(area.kind.byte());
    }
    Ok(Bytes::from(bytes))
}

/// Encode consecutive roots.
pub fn encode_consecutive_roots(roots: &[NbtTag]) -> WorldResult<Bytes> {
    if roots.is_empty() {
        return Err(BedrockWorldError::Validation(
            "record must contain at least one root NBT compound".to_string(),
        ));
    }
    let mut bytes = Vec::new();
    for root in roots {
        bytes.extend_from_slice(&serialize_root_nbt(root)?);
    }
    Ok(Bytes::from(bytes))
}

fn parse_player_value(
    key: BedrockDbKey,
    value: &Bytes,
    report: &mut WorldParseReport,
) -> ParsedDbValue {
    match parse_root_nbt(value) {
        Ok(nbt) => {
            let items = collect_item_stacks(&nbt);
            report.player_count += 1;
            report.item_count += items.len();
            let root = compound(&nbt);
            ParsedDbValue::Player(ParsedPlayer {
                key,
                unique_id: root.and_then(|root| long_field(root, "UniqueID")),
                position: root.and_then(|root| vec3_f64_field(root, "Pos")),
                dimension_id: root.and_then(|root| int_field(root, "DimensionId")),
                items,
                nbt,
            })
        }
        Err(error) => {
            report
                .parse_errors
                .push(format!("player NBT parse failed: {error}"));
            report.raw_entry_count += 1;
            ParsedDbValue::Raw(value.clone())
        }
    }
}

fn map_known_fields(roots: &[NbtTag]) -> MapKnownFields {
    let Some(root) = roots.first().and_then(compound) else {
        return MapKnownFields::default();
    };
    MapKnownFields {
        dimension: int_field_any(
            root,
            &["dimension", "dimensionId", "Dimension", "DimensionId"],
        ),
        center_x: int_field_any(root, &["xCenter", "centerX", "CenterX"]),
        center_z: int_field_any(root, &["zCenter", "centerZ", "CenterZ"]),
        scale: int_field_any(root, &["scale", "Scale"]),
        width: int_field_any(root, &["width", "Width"]),
        height: int_field_any(root, &["height", "Height"]),
        locked: bool_field_any(root, &["locked", "Locked"]),
    }
}

fn map_pixels(roots: &[NbtTag]) -> Option<MapPixels> {
    let root = roots.first().and_then(compound)?;
    let colors = byte_array_field_any(root, &["colors", "Colors", "pixels", "Pixels"])?;
    let width = int_field_any(root, &["width", "Width"])
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(128);
    let height = int_field_any(root, &["height", "Height"])
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_else(|| {
            u32::try_from(colors.len())
                .ok()
                .and_then(|len| len.checked_div(width))
                .unwrap_or(128)
        });
    let expected_len = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    (colors.len() == expected_len).then_some(MapPixels {
        width,
        height,
        colors: colors.iter().map(|value| *value as u8).collect(),
    })
}

fn int_field_any(root: &IndexMap<String, NbtTag>, names: &[&str]) -> Option<i32> {
    names.iter().find_map(|name| int_field(root, name))
}

fn bool_field_any(root: &IndexMap<String, NbtTag>, names: &[&str]) -> Option<bool> {
    names.iter().find_map(|name| bool_field(root, name))
}

fn byte_array_field_any<'a>(
    root: &'a IndexMap<String, NbtTag>,
    names: &[&str],
) -> Option<&'a [i8]> {
    for name in names {
        if let Some(NbtTag::ByteArray(values)) = root.get(*name) {
            return Some(values);
        }
    }
    None
}

pub(crate) fn parse_actor_value(value: &Bytes, report: &mut WorldParseReport) -> ParsedDbValue {
    match parse_consecutive_root_nbt(value) {
        Ok(tags) => {
            let entities = tags
                .into_iter()
                .map(|tag| parse_entity_from_nbt(tag, report))
                .collect::<Vec<_>>();
            report.entity_count += entities.len();
            ParsedDbValue::ActorEntities(entities)
        }
        Err(error) => {
            report
                .warnings
                .push(format!("actorprefix kept raw: {error}"));
            report.raw_entry_count += 1;
            ParsedDbValue::Raw(value.clone())
        }
    }
}

fn parse_biome_data(
    value: &Bytes,
    version: ChunkVersion,
    report: &mut WorldParseReport,
) -> ParsedChunkRecordValue {
    let result = match version {
        ChunkVersion::Old => parse_legacy_data2d(value),
        ChunkVersion::New => parse_data3d(value),
    };
    match result {
        Ok(data) => {
            report.biome_record_count += 1;
            report.biome_layer_count += data.storages.len();
            ParsedChunkRecordValue::BiomeData(data)
        }
        Err(error) => {
            report
                .warnings
                .push(format!("biome data kept raw: {error}"));
            report.raw_entry_count += 1;
            ParsedChunkRecordValue::Raw(value.clone())
        }
    }
}

pub(crate) fn parse_legacy_data2d(value: &[u8]) -> Result<ParsedBiomeData, String> {
    if value.len() < 768 {
        return Err(format!("Data2D is too short: {}", value.len()));
    }
    let height_map = read_height_map(&value[..512])?;
    let indices = value[512..768]
        .iter()
        .map(|value| u16::from(*value))
        .collect::<Vec<_>>();
    let palette = (0..=255).collect::<Vec<_>>();
    let mut counts = vec![0_u16; palette.len()];
    for index in &indices {
        if let Some(count) = counts.get_mut(usize::from(*index)) {
            *count = count.saturating_add(1);
        }
    }
    Ok(ParsedBiomeData {
        version: ChunkVersion::Old,
        height_map,
        storages: vec![ParsedBiomeStorage {
            y: None,
            palette,
            indices: Some(indices),
            counts,
        }],
    })
}

pub(crate) fn parse_data3d(value: &[u8]) -> Result<ParsedBiomeData, String> {
    if value.len() < 512 {
        return Err(format!("Data3D is too short: {}", value.len()));
    }
    let height_map = read_height_map(&value[..512])?;
    let mut offset = 512;
    let mut storages = Vec::new();
    let mut y = -64;
    while offset < value.len() {
        let (storage, consumed) = parse_subchunk_biomes(&value[offset..], y)?;
        if consumed == 0 {
            return Err("Data3D biome parser did not advance".to_string());
        }
        offset += consumed;
        y += 16;
        storages.push(storage);
    }
    Ok(ParsedBiomeData {
        version: ChunkVersion::New,
        height_map,
        storages,
    })
}

fn parse_subchunk_biomes(
    value: &[u8],
    start_y: i32,
) -> Result<(ParsedBiomeStorage, usize), String> {
    let Some(header) = value.first().copied() else {
        return Err("missing biome storage header".to_string());
    };
    if header == 0xff {
        return Ok((
            ParsedBiomeStorage {
                y: Some(start_y),
                palette: vec![u32::MAX],
                indices: None,
                counts: vec![4096],
            },
            1,
        ));
    }
    let bits_per_biome = header >> 1;
    let mut offset = 1;
    let indices = if bits_per_biome == 0 {
        vec![0_u16; 4096]
    } else {
        let word_count = packed_word_count(bits_per_biome);
        let words_byte_len = word_count
            .checked_mul(4)
            .ok_or_else(|| "biome palette word count overflowed".to_string())?;
        let words = value
            .get(offset..offset + words_byte_len)
            .ok_or_else(|| "biome palette words are truncated".to_string())?;
        offset += words_byte_len;
        unpack_indices(words, bits_per_biome)?
    };
    let palette_len = if bits_per_biome == 0 {
        1
    } else {
        let len = read_i32_le(value, offset)?;
        offset += 4;
        usize::try_from(len).map_err(|_| format!("invalid biome palette length: {len}"))?
    };
    if palette_len > MAX_BIOME_PALETTE_LEN {
        return Err(format!(
            "biome palette length {palette_len} exceeds maximum {MAX_BIOME_PALETTE_LEN}"
        ));
    }
    let mut palette = Vec::with_capacity(palette_len);
    for _ in 0..palette_len {
        let id = read_i32_le(value, offset)?;
        offset += 4;
        palette.push(u32::try_from(id).unwrap_or(u32::MAX));
    }
    let mut counts = vec![0_u16; palette.len()];
    for index in &indices {
        if let Some(count) = counts.get_mut(usize::from(*index)) {
            *count = count.saturating_add(1);
        }
    }
    Ok((
        ParsedBiomeStorage {
            y: Some(start_y),
            palette,
            indices: Some(indices),
            counts,
        },
        offset,
    ))
}

fn read_height_map(value: &[u8]) -> Result<Vec<i16>, String> {
    if value.len() != 512 {
        return Err(format!("height map must be 512 bytes, got {}", value.len()));
    }
    Ok(value
        .chunks_exact(2)
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
        .collect())
}

fn encode_biome_storage(storage: &ParsedBiomeStorage) -> WorldResult<Vec<u8>> {
    if storage.palette.is_empty() {
        return Err(BedrockWorldError::Validation(
            "biome storage palette cannot be empty".to_string(),
        ));
    }
    if storage.palette.as_slice() == [u32::MAX] && storage.indices.is_none() {
        return Ok(vec![0xff]);
    }
    if storage.palette.len() == 1
        && storage
            .indices
            .as_ref()
            .is_none_or(|indices| indices.len() == 4096 && indices.iter().all(|index| *index == 0))
    {
        let mut bytes = Vec::with_capacity(5);
        bytes.push(0);
        let id = i32::try_from(storage.palette[0])
            .map_err(|_| BedrockWorldError::Validation("biome id does not fit i32".to_string()))?;
        bytes.extend_from_slice(&id.to_le_bytes());
        return Ok(bytes);
    }
    let indices = storage.indices.as_ref().ok_or_else(|| {
        BedrockWorldError::Validation("non-uniform biome storage requires indices".to_string())
    })?;
    if indices.len() != 4096 {
        return Err(BedrockWorldError::Validation(format!(
            "biome storage requires 4096 indices, got {}",
            indices.len()
        )));
    }
    let bits = bits_per_palette_index(storage.palette.len())?;
    let mut bytes = Vec::new();
    bytes.push(bits << 1);
    bytes.extend_from_slice(&pack_indices(indices, bits)?);
    let palette_len = i32::try_from(storage.palette.len()).map_err(|_| {
        BedrockWorldError::Validation("biome palette length does not fit i32".to_string())
    })?;
    bytes.extend_from_slice(&palette_len.to_le_bytes());
    for id in &storage.palette {
        let id = i32::try_from(*id)
            .map_err(|_| BedrockWorldError::Validation("biome id does not fit i32".to_string()))?;
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    Ok(bytes)
}

fn packed_word_count(bits_per_value: u8) -> usize {
    if bits_per_value == 0 {
        return 0;
    }
    let values_per_word = usize::from(32 / bits_per_value);
    4096_usize.div_ceil(values_per_word)
}

fn bits_per_palette_index(palette_len: usize) -> WorldResult<u8> {
    let max_index = palette_len.saturating_sub(1);
    for bits in [1_u8, 2, 3, 4, 5, 6, 8, 16] {
        if max_index < (1_usize << bits) {
            return Ok(bits);
        }
    }
    Err(BedrockWorldError::Validation(format!(
        "biome palette length {palette_len} exceeds encodable range"
    )))
}

fn pack_indices(indices: &[u16], bits_per_value: u8) -> WorldResult<Vec<u8>> {
    if !matches!(bits_per_value, 1 | 2 | 3 | 4 | 5 | 6 | 8 | 16) {
        return Err(BedrockWorldError::Validation(format!(
            "unsupported biome bits-per-value: {bits_per_value}"
        )));
    }
    let values_per_word = usize::from(32 / bits_per_value);
    let mask = (1_u32 << bits_per_value) - 1;
    let mut bytes = Vec::with_capacity(packed_word_count(bits_per_value) * 4);
    for chunk in indices.chunks(values_per_word) {
        let mut word = 0_u32;
        for (offset, value) in chunk.iter().enumerate() {
            let value = u32::from(*value);
            if value > mask {
                return Err(BedrockWorldError::Validation(format!(
                    "biome index {value} exceeds {bits_per_value}-bit palette"
                )));
            }
            word |= value << (offset * usize::from(bits_per_value));
        }
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    Ok(bytes)
}

fn unpack_indices(words_bytes: &[u8], bits_per_value: u8) -> Result<Vec<u16>, String> {
    if bits_per_value == 0 {
        return Ok(vec![0; 4096]);
    }
    if !matches!(bits_per_value, 1 | 2 | 3 | 4 | 5 | 6 | 8 | 16) {
        return Err(format!(
            "unsupported biome bits-per-value: {bits_per_value}"
        ));
    }
    let values_per_word = usize::from(32 / bits_per_value);
    let mask = (1_u32 << bits_per_value) - 1;
    let mut indices = Vec::with_capacity(4096);
    for word_bytes in words_bytes.chunks_exact(4) {
        let word = u32::from_le_bytes([word_bytes[0], word_bytes[1], word_bytes[2], word_bytes[3]]);
        for item_index in 0..values_per_word {
            if indices.len() == 4096 {
                break;
            }
            indices.push(((word >> (item_index * usize::from(bits_per_value))) & mask) as u16);
        }
    }
    if indices.len() != 4096 {
        return Err(format!("decoded {} biome indices", indices.len()));
    }
    Ok(indices)
}

fn read_i32_le(value: &[u8], offset: usize) -> Result<i32, String> {
    let bytes = value
        .get(offset..offset + 4)
        .ok_or_else(|| "i32 field is truncated".to_string())?;
    Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn parse_hardcoded_spawn_areas(
    value: &Bytes,
    report: &mut WorldParseReport,
) -> ParsedChunkRecordValue {
    match read_hardcoded_spawn_areas(value) {
        Ok(areas) => {
            report.hardcoded_spawn_area_count += areas.len();
            ParsedChunkRecordValue::HardcodedSpawnAreas(areas)
        }
        Err(error) => {
            report
                .warnings
                .push(format!("hardcoded spawn areas kept raw: {error}"));
            report.raw_entry_count += 1;
            ParsedChunkRecordValue::Raw(value.clone())
        }
    }
}

fn read_hardcoded_spawn_areas(value: &[u8]) -> Result<Vec<ParsedHardcodedSpawnArea>, String> {
    let count = usize::try_from(read_i32_le(value, 0)?)
        .map_err(|_| "hardcoded spawn area count cannot be negative".to_string())?;
    let expected_len = 4 + count * 25;
    if value.len() != expected_len {
        return Err(format!(
            "expected {expected_len} bytes, got {}",
            value.len()
        ));
    }
    let mut areas = Vec::with_capacity(count);
    for index in 0..count {
        let offset = 4 + index * 25;
        areas.push(ParsedHardcodedSpawnArea {
            kind: match value[offset + 24] {
                1 => HardcodedSpawnAreaKind::NetherFortress,
                2 => HardcodedSpawnAreaKind::SwampHut,
                3 => HardcodedSpawnAreaKind::OceanMonument,
                5 => HardcodedSpawnAreaKind::PillagerOutpost,
                value => HardcodedSpawnAreaKind::Unknown(value),
            },
            min: [
                read_i32_le(value, offset)?,
                read_i32_le(value, offset + 4)?,
                read_i32_le(value, offset + 8)?,
            ],
            max: [
                read_i32_le(value, offset + 12)?,
                read_i32_le(value, offset + 16)?,
                read_i32_le(value, offset + 20)?,
            ],
        });
    }
    Ok(areas)
}

pub(crate) fn parse_block_entities(
    value: &Bytes,
    report: &mut WorldParseReport,
) -> ParsedChunkRecordValue {
    match parse_consecutive_root_nbt(value) {
        Ok(tags) => {
            let block_entities = tags
                .into_iter()
                .map(|tag| parse_block_entity_from_nbt(tag, report))
                .collect::<Vec<_>>();
            report.block_entity_count += block_entities.len();
            ParsedChunkRecordValue::BlockEntities(block_entities)
        }
        Err(error) => {
            report
                .warnings
                .push(format!("block entities kept raw: {error}"));
            report.raw_entry_count += 1;
            ParsedChunkRecordValue::Raw(value.clone())
        }
    }
}

fn parse_entities_chunk_record(
    value: &Bytes,
    report: &mut WorldParseReport,
) -> ParsedChunkRecordValue {
    match parse_consecutive_root_nbt(value) {
        Ok(tags) => {
            let entities = tags
                .into_iter()
                .map(|tag| parse_entity_from_nbt(tag, report))
                .collect::<Vec<_>>();
            report.entity_count += entities.len();
            ParsedChunkRecordValue::Entities(entities)
        }
        Err(error) => {
            report.warnings.push(format!("entities kept raw: {error}"));
            report.raw_entry_count += 1;
            ParsedChunkRecordValue::Raw(value.clone())
        }
    }
}

pub(crate) fn parse_entities_from_value(
    value: &Bytes,
    report: &mut WorldParseReport,
) -> Vec<ParsedEntity> {
    match parse_actor_value(value, report) {
        ParsedDbValue::ActorEntities(entities) => entities,
        _ => Vec::new(),
    }
}

pub(crate) fn parse_block_entities_from_value(
    value: &Bytes,
    report: &mut WorldParseReport,
) -> Vec<ParsedBlockEntity> {
    match parse_block_entities(value, report) {
        ParsedChunkRecordValue::BlockEntities(block_entities) => block_entities,
        _ => Vec::new(),
    }
}

fn parse_pending_ticks(value: &Bytes, report: &mut WorldParseReport) -> ParsedChunkRecordValue {
    match parse_consecutive_root_nbt(value) {
        Ok(tags) => ParsedChunkRecordValue::PendingTicks(tags),
        Err(error) => {
            report
                .warnings
                .push(format!("pending ticks kept raw: {error}"));
            report.raw_entry_count += 1;
            ParsedChunkRecordValue::Raw(value.clone())
        }
    }
}

fn parse_entity_from_nbt(nbt: NbtTag, report: &mut WorldParseReport) -> ParsedEntity {
    let items = collect_item_stacks(&nbt);
    report.item_count += items.len();
    let root = compound(&nbt);
    ParsedEntity {
        identifier: root.and_then(entity_identifier),
        definitions: root.map_or_else(Vec::new, entity_definitions),
        unique_id: root.and_then(|root| long_field(root, "UniqueID")),
        position: root.and_then(|root| vec3_f64_field(root, "Pos")),
        rotation: root.and_then(|root| vec2_f32_field(root, "Rotation")),
        motion: root.and_then(|root| vec3_f32_field(root, "Motion")),
        items,
        nbt,
    }
}

fn parse_block_entity_from_nbt(nbt: NbtTag, report: &mut WorldParseReport) -> ParsedBlockEntity {
    let items = collect_item_stacks(&nbt);
    report.item_count += items.len();
    let root = compound(&nbt);
    ParsedBlockEntity {
        id: root
            .and_then(|root| string_field(root, "id"))
            .map(ToString::to_string),
        position: root.and_then(|root| {
            Some([
                int_field(root, "x")?,
                int_field(root, "y")?,
                int_field(root, "z")?,
            ])
        }),
        is_movable: root.and_then(|root| bool_field(root, "isMovable")),
        custom_name: root
            .and_then(|root| string_field(root, "CustomName"))
            .map(ToString::to_string),
        items,
        nbt,
    }
}

pub(crate) fn collect_item_stacks(tag: &NbtTag) -> Vec<ItemStack> {
    let mut items = Vec::new();
    collect_item_stacks_inner(tag, &mut items);
    items
}

fn collect_item_stacks_inner(tag: &NbtTag, items: &mut Vec<ItemStack>) {
    match tag {
        NbtTag::Compound(root) => {
            if looks_like_item_stack(root) {
                items.push(ItemStack {
                    name: string_field(root, "Name")
                        .or_else(|| string_field(root, "name"))
                        .map(ToString::to_string),
                    count: int_field(root, "Count"),
                    damage: int_field(root, "Damage").or_else(|| int_field(root, "Aux")),
                    was_picked_up: bool_field(root, "WasPickedUp"),
                    has_block: root.contains_key("Block"),
                    has_tag: root.contains_key("tag"),
                    nbt: tag.clone(),
                });
            }
            for value in root.values() {
                collect_item_stacks_inner(value, items);
            }
        }
        NbtTag::List(values) => {
            for value in values {
                collect_item_stacks_inner(value, items);
            }
        }
        _ => {}
    }
}

fn looks_like_item_stack(root: &IndexMap<String, NbtTag>) -> bool {
    (root.contains_key("Name") || root.contains_key("name")) && root.contains_key("Count")
}

fn should_try_nbt_plain_key(name: &str) -> bool {
    matches!(
        name,
        "AutonomousEntities"
            | "autonomousentities"
            | "BiomeData"
            | "LevelChunkMetaDataDictionary"
            | "LocalPlayer"
            | "Nether"
            | "Overworld"
            | "TheEnd"
            | "WorldClocks"
            | "mobevents"
            | "scoreboard"
    )
}

fn entity_identifier(root: &IndexMap<String, NbtTag>) -> Option<String> {
    string_field(root, "identifier")
        .or_else(|| string_field(root, "Identifier"))
        .or_else(|| string_field(root, "id"))
        .map(ToString::to_string)
}

fn entity_definitions(root: &IndexMap<String, NbtTag>) -> Vec<String> {
    match root.get("definitions").or_else(|| root.get("Definitions")) {
        Some(NbtTag::List(values)) => values
            .iter()
            .filter_map(|value| match value {
                NbtTag::String(value) => Some(value.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn compound(tag: &NbtTag) -> Option<&IndexMap<String, NbtTag>> {
    match tag {
        NbtTag::Compound(root) => Some(root),
        _ => None,
    }
}

fn string_field<'a>(root: &'a IndexMap<String, NbtTag>, key: &str) -> Option<&'a str> {
    match root.get(key) {
        Some(NbtTag::String(value)) => Some(value.as_str()),
        _ => None,
    }
}

fn bool_field(root: &IndexMap<String, NbtTag>, key: &str) -> Option<bool> {
    match root.get(key) {
        Some(NbtTag::Byte(value)) => Some(*value != 0),
        Some(NbtTag::Short(value)) => Some(*value != 0),
        Some(NbtTag::Int(value)) => Some(*value != 0),
        _ => None,
    }
}

fn int_field(root: &IndexMap<String, NbtTag>, key: &str) -> Option<i32> {
    match root.get(key) {
        Some(NbtTag::Byte(value)) => Some(i32::from(*value)),
        Some(NbtTag::Short(value)) => Some(i32::from(*value)),
        Some(NbtTag::Int(value)) => Some(*value),
        Some(NbtTag::Long(value)) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn long_field(root: &IndexMap<String, NbtTag>, key: &str) -> Option<i64> {
    match root.get(key) {
        Some(NbtTag::Byte(value)) => Some(i64::from(*value)),
        Some(NbtTag::Short(value)) => Some(i64::from(*value)),
        Some(NbtTag::Int(value)) => Some(i64::from(*value)),
        Some(NbtTag::Long(value)) => Some(*value),
        _ => None,
    }
}

fn f64_value(tag: &NbtTag) -> Option<f64> {
    match tag {
        NbtTag::Float(value) => Some(f64::from(*value)),
        NbtTag::Double(value) => Some(*value),
        NbtTag::Int(value) => Some(f64::from(*value)),
        NbtTag::Long(value) => Some(*value as f64),
        _ => None,
    }
}

fn f32_value(tag: &NbtTag) -> Option<f32> {
    match tag {
        NbtTag::Float(value) => Some(*value),
        NbtTag::Double(value) => Some(*value as f32),
        NbtTag::Int(value) => Some(*value as f32),
        _ => None,
    }
}

fn vec3_f64_field(root: &IndexMap<String, NbtTag>, key: &str) -> Option<[f64; 3]> {
    let Some(NbtTag::List(values)) = root.get(key) else {
        return None;
    };
    Some([
        f64_value(values.first()?)?,
        f64_value(values.get(1)?)?,
        f64_value(values.get(2)?)?,
    ])
}

fn vec3_f32_field(root: &IndexMap<String, NbtTag>, key: &str) -> Option<[f32; 3]> {
    let Some(NbtTag::List(values)) = root.get(key) else {
        return None;
    };
    Some([
        f32_value(values.first()?)?,
        f32_value(values.get(1)?)?,
        f32_value(values.get(2)?)?,
    ])
}

fn vec2_f32_field(root: &IndexMap<String, NbtTag>, key: &str) -> Option<[f32; 2]> {
    let Some(NbtTag::List(values)) = root.get(key) else {
        return None;
    };
    Some([f32_value(values.first()?)?, f32_value(values.get(1)?)?])
}

fn read_i32(value: &[u8]) -> Option<i32> {
    let bytes: [u8; 4] = value.get(..4)?.try_into().ok()?;
    Some(i32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::serialize_root_nbt;
    use crate::storage::{
        MemoryStorage, StorageBatch, StorageReadOptions, StorageScanOutcome, WorldStorage,
    };

    struct KeyOnlySummaryStorage {
        key: Bytes,
    }

    impl WorldStorage for KeyOnlySummaryStorage {
        fn get(&self, _key: &[u8]) -> WorldResult<Option<Bytes>> {
            Ok(None)
        }

        fn put(&self, _key: &[u8], _value: &[u8]) -> WorldResult<()> {
            Err(BedrockWorldError::ReadOnly)
        }

        fn delete(&self, _key: &[u8]) -> WorldResult<()> {
            Err(BedrockWorldError::ReadOnly)
        }

        fn for_each_key(
            &self,
            _options: StorageReadOptions,
            visitor: &mut (dyn FnMut(&[u8]) -> WorldResult<StorageVisitorControl> + Send),
        ) -> WorldResult<StorageScanOutcome> {
            let _ = visitor(&self.key)?;
            Ok(StorageScanOutcome::empty())
        }

        fn for_each_prefix(
            &self,
            _prefix: &[u8],
            _options: StorageReadOptions,
            _visitor: &mut (dyn FnMut(&[u8], &Bytes) -> WorldResult<StorageVisitorControl> + Send),
        ) -> WorldResult<StorageScanOutcome> {
            Err(BedrockWorldError::Validation(
                "summary parsing requested values".to_string(),
            ))
        }

        fn write_batch(&self, _batch: &StorageBatch) -> WorldResult<()> {
            Err(BedrockWorldError::ReadOnly)
        }

        fn flush(&self) -> WorldResult<()> {
            Ok(())
        }
    }

    #[test]
    fn item_stack_extracts_common_fields() {
        let item = NbtTag::Compound(IndexMap::from([
            (
                "Name".to_string(),
                NbtTag::String("minecraft:stone".to_string()),
            ),
            ("Count".to_string(), NbtTag::Byte(5)),
            ("Damage".to_string(), NbtTag::Short(1)),
            ("WasPickedUp".to_string(), NbtTag::Byte(1)),
        ]));

        let items = collect_item_stacks(&item);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name.as_deref(), Some("minecraft:stone"));
        assert_eq!(items[0].count, Some(5));
        assert_eq!(items[0].damage, Some(1));
        assert_eq!(items[0].was_picked_up, Some(true));
    }

    #[test]
    fn entity_extracts_identifier_position_and_items() {
        let entity = NbtTag::Compound(IndexMap::from([
            (
                "identifier".to_string(),
                NbtTag::String("minecraft:pig".to_string()),
            ),
            (
                "Pos".to_string(),
                NbtTag::List(vec![
                    NbtTag::Float(1.0),
                    NbtTag::Float(2.0),
                    NbtTag::Float(3.0),
                ]),
            ),
            (
                "Inventory".to_string(),
                NbtTag::List(vec![NbtTag::Compound(IndexMap::from([
                    (
                        "Name".to_string(),
                        NbtTag::String("minecraft:dirt".to_string()),
                    ),
                    ("Count".to_string(), NbtTag::Byte(1)),
                ]))]),
            ),
        ]));
        let bytes = Bytes::from(serialize_root_nbt(&entity).expect("serialize"));
        let mut report = WorldParseReport::default();

        let value = parse_actor_value(&bytes, &mut report);

        let ParsedDbValue::ActorEntities(entities) = value else {
            panic!("expected entity value");
        };
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].identifier.as_deref(), Some("minecraft:pig"));
        assert_eq!(entities[0].position, Some([1.0, 2.0, 3.0]));
        assert_eq!(entities[0].items.len(), 1);
    }

    #[test]
    fn block_entity_extracts_container_items() {
        let block_entity = NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::String("Chest".to_string())),
            ("x".to_string(), NbtTag::Int(1)),
            ("y".to_string(), NbtTag::Int(2)),
            ("z".to_string(), NbtTag::Int(3)),
            ("isMovable".to_string(), NbtTag::Byte(1)),
            (
                "Items".to_string(),
                NbtTag::List(vec![NbtTag::Compound(IndexMap::from([
                    (
                        "Name".to_string(),
                        NbtTag::String("minecraft:apple".to_string()),
                    ),
                    ("Count".to_string(), NbtTag::Byte(2)),
                ]))]),
            ),
        ]));
        let bytes = Bytes::from(serialize_root_nbt(&block_entity).expect("serialize"));
        let mut report = WorldParseReport::default();

        let value = parse_block_entities(&bytes, &mut report);

        let ParsedChunkRecordValue::BlockEntities(block_entities) = value else {
            panic!("expected block entities");
        };
        assert_eq!(block_entities.len(), 1);
        assert_eq!(block_entities[0].id.as_deref(), Some("Chest"));
        assert_eq!(block_entities[0].position, Some([1, 2, 3]));
        assert_eq!(block_entities[0].items.len(), 1);
    }

    #[test]
    fn biome_lookup_uses_xz_plane_storage_order() {
        let mut indices = vec![0_u16; 4096];
        indices[crate::block_storage_index(1, 2, 3)] = 2;
        let storage = ParsedBiomeStorage {
            y: Some(0),
            palette: vec![10, 20, 30],
            indices: Some(indices),
            counts: vec![4095, 0, 1],
        };

        assert_eq!(storage.biome_id_at(1, 2, 3), Some(30));
        assert_eq!(storage.biome_id_at(1, 3, 3), Some(10));
    }

    #[test]
    fn hsa_records_roundtrip_reference_binary_layout() {
        let areas = vec![ParsedHardcodedSpawnArea {
            kind: HardcodedSpawnAreaKind::PillagerOutpost,
            min: [1, 2, 3],
            max: [4, 5, 6],
        }];

        let bytes = encode_hardcoded_spawn_area_records(&areas).expect("encode hsa");
        let decoded = parse_hardcoded_spawn_area_records(&bytes).expect("decode hsa");

        assert_eq!(bytes.len(), 29);
        assert_eq!(decoded, areas);
    }

    #[test]
    fn biome2d_and_biome3d_codecs_roundtrip() {
        let height_map = (0..256).map(|value| value as i16).collect::<Vec<_>>();
        let biomes = (0..256).map(|value| value as u8).collect::<Vec<_>>();
        let data2d = Biome2d::new(height_map.clone(), biomes.clone()).expect("2d");
        assert_eq!(
            Biome2d::parse(&data2d.encode().expect("encode")).expect("parse"),
            data2d
        );

        let storage = ParsedBiomeStorage {
            y: Some(-64),
            palette: vec![1, 2],
            indices: Some(vec![0; 4096]),
            counts: vec![4096, 0],
        };
        let data3d = Biome3d::new(height_map, vec![storage]).expect("3d");
        assert_eq!(
            Biome3d::parse(&data3d.encode().expect("encode")).expect("parse"),
            data3d
        );

        let inherited = Biome3d::new(
            vec![0; 256],
            vec![ParsedBiomeStorage {
                y: Some(-64),
                palette: vec![u32::MAX],
                indices: None,
                counts: vec![4096],
            }],
        )
        .expect("inherited biome storage");
        assert_eq!(
            Biome3d::parse(&inherited.encode().expect("encode inherited"))
                .expect("parse inherited"),
            inherited
        );
    }

    #[test]
    fn map_and_global_records_extract_typed_fields() {
        let map_root = NbtTag::Compound(IndexMap::from([
            ("dimension".to_string(), NbtTag::Int(0)),
            ("xCenter".to_string(), NbtTag::Int(10)),
            ("zCenter".to_string(), NbtTag::Int(-20)),
            ("scale".to_string(), NbtTag::Byte(2)),
            ("width".to_string(), NbtTag::Int(2)),
            ("height".to_string(), NbtTag::Int(2)),
            ("colors".to_string(), NbtTag::ByteArray(vec![1, 2, 3, 4])),
        ]));
        let map_bytes = Bytes::from(serialize_root_nbt(&map_root).expect("serialize"));
        let map = parse_map_record(MapRecordId::unchecked("5"), map_bytes).expect("map");

        assert_eq!(map.known_fields.center_x, Some(10));
        assert_eq!(
            map.pixels.as_ref().map(|pixels| pixels.colors.as_slice()),
            Some(&[1, 2, 3, 4][..])
        );

        let global = parse_global_record(
            GlobalRecordKind::Scoreboard,
            "scoreboard".to_string(),
            encode_consecutive_roots(&[NbtTag::Compound(IndexMap::new())]).expect("encode"),
        )
        .expect("global");
        assert_eq!(global.kind, GlobalRecordKind::Scoreboard);
    }

    #[test]
    fn chunk_record_parser_preserves_legacy_terrain_structure() {
        let records = vec![ChunkRecord {
            key: crate::ChunkKey::new(
                ChunkPos {
                    x: 0,
                    z: 0,
                    dimension: crate::Dimension::Overworld,
                },
                ChunkRecordTag::LegacyTerrain,
            ),
            value: Bytes::from(vec![0; crate::LEGACY_TERRAIN_VALUE_LEN]),
        }];

        let parsed = parse_chunk_records(
            ChunkPos {
                x: 0,
                z: 0,
                dimension: crate::Dimension::Overworld,
            },
            records,
        );

        assert_eq!(parsed.report.legacy_terrain_count, 1);
        assert!(matches!(
            parsed.records[0].value,
            ParsedChunkRecordValue::LegacyTerrain(_)
        ));
    }

    #[test]
    fn chunk_record_parser_counts_legacy_subchunks() {
        let mut value = vec![0; crate::LEGACY_SUBCHUNK_MIN_VALUE_LEN];
        value[0] = 2;
        let records = vec![ChunkRecord {
            key: crate::ChunkKey::subchunk(
                ChunkPos {
                    x: 0,
                    z: 0,
                    dimension: crate::Dimension::Overworld,
                },
                0,
            ),
            value: Bytes::from(value),
        }];

        let parsed = parse_chunk_records(
            ChunkPos {
                x: 0,
                z: 0,
                dimension: crate::Dimension::Overworld,
            },
            records,
        );

        assert_eq!(parsed.report.subchunk_count, 1);
        assert_eq!(parsed.report.legacy_subchunk_count, 1);
        assert!(matches!(
            parsed.records[0].value,
            ParsedChunkRecordValue::SubChunk(SubChunk {
                format: SubChunkFormat::LegacySubChunk(_),
                ..
            })
        ));
    }

    #[test]
    fn borrowed_chunk_record_parser_leaves_source_records_available() {
        let pos = ChunkPos {
            x: 4,
            z: -7,
            dimension: crate::Dimension::Overworld,
        };
        let records = vec![ChunkRecord {
            key: crate::ChunkKey::new(pos, ChunkRecordTag::Version),
            value: Bytes::from_static(&[42]),
        }];

        let parsed =
            parse_chunk_records_ref_with_options(pos, &records, WorldParseOptions::summary());

        assert_eq!((records[0].value[0], parsed.report.entry_count), (42, 1));
    }

    #[test]
    fn summary_parse_does_not_retain_raw_entries() {
        let storage = MemoryStorage::new();
        let chunk_key = crate::ChunkKey::new(
            ChunkPos {
                x: 0,
                z: 0,
                dimension: crate::Dimension::Overworld,
            },
            ChunkRecordTag::Version,
        );
        storage
            .put(&chunk_key.encode(), &[1])
            .expect("insert chunk version");
        storage
            .put(
                b"~local_player",
                &serialize_root_nbt(&NbtTag::Compound(IndexMap::new())).expect("serialize"),
            )
            .expect("insert player");

        let parsed = parse_world_storage(
            LevelDatDocument {
                header: crate::LevelDatHeader {
                    version: 10,
                    declared_len: 0,
                    actual_payload_len: 0,
                },
                root: NbtTag::Compound(IndexMap::new()),
                warnings: Vec::new(),
            },
            &storage,
            WorldParseOptions::summary(),
        )
        .expect("parse summary");

        assert_eq!(parsed.report.entry_count, 2);
        assert_eq!(parsed.report.chunk_count, 1);
        assert!(parsed.entries.is_empty());
    }

    #[test]
    fn summary_parse_uses_key_scan_without_actor_resolution() {
        let chunk_key = crate::ChunkKey::new(
            ChunkPos {
                x: 3,
                z: -4,
                dimension: crate::Dimension::Overworld,
            },
            ChunkRecordTag::Version,
        );
        let storage = KeyOnlySummaryStorage {
            key: chunk_key.encode(),
        };

        let parsed = parse_world_storage(
            LevelDatDocument::new(10, NbtTag::Compound(IndexMap::new())),
            &storage,
            WorldParseOptions::summary(),
        )
        .expect("parse summary");

        assert_eq!(parsed.report.entry_count, 1);
        assert_eq!(parsed.report.chunk_count, 1);
        assert!(parsed.entries.is_empty());
    }
}
