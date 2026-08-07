//! Bedrock chunk keys, coordinates, and subchunk payload parsing.
//!
//! This module decodes LevelDB key shapes used by Minecraft Bedrock worlds and
//! exposes conservative parsers for modern paletted subchunks plus older
//! LevelDB-era terrain arrays. Unsupported payloads are preserved as raw bytes
//! where possible so inspection tools can keep scanning mixed-version worlds.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, parse_consecutive_root_nbt, parse_root_nbt_with_consumed};
use crate::surface::is_air_block_name;
use bytes::Bytes;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;

const MAX_SUBCHUNK_PALETTE_LEN: usize = 4096;
/// Number of block ID entries in an old 16x128x16 `LegacyTerrain` value.
pub const LEGACY_TERRAIN_BLOCK_COUNT: usize = 16 * 128 * 16;
/// Exact byte length of an old `LevelDB` `LegacyTerrain` value.
pub const LEGACY_TERRAIN_VALUE_LEN: usize = 83_200;
/// Number of block entries in a 16x16x16 legacy subchunk.
pub const LEGACY_SUBCHUNK_BLOCK_COUNT: usize = 16 * 16 * 16;
/// Minimum byte length of a legacy subchunk without light arrays.
pub const LEGACY_SUBCHUNK_MIN_VALUE_LEN: usize =
    1 + LEGACY_SUBCHUNK_BLOCK_COUNT + LEGACY_SUBCHUNK_BLOCK_COUNT / 2;
/// Byte length of a legacy subchunk with sky and block light arrays.
pub const LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN: usize =
    LEGACY_SUBCHUNK_MIN_VALUE_LEN + LEGACY_SUBCHUNK_BLOCK_COUNT;

const LEGACY_TERRAIN_BLOCK_DATA_OFFSET: usize = LEGACY_TERRAIN_BLOCK_COUNT;
const LEGACY_TERRAIN_SKY_LIGHT_OFFSET: usize =
    LEGACY_TERRAIN_BLOCK_DATA_OFFSET + LEGACY_TERRAIN_BLOCK_COUNT / 2;
const LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET: usize =
    LEGACY_TERRAIN_SKY_LIGHT_OFFSET + LEGACY_TERRAIN_BLOCK_COUNT / 2;
const LEGACY_TERRAIN_HEIGHTMAP_OFFSET: usize =
    LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET + LEGACY_TERRAIN_BLOCK_COUNT / 2;
const LEGACY_TERRAIN_BIOME_OFFSET: usize = LEGACY_TERRAIN_HEIGHTMAP_OFFSET + 16 * 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Bedrock dimension identifier.
pub enum Dimension {
    /// The overworld dimension, encoded as `0`.
    Overworld,
    /// The Nether dimension, encoded as `1`.
    Nether,
    /// The End dimension, encoded as `2`.
    End,
    /// A dimension id not recognized by this crate.
    Unknown(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Vertical build-height generation used for chunk bounds.
pub enum ChunkVersion {
    /// Pre-Caves-and-Cliffs vertical range.
    Old,
    /// Modern extended vertical range.
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Absolute block position within a world.
pub struct BlockPos {
    /// Absolute X block coordinate.
    pub x: i32,
    /// Absolute Y block coordinate.
    pub y: i32,
    /// Absolute Z block coordinate.
    pub z: i32,
}

impl Dimension {
    #[must_use]
    /// Returns the numeric Bedrock dimension id.
    pub const fn id(self) -> i32 {
        match self {
            Self::Overworld => 0,
            Self::Nether => 1,
            Self::End => 2,
            Self::Unknown(value) => value,
        }
    }

    #[must_use]
    /// Decodes a numeric Bedrock dimension id.
    pub const fn from_id(id: i32) -> Self {
        match id {
            0 => Self::Overworld,
            1 => Self::Nether,
            2 => Self::End,
            value => Self::Unknown(value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Chunk position and dimension.
pub struct ChunkPos {
    /// Chunk X coordinate.
    pub x: i32,
    /// Chunk Z coordinate.
    pub z: i32,
    /// Dimension containing this chunk.
    pub dimension: Dimension,
}

impl ChunkPos {
    #[must_use]
    /// Returns the inclusive block Y range for this chunk and version.
    pub const fn y_range(self, version: ChunkVersion) -> (i32, i32) {
        match self.dimension {
            Dimension::Nether => (0, 127),
            Dimension::End => (0, 255),
            Dimension::Overworld => match version {
                ChunkVersion::Old => (0, 255),
                ChunkVersion::New => (-64, 319),
            },
            Dimension::Unknown(_) => (0, -1),
        }
    }

    #[must_use]
    /// Returns the inclusive subchunk Y-index range for this chunk and version.
    pub const fn subchunk_index_range(self, version: ChunkVersion) -> (i8, i8) {
        match self.dimension {
            Dimension::Nether => (0, 7),
            Dimension::End => (0, 15),
            Dimension::Overworld => match version {
                ChunkVersion::Old => (0, 15),
                ChunkVersion::New => (-4, 19),
            },
            Dimension::Unknown(_) => (0, -1),
        }
    }

    #[must_use]
    /// Returns the minimum block position covered by this chunk.
    pub const fn min_block_pos(self, version: ChunkVersion) -> BlockPos {
        let (min_y, _) = self.y_range(version);
        BlockPos {
            x: self.x * 16,
            y: min_y,
            z: self.z * 16,
        }
    }

    #[must_use]
    /// Returns the maximum block position covered by this chunk.
    pub const fn max_block_pos(self, version: ChunkVersion) -> BlockPos {
        let (_, max_y) = self.y_range(version);
        BlockPos {
            x: self.x * 16 + 15,
            y: max_y,
            z: self.z * 16 + 15,
        }
    }
}

impl BlockPos {
    #[must_use]
    /// Converts this block position to a chunk position in the given dimension.
    pub const fn to_chunk_pos(self, dimension: Dimension) -> ChunkPos {
        let x = if self.x < 0 { self.x - 15 } else { self.x } / 16;
        let z = if self.z < 0 { self.z - 15 } else { self.z } / 16;
        ChunkPos { x, z, dimension }
    }

    #[must_use]
    /// Returns local chunk X/Z offsets and the absolute Y coordinate.
    pub const fn in_chunk_offset(self) -> (u8, i32, u8) {
        let mut x = self.x % 16;
        let mut z = self.z % 16;
        if x < 0 {
            x += 16;
        }
        if z < 0 {
            z += 16;
        }
        (x as u8, self.y, z as u8)
    }
}

#[must_use]
/// Returns the Bedrock X-major storage index for local 16x16x16 coordinates.
pub fn block_storage_index(local_x: u8, local_y: u8, local_z: u8) -> usize {
    usize::from(local_x) * 256 + usize::from(local_z) * 16 + usize::from(local_y)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Bedrock chunk record tag byte used in `LevelDB` chunk keys.
pub enum ChunkRecordTag {
    /// Modern `Data3D` terrain and biome record.
    Data3D,
    /// Modern `Data2D` heightmap and biome record.
    Data2D,
    /// Legacy `Data2D` heightmap and biome record.
    Data2DLegacy,
    /// Subchunk payload record.
    SubChunkPrefix,
    /// Old LevelDB-era terrain record.
    LegacyTerrain,
    /// Block-entity NBT record.
    BlockEntity,
    /// Legacy inline entity NBT record.
    Entity,
    /// Pending tick NBT record.
    PendingTicks,
    /// Block extra-data record.
    BlockExtraData,
    /// Biome state record.
    BiomeState,
    /// Finalized state record.
    FinalizedState,
    /// Chunk conversion data record.
    ConversionData,
    /// Border blocks record.
    BorderBlocks,
    /// Hardcoded spawn-area record.
    HardcodedSpawners,
    /// Random tick record.
    RandomTicks,
    /// Checksums record.
    Checksums,
    /// Generation seed record.
    GenerationSeed,
    /// Metadata hash record.
    MetaDataHash,
    /// Pre-Caves-and-Cliffs blending marker.
    GeneratedPreCavesAndCliffsBlending,
    /// Blending biome-height record.
    BlendingBiomeHeight,
    /// Blending data record.
    BlendingData,
    /// Actor digest version record.
    ActorDigestVersion,
    /// Current chunk version record.
    Version,
    /// Old chunk version record.
    VersionOld,
    /// Legacy chunk version record.
    LegacyVersion,
    /// Unknown value preserved for forward compatibility.
    Unknown(u8),
}

impl ChunkRecordTag {
    #[must_use]
    /// Returns the raw chunk record tag byte.
    pub const fn byte(self) -> u8 {
        match self {
            Self::Data3D => 0x2b,
            Self::Version => 0x2c,
            Self::Data2D => 0x2d,
            Self::Data2DLegacy => 0x2e,
            Self::SubChunkPrefix => 0x2f,
            Self::LegacyTerrain => 0x30,
            Self::BlockEntity => 0x31,
            Self::Entity => 0x32,
            Self::PendingTicks => 0x33,
            Self::BlockExtraData => 0x34,
            Self::BiomeState => 0x35,
            Self::FinalizedState => 0x36,
            Self::ConversionData => 0x37,
            Self::BorderBlocks => 0x38,
            Self::HardcodedSpawners => 0x39,
            Self::RandomTicks => 0x3a,
            Self::Checksums => 0x3b,
            Self::GenerationSeed => 0x3c,
            Self::GeneratedPreCavesAndCliffsBlending => 0x3d,
            Self::BlendingBiomeHeight => 0x3e,
            Self::MetaDataHash => 0x3f,
            Self::BlendingData => 0x40,
            Self::ActorDigestVersion => 0x41,
            Self::VersionOld => 0x76,
            Self::LegacyVersion => 0x77,
            Self::Unknown(value) => value,
        }
    }

    #[must_use]
    /// Decodes a raw chunk record tag byte.
    pub const fn from_byte(value: u8) -> Self {
        match value {
            0x2b => Self::Data3D,
            0x2c => Self::Version,
            0x2d => Self::Data2D,
            0x2e => Self::Data2DLegacy,
            0x2f => Self::SubChunkPrefix,
            0x30 => Self::LegacyTerrain,
            0x31 => Self::BlockEntity,
            0x32 => Self::Entity,
            0x33 => Self::PendingTicks,
            0x34 => Self::BlockExtraData,
            0x35 => Self::BiomeState,
            0x36 => Self::FinalizedState,
            0x37 => Self::ConversionData,
            0x38 => Self::BorderBlocks,
            0x39 => Self::HardcodedSpawners,
            0x3a => Self::RandomTicks,
            0x3b => Self::Checksums,
            0x3c => Self::GenerationSeed,
            0x3d => Self::GeneratedPreCavesAndCliffsBlending,
            0x3e => Self::BlendingBiomeHeight,
            0x3f => Self::MetaDataHash,
            0x40 => Self::BlendingData,
            0x41 => Self::ActorDigestVersion,
            0x76 => Self::VersionOld,
            0x77 => Self::LegacyVersion,
            other => Self::Unknown(other),
        }
    }

    #[must_use]
    /// Returns whether this tag can contribute renderable terrain data.
    pub const fn is_render_chunk_record(self) -> bool {
        matches!(
            self,
            Self::Data3D
                | Self::Data2D
                | Self::Data2DLegacy
                | Self::LegacyTerrain
                | Self::SubChunkPrefix
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Classified Bedrock `LevelDB` key.
///
/// Chunk keys carry coordinate/tag structure. Non-chunk variants model the
/// documented global, player, map, village, actor, and string-key records while
/// preserving unknown bytes for forward compatibility.
pub enum BedrockDbKey {
    /// Chunk-scoped record such as subchunk terrain, block entities, or HSA.
    Chunk(ChunkKey),
    /// Local-player key, accepting both `LocalPlayer` and `~local_player`.
    LocalPlayer,
    /// Remote-player key using the `player_` prefix.
    RemotePlayer(String),
    /// Modern actor payload key `actorprefix<uid>`.
    ActorPrefix {
        /// Actor id encoded in an `actorprefix` key.
        actor_id: i64,
    },
    /// Modern actor digest key `digp<x><z>[dimension]`.
    ActorDigest {
        /// Chunk position encoded in a `digp` actor digest key.
        pos: ChunkPos,
    },
    /// Map data key with the `map_` prefix.
    Map(String),
    /// Village record key.
    Village(ParsedVillageKey),
    /// Known global record key.
    Global(GlobalRecordKind),
    /// Nether/end portal tracking record.
    Portals,
    /// Scheduler write tracking record.
    SchedulerWt,
    /// Structure-template record.
    StructureTemplate(String),
    /// Ticking-area record.
    TickingArea(String),
    /// Flat-world layer settings record.
    GameFlatWorldLayers,
    /// Other UTF-8 key not matched by a more specific classifier.
    PlainString(String),
    /// Non-UTF-8 or otherwise unknown key bytes.
    Unknown(Bytes),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Known village record suffix kind.
pub enum VillageRecordKind {
    /// Village info record.
    Info,
    /// Village dwellers record.
    Dwellers,
    /// Village players record.
    Players,
    /// Village point-of-interest record.
    Poi,
    /// Unknown value preserved for forward compatibility.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Parsed village storage key components.
pub struct ParsedVillageKey {
    /// Original raw value retained for inspection or roundtrip preservation.
    pub raw: String,
    /// Bedrock dimension encoded in the village key, when present.
    pub dimension: Option<Dimension>,
    /// Village UUID component decoded from the key.
    pub uuid: String,
    /// Classified kind for this record.
    pub kind: VillageRecordKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Validated map record identifier without the `map_` storage prefix.
pub struct MapRecordId(String);

impl MapRecordId {
    /// Creates a map record id from a printable ASCII suffix.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::Validation`] when the id is empty or
    /// contains non-printable/non-ASCII bytes.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.is_empty() || !id.as_bytes().iter().all(u8::is_ascii_graphic) {
            return Err(BedrockWorldError::Validation(
                "map id must be non-empty printable ASCII".to_string(),
            ));
        }
        Ok(Self(id))
    }

    #[must_use]
    /// Creates a map record id without validation.
    ///
    /// Use this only when preserving an already-decoded storage key.
    pub fn unchecked(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    /// Returns the id suffix without the `map_` storage prefix.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    /// Encodes this id as the `LevelDB` key `map_<id>`.
    pub fn storage_key(&self) -> Bytes {
        Bytes::from(format!("map_{}", self.0))
    }

    #[must_use]
    /// Decodes a `LevelDB` map key into an id suffix.
    pub fn from_storage_key(key: &[u8]) -> Option<Self> {
        ascii_suffix(key, b"map_").map(Self)
    }
}

impl std::fmt::Display for MapRecordId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for MapRecordId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Opaque 8-byte actor storage token stored in `digp` and appended to `actorprefix`.
///
/// This is deliberately not the NBT `UniqueID`. Bedrock derives this token from
/// `UniqueID` by complementing the world-start-count half and encoding the result
/// big-endian before storing those raw bytes in the database key.
pub struct ActorUid(pub i64);

impl ActorUid {
    #[must_use]
    /// Derives the modern actor storage token from the NBT `UniqueID` using the
    /// same transformation as Bedrock/BedrockMap.
    pub fn from_unique_id(unique_id: i64) -> Self {
        let unique = unique_id as u64;
        let world_start_count = unique >> 32;
        let index = unique & 0xffff_ffff;
        let storage = ((0xffff_ffff_u64.wrapping_sub(world_start_count)) << 32) | index;
        Self(i64::from_le_bytes(storage.to_be_bytes()))
    }

    #[must_use]
    /// Returns the exact eight storage bytes referenced by `digp`.
    pub const fn raw_storage_bytes(self) -> [u8; 8] {
        self.0.to_le_bytes()
    }

    #[must_use]
    /// Encodes this storage token as `actorprefix<raw 8 bytes>`.
    pub fn storage_key(self) -> Bytes {
        let mut bytes = Vec::with_capacity(19);
        bytes.extend_from_slice(b"actorprefix");
        bytes.extend_from_slice(&self.raw_storage_bytes());
        Bytes::from(bytes)
    }

    #[must_use]
    /// Decodes an `actorprefix` storage key into an actor id.
    pub fn from_actorprefix_key(key: &[u8]) -> Option<Self> {
        parse_i64_suffix(key, b"actorprefix").map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Chunk actor digest key used by modern Bedrock entity storage.
pub struct ActorDigestKey {
    /// Chunk whose digest lists actor ids for the chunk.
    pub pos: ChunkPos,
}

impl ActorDigestKey {
    #[must_use]
    /// Creates a digest key for a chunk.
    pub const fn new(pos: ChunkPos) -> Self {
        Self { pos }
    }

    #[must_use]
    /// Encodes this digest as `digp<x><z>[dimension]`.
    pub fn storage_key(self) -> Bytes {
        let mut bytes = Vec::with_capacity(if self.pos.dimension == Dimension::Overworld {
            12
        } else {
            16
        });
        bytes.extend_from_slice(b"digp");
        bytes.extend_from_slice(&self.pos.x.to_le_bytes());
        bytes.extend_from_slice(&self.pos.z.to_le_bytes());
        if self.pos.dimension != Dimension::Overworld {
            bytes.extend_from_slice(&self.pos.dimension.id().to_le_bytes());
        }
        Bytes::from(bytes)
    }

    #[must_use]
    /// Decodes a `digp` storage key into a digest key.
    pub fn from_storage_key(key: &[u8]) -> Option<Self> {
        parse_chunk_pos_suffix(key, b"digp").map(Self::new)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Known non-chunk global records in a Bedrock `LevelDB` world.
pub enum GlobalRecordKind {
    /// `mobevents` global NBT record.
    MobEvents,
    /// Dimension metadata record: `Overworld`, `Nether`, or `TheEnd`.
    Dimension(Dimension),
    /// `scoreboard` global NBT record.
    Scoreboard,
    /// `LocalPlayer` global/player record.
    LocalPlayer,
    /// Autonomous entity tracking record.
    AutonomousEntities,
    /// Global biome metadata dictionary.
    BiomeData,
    /// Level chunk metadata dictionary.
    LevelChunkMetaDataDictionary,
    /// World clock metadata.
    WorldClocks,
    /// Preserved UTF-8 global key not recognized by this crate.
    Other(String),
}

impl GlobalRecordKind {
    #[must_use]
    /// Classifies an exact storage key as a known global record.
    pub fn from_key(key: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(key).ok()?;
        match text {
            "mobevents" => Some(Self::MobEvents),
            "Overworld" => Some(Self::Dimension(Dimension::Overworld)),
            "Nether" => Some(Self::Dimension(Dimension::Nether)),
            "TheEnd" => Some(Self::Dimension(Dimension::End)),
            "scoreboard" => Some(Self::Scoreboard),
            "LocalPlayer" => Some(Self::LocalPlayer),
            "AutonomousEntities" | "autonomousentities" => Some(Self::AutonomousEntities),
            "BiomeData" => Some(Self::BiomeData),
            "LevelChunkMetaDataDictionary" => Some(Self::LevelChunkMetaDataDictionary),
            "WorldClocks" => Some(Self::WorldClocks),
            _ => None,
        }
    }

    #[must_use]
    /// Returns the canonical storage name for this global record.
    pub fn name(&self) -> String {
        match self {
            Self::MobEvents => "mobevents".to_string(),
            Self::Dimension(Dimension::Overworld) => "Overworld".to_string(),
            Self::Dimension(Dimension::Nether) => "Nether".to_string(),
            Self::Dimension(Dimension::End) => "TheEnd".to_string(),
            Self::Dimension(Dimension::Unknown(id)) => format!("Dimension({id})"),
            Self::Scoreboard => "scoreboard".to_string(),
            Self::LocalPlayer => "LocalPlayer".to_string(),
            Self::AutonomousEntities => "AutonomousEntities".to_string(),
            Self::BiomeData => "BiomeData".to_string(),
            Self::LevelChunkMetaDataDictionary => "LevelChunkMetaDataDictionary".to_string(),
            Self::WorldClocks => "WorldClocks".to_string(),
            Self::Other(name) => name.clone(),
        }
    }

    #[must_use]
    /// Encodes this global kind as an exact `LevelDB` key.
    pub fn storage_key(&self) -> Bytes {
        Bytes::from(self.name())
    }
}

impl BedrockDbKey {
    #[must_use]
    /// Decodes this value from Bedrock storage bytes.
    pub fn decode(key: &[u8]) -> Self {
        if key == b"~local_player" {
            return Self::LocalPlayer;
        }
        if let Some(remote_player) = key.strip_prefix(b"player_") {
            return Self::RemotePlayer(String::from_utf8_lossy(remote_player).into_owned());
        }
        if let Some(actor_id) = parse_i64_suffix(key, b"actorprefix") {
            return Self::ActorPrefix { actor_id };
        }
        if let Some(pos) = parse_chunk_pos_suffix(key, b"digp") {
            return Self::ActorDigest { pos };
        }
        if key == b"portals" {
            return Self::Portals;
        }
        if key == b"schedulerWT" {
            return Self::SchedulerWt;
        }
        if let Some(map_id) = ascii_suffix(key, b"map_") {
            return Self::Map(map_id);
        }
        if let Some(village) = parse_village_key(key) {
            return Self::Village(village);
        }
        if let Some(name) = ascii_suffix(key, b"structuretemplate") {
            return Self::StructureTemplate(name);
        }
        if let Some(name) = ascii_suffix(key, b"tickingarea") {
            return Self::TickingArea(name);
        }
        if key == b"game_flatworldlayers" {
            return Self::GameFlatWorldLayers;
        }
        if let Some(kind) = GlobalRecordKind::from_key(key) {
            return Self::Global(kind);
        }
        if key.iter().all(u8::is_ascii_graphic) {
            return Self::PlainString(String::from_utf8_lossy(key).into_owned());
        }
        if let Ok(chunk_key) = ChunkKey::decode(key) {
            if matches!(chunk_key.tag, ChunkRecordTag::Unknown(_)) {
                return Self::Unknown(Bytes::copy_from_slice(key));
            }
            return Self::Chunk(chunk_key);
        }
        Self::Unknown(Bytes::copy_from_slice(key))
    }

    #[must_use]
    /// Returns a stable human-readable key category.
    pub fn summary_kind(&self) -> String {
        match self {
            Self::Chunk(key) => format!("Chunk::{:?}", key.tag),
            Self::LocalPlayer => "LocalPlayer".to_string(),
            Self::RemotePlayer(_) => "RemotePlayer".to_string(),
            Self::ActorPrefix { .. } => "ActorPrefix".to_string(),
            Self::ActorDigest { .. } => "ActorDigest".to_string(),
            Self::Map(_) => "Map".to_string(),
            Self::Village(village) => format!("Village::{:?}", village.kind),
            Self::Global(kind) => format!("Global::{}", kind.name()),
            Self::Portals => "Portals".to_string(),
            Self::SchedulerWt => "SchedulerWt".to_string(),
            Self::StructureTemplate(_) => "StructureTemplate".to_string(),
            Self::TickingArea(_) => "TickingArea".to_string(),
            Self::GameFlatWorldLayers => "GameFlatWorldLayers".to_string(),
            Self::PlainString(value) => format!("PlainString::{value}"),
            Self::Unknown(_) => "Unknown".to_string(),
        }
    }

    #[must_use]
    /// Encodes this value into Bedrock storage bytes.
    pub fn encode(&self) -> Option<Bytes> {
        match self {
            Self::Chunk(key) => Some(key.encode()),
            Self::LocalPlayer => Some(Bytes::from_static(b"~local_player")),
            Self::RemotePlayer(xuid) => Some(Bytes::from(format!("player_{xuid}"))),
            Self::ActorPrefix { actor_id } => Some(ActorUid(*actor_id).storage_key()),
            Self::ActorDigest { pos } => Some(ActorDigestKey::new(*pos).storage_key()),
            Self::Map(id) => Some(MapRecordId::unchecked(id.clone()).storage_key()),
            Self::Village(key) => Some(Bytes::copy_from_slice(key.raw.as_bytes())),
            Self::Global(kind) => Some(kind.storage_key()),
            Self::Portals => Some(Bytes::from_static(b"portals")),
            Self::SchedulerWt => Some(Bytes::from_static(b"schedulerWT")),
            Self::StructureTemplate(name) => Some(Bytes::from(format!("structuretemplate{name}"))),
            Self::TickingArea(name) => Some(Bytes::from(format!("tickingarea{name}"))),
            Self::GameFlatWorldLayers => Some(Bytes::from_static(b"game_flatworldlayers")),
            Self::PlainString(name) => Some(Bytes::copy_from_slice(name.as_bytes())),
            Self::Unknown(bytes) => Some(bytes.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Allocation-free category for a raw Bedrock database key.
pub enum BedrockDbKeyKind {
    /// Chunk-scoped record and its record tag.
    Chunk(ChunkRecordTag),
    /// Local player record.
    LocalPlayer,
    /// Remote player record.
    RemotePlayer,
    /// Actor payload record.
    ActorPrefix,
    /// Actor digest record.
    ActorDigest,
    /// Map record.
    Map,
    /// Village record.
    Village,
    /// Known global record.
    Global,
    /// Unclassified key.
    Other,
}

impl BedrockDbKeyKind {
    #[must_use]
    /// Classifies a raw database key without allocating.
    pub fn classify(key: &[u8]) -> Self {
        if key == b"~local_player" || key == b"LocalPlayer" {
            return Self::LocalPlayer;
        }
        if key.starts_with(b"player_") {
            return Self::RemotePlayer;
        }
        if key.starts_with(b"actorprefix") {
            return Self::ActorPrefix;
        }
        if key.starts_with(b"digp") {
            return Self::ActorDigest;
        }
        if key.starts_with(b"map_") {
            return Self::Map;
        }
        if key.starts_with(b"VILLAGE_") {
            return Self::Village;
        }
        if is_known_global_key(key) {
            return Self::Global;
        }
        encoded_chunk_tag(key).map_or(Self::Other, Self::Chunk)
    }

    #[must_use]
    /// Returns the stable summary label for this category.
    pub fn summary_kind(self) -> String {
        match self {
            Self::Chunk(tag) => format!("Chunk::{tag:?}"),
            Self::LocalPlayer => "LocalPlayer".to_string(),
            Self::RemotePlayer => "RemotePlayer".to_string(),
            Self::ActorPrefix => "ActorPrefix".to_string(),
            Self::ActorDigest => "ActorDigest".to_string(),
            Self::Map => "Map".to_string(),
            Self::Village => "Village".to_string(),
            Self::Global => "Global".to_string(),
            Self::Other => "Other".to_string(),
        }
    }
}

fn is_known_global_key(key: &[u8]) -> bool {
    const GLOBAL_KEYS: &[&[u8]] = &[
        b"mobevents",
        b"Overworld",
        b"Nether",
        b"TheEnd",
        b"scoreboard",
        b"AutonomousEntities",
        b"autonomousentities",
        b"BiomeData",
        b"LevelChunkMetaDataDictionary",
        b"WorldClocks",
        b"portals",
        b"schedulerWT",
        b"game_flatworldlayers",
    ];
    GLOBAL_KEYS.contains(&key)
}

fn encoded_chunk_tag(key: &[u8]) -> Option<ChunkRecordTag> {
    let tag_index = match key.len() {
        9 | 10 => 8,
        13 | 14 => 12,
        _ => return None,
    };
    let tag = ChunkRecordTag::from_byte(*key.get(tag_index)?);
    (!matches!(tag, ChunkRecordTag::Unknown(_))).then_some(tag)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Stack-encoded Bedrock chunk key.
pub struct EncodedChunkKey {
    bytes: [u8; 14],
    len: u8,
}

impl EncodedChunkKey {
    #[must_use]
    /// Returns the encoded key bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    #[must_use]
    /// Returns the encoded key length.
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    #[must_use]
    /// Returns whether the encoded key is empty.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl AsRef<[u8]> for EncodedChunkKey {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Decoded chunk storage key with position, tag, and optional subchunk index.
pub struct ChunkKey {
    /// Chunk position encoded in the storage key.
    pub pos: ChunkPos,
    /// Chunk record tag byte decoded from the key.
    pub tag: ChunkRecordTag,
    /// Optional subchunk Y index for `SubChunkPrefix` records.
    pub subchunk_y: Option<i8>,
}

impl ChunkKey {
    #[must_use]
    /// Creates a non-subchunk chunk key for the given position and record tag.
    pub const fn new(pos: ChunkPos, tag: ChunkRecordTag) -> Self {
        Self {
            pos,
            tag,
            subchunk_y: None,
        }
    }

    #[must_use]
    /// Creates a `SubChunkPrefix` key for the given vertical subchunk index.
    pub const fn subchunk(pos: ChunkPos, y: i8) -> Self {
        Self {
            pos,
            tag: ChunkRecordTag::SubChunkPrefix,
            subchunk_y: Some(y),
        }
    }

    #[must_use]
    /// Encodes this value into a stack-backed key.
    pub fn encode_inline(&self) -> EncodedChunkKey {
        let mut bytes = [0_u8; 14];
        bytes[..4].copy_from_slice(&self.pos.x.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.pos.z.to_le_bytes());
        let mut len = 8_usize;
        if self.pos.dimension != Dimension::Overworld {
            bytes[len..len + 4].copy_from_slice(&self.pos.dimension.id().to_le_bytes());
            len += 4;
        }
        bytes[len] = self.tag.byte();
        len += 1;
        if let Some(y) = self.subchunk_y {
            bytes[len] = y.to_ne_bytes()[0];
            len += 1;
        }
        EncodedChunkKey {
            bytes,
            len: u8::try_from(len).unwrap_or(14),
        }
    }

    #[must_use]
    /// Encodes this value into owned Bedrock storage bytes.
    pub fn encode(&self) -> Bytes {
        Bytes::copy_from_slice(self.encode_inline().as_bytes())
    }

    /// Decodes this value from Bedrock storage bytes.
    pub fn decode(key: &[u8]) -> Result<Self> {
        match key.len() {
            9 | 10 | 13 | 14 => {}
            len => {
                return Err(BedrockWorldError::InvalidKey(format!(
                    "unsupported chunk key length: {len}"
                )));
            }
        }

        let x = read_i32(key, 0)?;
        let z = read_i32(key, 4)?;
        let (dimension, tag_index) = if key.len() >= 13 {
            (Dimension::from_id(read_i32(key, 8)?), 12)
        } else {
            (Dimension::Overworld, 8)
        };
        let tag = ChunkRecordTag::from_byte(
            *key.get(tag_index)
                .ok_or_else(|| BedrockWorldError::InvalidKey("missing record tag".to_string()))?,
        );
        let subchunk_y = if matches!(key.len(), 10 | 14) {
            Some(i8::from_ne_bytes([key[tag_index + 1]]))
        } else {
            None
        };
        Ok(Self {
            pos: ChunkPos { x, z, dimension },
            tag,
            subchunk_y,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Raw chunk record paired with its decoded chunk key.
pub struct ChunkRecord {
    /// Decoded storage key for this record.
    pub key: ChunkKey,
    /// Parsed or raw value associated with this record.
    pub value: Bytes,
}

#[derive(Debug, Clone, PartialEq)]
/// Block state decoded from a Bedrock palette entry.
pub struct BlockState {
    /// Named Bedrock value or identifier.
    pub name: String,
    /// Palette block states in storage order.
    pub states: BTreeMap<String, NbtTag>,
    /// Bedrock format or payload version.
    pub version: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
/// Block palette and optional unpacked indices for a subchunk storage.
pub struct BlockPalette {
    /// Palette block states in storage order.
    pub states: Vec<BlockState>,
    /// Optional unpacked palette indices in Bedrock storage order.
    pub indices: Option<Vec<u16>>,
    /// Packed palette indices retained for exact surface-column sampling.
    packed_indices: Option<PackedPaletteIndices>,
    /// Per-palette-entry usage counts when the decode request needs them.
    pub counts: Option<Vec<u16>>,
}

#[derive(Debug, Clone, PartialEq)]
struct PackedPaletteIndices {
    bytes: Bytes,
    bits_per_block: u8,
    palette_len: usize,
}

impl PackedPaletteIndices {
    fn get(&self, index: usize) -> Option<u16> {
        if index >= 4096 {
            return None;
        }
        if self.bits_per_block == 0 {
            return Some(0);
        }
        let values_per_word = usize::from(32 / self.bits_per_block);
        let word_index = index / values_per_word;
        let item_index = index % values_per_word;
        let byte_offset = word_index.checked_mul(4)?;
        let word_bytes: [u8; 4] = self
            .bytes
            .get(byte_offset..byte_offset + 4)?
            .try_into()
            .ok()?;
        let word = u32::from_le_bytes(word_bytes);
        let mask = (1_u32 << self.bits_per_block) - 1;
        let value = ((word >> (item_index * usize::from(self.bits_per_block))) & mask) as u16;
        (usize::from(value) < self.palette_len).then_some(value)
    }

    fn decode_all(&self) -> Option<Vec<u16>> {
        if self.bits_per_block == 0 {
            return Some(vec![0; 4096]);
        }
        let values_per_word = usize::from(32 / self.bits_per_block);
        let mask = (1_u32 << self.bits_per_block) - 1;
        let mut values = Vec::with_capacity(4096);
        for word_bytes in self.bytes.chunks_exact(4) {
            let word = u32::from_le_bytes(word_bytes.try_into().ok()?);
            for item_index in 0..values_per_word {
                if values.len() == 4096 {
                    return Some(values);
                }
                let value =
                    ((word >> (item_index * usize::from(self.bits_per_block))) & mask) as u16;
                if usize::from(value) >= self.palette_len {
                    return None;
                }
                values.push(value);
            }
        }
        (values.len() == 4096).then_some(values)
    }
}

impl BlockPalette {
    #[must_use]
    /// Creates a palette backed by already unpacked block indices.
    ///
    /// This is intended for callers that construct decoded subchunks, including
    /// tests and format adapters. Normal storage decoding retains a packed
    /// representation when unpacking is unnecessary.
    pub fn with_unpacked_indices(
        states: Vec<BlockState>,
        indices: Vec<u16>,
        counts: Option<Vec<u16>>,
    ) -> Self {
        Self {
            states,
            indices: Some(indices),
            packed_indices: None,
            counts,
        }
    }

    #[must_use]
    /// Returns the decoded palette index at local subchunk coordinates.
    pub fn palette_index_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u16> {
        if local_x >= 16 || local_y >= 16 || local_z >= 16 {
            return None;
        }
        let index = block_storage_index(local_x, local_y, local_z);
        self.indices
            .as_ref()
            .and_then(|indices| indices.get(index).copied())
            .or_else(|| self.packed_indices.as_ref()?.get(index))
    }

    #[must_use]
    /// Returns the block state at local subchunk coordinates.
    pub fn block_state_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<&BlockState> {
        let palette_index = usize::from(self.palette_index_at(local_x, local_y, local_z)?);
        self.states.get(palette_index)
    }

    fn block_state_with_palette_index_at(
        &self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
    ) -> Option<BlockStatePaletteEntry<'_>> {
        let palette_index = usize::from(self.palette_index_at(local_x, local_y, local_z)?);
        let state = self.states.get(palette_index)?;
        Some(BlockStatePaletteEntry {
            state,
            storage_index: 0,
        })
    }

    pub(crate) fn surface_indices(&self) -> Option<Cow<'_, [u16]>> {
        if let Some(indices) = &self.indices {
            return (indices.len() >= 4096).then(|| Cow::Borrowed(&indices[..4096]));
        }
        self.packed_indices.as_ref()?.decode_all().map(Cow::Owned)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BlockStatePaletteEntry<'chunk> {
    pub(crate) state: &'chunk BlockState,
    pub(crate) storage_index: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
/// Controls whether subchunk parsing keeps full indices or counts only.
pub enum SubChunkDecodeMode {
    /// Decode palette counts without retaining all block indices.
    CountsOnly,
    /// Retain packed palette indices for exact surface-column sampling.
    SurfaceColumns,
    /// Retain packed palette words while allowing full 3D random access.
    ///
    /// This avoids allocating a 4096-entry `u16` array for every palette storage.
    PackedIndices,
    #[default]
    /// Decode and retain full block index arrays.
    FullIndices,
}

#[derive(Debug, Clone, PartialEq)]
/// Decoded subchunk payload family.
pub enum SubChunkFormat {
    /// Legacy pre-paletted subchunk payload.
    LegacySubChunk(LegacySubChunk),
    /// Old LevelDB-era terrain record.
    LegacyTerrain,
    /// Old fixed-array v1 subchunk payload.
    FixedArrayV1,
    /// Modern paletted subchunk payload.
    Paletted {
        /// Bedrock format or payload version.
        version: u8,
        /// Biome or block storages decoded from the record.
        storages: Vec<BlockPalette>,
    },
    /// Raw bytes preserved because the payload was not decoded.
    Raw {
        /// Bedrock format or payload version.
        version: Option<u8>,
        /// Raw payload bytes preserved for unsupported formats.
        bytes: Bytes,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// Decoded subchunk at a vertical subchunk index.
pub struct SubChunk {
    /// Vertical subchunk index encoded by the storage key.
    pub y: i8,
    /// Decoded payload family for this value.
    pub format: SubChunkFormat,
}

impl SubChunk {
    #[must_use]
    /// Returns the primary block state at local subchunk coordinates.
    pub fn block_state_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<&BlockState> {
        match &self.format {
            SubChunkFormat::Paletted { storages, .. } => storages
                .first()
                .and_then(|storage| storage.block_state_at(local_x, local_y, local_z)),
            _ => None,
        }
    }

    #[must_use]
    /// Returns the first visible block state at local subchunk coordinates.
    pub fn visible_block_state_at(
        &self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
    ) -> Option<&BlockState> {
        self.visible_block_states_at(local_x, local_y, local_z)
            .next()
    }

    #[must_use]
    /// Iterates visible block states at local subchunk coordinates from top storage to bottom.
    pub fn visible_block_states_at(
        &self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
    ) -> VisibleBlockStatesAt<'_> {
        let storages = match &self.format {
            SubChunkFormat::Paletted { storages, .. } => Some(storages.iter().rev()),
            _ => None,
        };
        VisibleBlockStatesAt {
            storages,
            local_x,
            local_y,
            local_z,
        }
    }

    pub(crate) fn visible_block_surface_states_at(
        &self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
    ) -> VisibleBlockSurfaceStatesAt<'_> {
        let storages = match &self.format {
            SubChunkFormat::Paletted { storages, .. } => Some(storages.iter().enumerate().rev()),
            _ => None,
        };
        VisibleBlockSurfaceStatesAt {
            storages,
            local_x,
            local_y,
            local_z,
        }
    }

    #[must_use]
    /// Legacy block id at.
    pub fn legacy_block_id_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u8> {
        match &self.format {
            SubChunkFormat::LegacySubChunk(subchunk) => {
                subchunk.block_id_at(local_x, local_y, local_z)
            }
            _ => None,
        }
    }

    #[must_use]
    /// Legacy block data at.
    pub fn legacy_block_data_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u8> {
        match &self.format {
            SubChunkFormat::LegacySubChunk(subchunk) => {
                subchunk.block_data_at(local_x, local_y, local_z)
            }
            _ => None,
        }
    }
}

/// Iterator over visible block states at a local coordinate.
pub struct VisibleBlockStatesAt<'chunk> {
    storages: Option<std::iter::Rev<std::slice::Iter<'chunk, BlockPalette>>>,
    local_x: u8,
    local_y: u8,
    local_z: u8,
}

impl<'chunk> Iterator for VisibleBlockStatesAt<'chunk> {
    type Item = &'chunk BlockState;

    fn next(&mut self) -> Option<Self::Item> {
        let storages = self.storages.as_mut()?;
        for storage in storages {
            let Some(entry) =
                storage.block_state_with_palette_index_at(self.local_x, self.local_y, self.local_z)
            else {
                continue;
            };
            if !is_air_block_name(&entry.state.name) {
                return Some(entry.state);
            }
        }
        None
    }
}

pub(crate) struct VisibleBlockSurfaceStatesAt<'chunk> {
    storages: Option<std::iter::Rev<std::iter::Enumerate<std::slice::Iter<'chunk, BlockPalette>>>>,
    local_x: u8,
    local_y: u8,
    local_z: u8,
}

impl<'chunk> Iterator for VisibleBlockSurfaceStatesAt<'chunk> {
    type Item = BlockStatePaletteEntry<'chunk>;

    fn next(&mut self) -> Option<Self::Item> {
        let storages = self.storages.as_mut()?;
        for (storage_index, storage) in storages {
            let Some(mut entry) =
                storage.block_state_with_palette_index_at(self.local_x, self.local_y, self.local_z)
            else {
                continue;
            };
            if !is_air_block_name(&entry.state.name) {
                entry.storage_index = storage_index;
                return Some(entry);
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Legacy biome sample containing biome id and saved RGB components.
pub struct LegacyBiomeSample {
    /// Biome id associated with the sampled column.
    pub biome_id: u8,
    /// Red color component saved by legacy biome data.
    pub red: u8,
    /// Green color component saved by legacy biome data.
    pub green: u8,
    /// Blue color component saved by legacy biome data.
    pub blue: u8,
}

impl LegacyBiomeSample {
    #[must_use]
    /// Rgb u32.
    pub const fn rgb_u32(self) -> u32 {
        ((self.red as u32) << 16) | ((self.green as u32) << 8) | self.blue as u32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Decoded view over an old LevelDB-era terrain value.
pub struct LegacyTerrain {
    bytes: Bytes,
}

impl LegacyTerrain {
    /// Parses this value from Bedrock storage bytes.
    pub fn parse(bytes: Bytes) -> Result<Self> {
        if bytes.len() != LEGACY_TERRAIN_VALUE_LEN {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "LegacyTerrain value must be {LEGACY_TERRAIN_VALUE_LEN} bytes, got {}",
                bytes.len()
            )));
        }
        Ok(Self { bytes })
    }

    #[must_use]
    /// Returns the complete raw `LegacyTerrain` value bytes.
    pub fn raw(&self) -> &Bytes {
        &self.bytes
    }

    #[must_use]
    /// Returns the 16x128x16 block id array.
    pub fn block_ids(&self) -> &[u8] {
        &self.bytes[..LEGACY_TERRAIN_BLOCK_COUNT]
    }

    #[must_use]
    /// Returns packed 4-bit block data values.
    pub fn block_data(&self) -> &[u8] {
        &self.bytes[LEGACY_TERRAIN_BLOCK_DATA_OFFSET..LEGACY_TERRAIN_SKY_LIGHT_OFFSET]
    }

    #[must_use]
    /// Returns packed 4-bit sky-light values.
    pub fn sky_light(&self) -> &[u8] {
        &self.bytes[LEGACY_TERRAIN_SKY_LIGHT_OFFSET..LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET]
    }

    #[must_use]
    /// Returns packed 4-bit block-light values.
    pub fn block_light(&self) -> &[u8] {
        &self.bytes[LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET..LEGACY_TERRAIN_HEIGHTMAP_OFFSET]
    }

    #[must_use]
    /// Returns raw heightmap bytes in `z * 16 + x` column order.
    pub fn heightmap(&self) -> &[u8] {
        &self.bytes[LEGACY_TERRAIN_HEIGHTMAP_OFFSET..LEGACY_TERRAIN_BIOME_OFFSET]
    }

    #[must_use]
    /// Returns legacy biome samples as `[biome_id, red, green, blue]` columns.
    pub fn biomes(&self) -> &[u8] {
        &self.bytes[LEGACY_TERRAIN_BIOME_OFFSET..LEGACY_TERRAIN_VALUE_LEN]
    }

    #[must_use]
    /// Returns the legacy terrain block-array index for local coordinates.
    pub fn block_index(local_x: u8, local_y: u8, local_z: u8) -> Option<usize> {
        if local_x < 16 && local_y < 128 && local_z < 16 {
            Some((usize::from(local_x) << 11) | (usize::from(local_z) << 7) | usize::from(local_y))
        } else {
            None
        }
    }

    #[must_use]
    /// Returns the horizontal column index in `z * 16 + x` order.
    pub fn column_index(local_x: u8, local_z: u8) -> Option<usize> {
        if local_x < 16 && local_z < 16 {
            Some(usize::from(local_z) * 16 + usize::from(local_x))
        } else {
            None
        }
    }

    #[must_use]
    /// Returns the legacy numeric block id at local coordinates.
    pub fn block_id_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u8> {
        Self::block_index(local_x, local_y, local_z)
            .and_then(|index| self.block_ids().get(index).copied())
    }

    #[must_use]
    /// Returns the 4-bit block data value at local coordinates.
    pub fn block_data_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u8> {
        Self::block_index(local_x, local_y, local_z)
            .and_then(|index| nibble_at(self.block_data(), index))
    }

    #[must_use]
    /// Returns the 4-bit sky-light value at local coordinates.
    pub fn sky_light_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u8> {
        Self::block_index(local_x, local_y, local_z)
            .and_then(|index| nibble_at(self.sky_light(), index))
    }

    #[must_use]
    /// Returns the 4-bit block-light value at local coordinates.
    pub fn block_light_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u8> {
        Self::block_index(local_x, local_y, local_z)
            .and_then(|index| nibble_at(self.block_light(), index))
    }

    #[must_use]
    /// Returns the raw terrain heightmap value for a local column.
    pub fn height_at(&self, local_x: u8, local_z: u8) -> Option<u8> {
        Self::column_index(local_x, local_z).and_then(|index| self.heightmap().get(index).copied())
    }

    #[must_use]
    /// Returns the legacy biome sample for a local column.
    pub fn biome_sample_at(&self, local_x: u8, local_z: u8) -> Option<LegacyBiomeSample> {
        let offset = Self::column_index(local_x, local_z)?.checked_mul(4)?;
        let bytes = self.biomes().get(offset..offset + 4)?;
        Some(LegacyBiomeSample {
            biome_id: bytes[0],
            red: bytes[1],
            green: bytes[2],
            blue: bytes[3],
        })
    }

    #[must_use]
    /// Returns the legacy RGB biome color for a local column.
    pub fn biome_color_at(&self, local_x: u8, local_z: u8) -> Option<u32> {
        self.biome_sample_at(local_x, local_z)
            .map(LegacyBiomeSample::rgb_u32)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Decoded view over a legacy pre-paletted subchunk payload.
pub struct LegacySubChunk {
    version: u8,
    bytes: Bytes,
}

impl LegacySubChunk {
    /// Parses this value from Bedrock storage bytes.
    pub fn parse(bytes: Bytes) -> Result<Self> {
        let Some(version) = bytes.first().copied() else {
            return Err(BedrockWorldError::UnsupportedChunkFormat(
                "legacy subchunk value is empty".to_string(),
            ));
        };
        if !matches!(version, 0 | 2..=7) {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "version {version} is not a legacy subchunk payload"
            )));
        }
        if !matches!(
            bytes.len(),
            LEGACY_SUBCHUNK_MIN_VALUE_LEN | LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN
        ) {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "legacy subchunk value has invalid length {}",
                bytes.len()
            )));
        }
        Ok(Self { version, bytes })
    }

    #[must_use]
    /// Returns the legacy subchunk payload version byte.
    pub const fn version(&self) -> u8 {
        self.version
    }

    #[must_use]
    /// Returns the complete raw legacy subchunk payload.
    pub fn raw(&self) -> &Bytes {
        &self.bytes
    }

    #[must_use]
    /// Returns the 16x16x16 block id array.
    pub fn block_ids(&self) -> &[u8] {
        let start = 1;
        let end = start + LEGACY_SUBCHUNK_BLOCK_COUNT;
        &self.bytes[start..end]
    }

    #[must_use]
    /// Returns packed 4-bit block data values.
    pub fn block_data(&self) -> &[u8] {
        let start = 1 + LEGACY_SUBCHUNK_BLOCK_COUNT;
        let end = start + LEGACY_SUBCHUNK_BLOCK_COUNT / 2;
        &self.bytes[start..end]
    }

    #[must_use]
    /// Returns packed 4-bit sky-light values when present.
    pub fn sky_light(&self) -> Option<&[u8]> {
        if self.bytes.len() != LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN {
            return None;
        }
        let start = 1 + LEGACY_SUBCHUNK_BLOCK_COUNT + LEGACY_SUBCHUNK_BLOCK_COUNT / 2;
        let end = start + LEGACY_SUBCHUNK_BLOCK_COUNT / 2;
        Some(&self.bytes[start..end])
    }

    #[must_use]
    /// Returns packed 4-bit block-light values when present.
    pub fn block_light(&self) -> Option<&[u8]> {
        if self.bytes.len() != LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN {
            return None;
        }
        let start = 1 + LEGACY_SUBCHUNK_BLOCK_COUNT + LEGACY_SUBCHUNK_BLOCK_COUNT;
        Some(&self.bytes[start..])
    }

    #[must_use]
    /// Returns the legacy subchunk block-array index for local coordinates.
    pub fn block_index(local_x: u8, local_y: u8, local_z: u8) -> Option<usize> {
        if local_x < 16 && local_y < 16 && local_z < 16 {
            Some(usize::from(local_x) * 256 + usize::from(local_z) * 16 + usize::from(local_y))
        } else {
            None
        }
    }

    #[must_use]
    /// Returns the legacy numeric block id at local subchunk coordinates.
    pub fn block_id_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u8> {
        Self::block_index(local_x, local_y, local_z)
            .and_then(|index| self.block_ids().get(index).copied())
    }

    #[must_use]
    /// Returns the 4-bit block data value at local subchunk coordinates.
    pub fn block_data_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u8> {
        Self::block_index(local_x, local_y, local_z)
            .and_then(|index| nibble_at(self.block_data(), index))
    }

    #[must_use]
    /// Returns the 4-bit sky-light value at local subchunk coordinates.
    pub fn sky_light_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u8> {
        Self::block_index(local_x, local_y, local_z)
            .and_then(|index| nibble_at(self.sky_light()?, index))
    }

    #[must_use]
    /// Returns the 4-bit block-light value at local subchunk coordinates.
    pub fn block_light_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u8> {
        Self::block_index(local_x, local_y, local_z)
            .and_then(|index| nibble_at(self.block_light()?, index))
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Entity data data model.
pub struct EntityData {
    /// Root NBT tag for the entity payload.
    pub tag: NbtTag,
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed chunk with records grouped by position.
pub struct Chunk {
    /// Chunk position represented by this parsed chunk.
    pub pos: ChunkPos,
    /// Bedrock format or payload version.
    pub version: Option<u8>,
    /// Records included in this result.
    pub records: Vec<ChunkRecord>,
}

impl Chunk {
    /// Returns a decoded subchunk by vertical index, when the record is present.
    pub fn get_subchunk(&self, y: i8) -> Result<Option<SubChunk>> {
        let Some(record) = self.records.iter().find(|record| {
            record.key.tag == ChunkRecordTag::SubChunkPrefix && record.key.subchunk_y == Some(y)
        }) else {
            return Ok(None);
        };
        parse_subchunk(y, record.value.clone()).map(Some)
    }

    /// Returns the decoded legacy terrain record, when present.
    pub fn legacy_terrain(&self) -> Result<Option<LegacyTerrain>> {
        let Some(record) = self
            .records
            .iter()
            .find(|record| record.key.tag == ChunkRecordTag::LegacyTerrain)
        else {
            return Ok(None);
        };
        LegacyTerrain::parse(record.value.clone()).map(Some)
    }

    /// Get block.
    pub fn get_block(&self, x: u8, y: i16, z: u8) -> Result<BlockState> {
        if x >= 16 || z >= 16 {
            return Err(BedrockWorldError::Validation(format!(
                "local block coordinates must use x/z in 0..15, got x={x}, z={z}"
            )));
        }

        let subchunk_y = i8::try_from(i32::from(y).div_euclid(16)).map_err(|_| {
            BedrockWorldError::Validation(format!(
                "block y={y} cannot be represented as a Bedrock subchunk index"
            ))
        })?;
        let local_y = u8::try_from(i32::from(y).rem_euclid(16)).map_err(|_| {
            BedrockWorldError::Validation(format!("block y={y} has invalid local subchunk offset"))
        })?;
        if let Some(subchunk) = self.get_subchunk(subchunk_y)? {
            if let Some(state) = subchunk.block_state_at(x, local_y, z) {
                return Ok(state.clone());
            }
            if let Some(id) = subchunk.legacy_block_id_at(x, local_y, z) {
                let mut states = BTreeMap::new();
                if let Some(data) = subchunk.legacy_block_data_at(x, local_y, z) {
                    states.insert("data".to_string(), NbtTag::Byte(data as i8));
                }
                return Ok(BlockState {
                    name: format!("legacy:{id}"),
                    states,
                    version: None,
                });
            }
        }
        if (0..=127).contains(&y) {
            let Some(terrain) = self.legacy_terrain()? else {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "chunk {:?} has no legacy terrain record",
                    self.pos
                )));
            };
            let local_y = u8::try_from(y).map_err(|_| {
                BedrockWorldError::Validation(format!("legacy block y={y} is outside 0..127"))
            })?;
            let id = terrain.block_id_at(x, local_y, z).ok_or_else(|| {
                BedrockWorldError::UnsupportedChunkFormat(format!(
                    "chunk {:?} has no legacy block id at local ({x}, {y}, {z})",
                    self.pos
                ))
            })?;
            let data = terrain.block_data_at(x, local_y, z).unwrap_or(0);
            let mut states = BTreeMap::new();
            states.insert("data".to_string(), NbtTag::Byte(data as i8));
            return Ok(BlockState {
                name: format!("legacy:{id}"),
                states,
                version: None,
            });
        }
        Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "chunk {:?} does not expose a block state at local ({x}, {y}, {z})",
            self.pos
        )))
    }

    /// Set block.
    pub fn set_block(&mut self, _x: u8, _y: i16, _z: u8, _block: BlockState) -> Result<()> {
        Err(BedrockWorldError::UnsupportedChunkFormat(
            "structured block editing is not enabled for this chunk format".to_string(),
        ))
    }

    /// Get entities.
    pub fn get_entities(&self) -> Result<Vec<EntityData>> {
        let mut entities = Vec::new();
        for record in self
            .records
            .iter()
            .filter(|record| record.key.tag == ChunkRecordTag::Entity)
        {
            entities.extend(parse_consecutive_nbt(record.value.as_ref())?);
        }
        Ok(entities)
    }

    /// Get block entities.
    pub fn get_block_entities(&self) -> Result<Vec<EntityData>> {
        let mut entities = Vec::new();
        for record in self
            .records
            .iter()
            .filter(|record| record.key.tag == ChunkRecordTag::BlockEntity)
        {
            entities.extend(parse_consecutive_nbt(record.value.as_ref())?);
        }
        Ok(entities)
    }
}

/// Parse subchunk.
pub fn parse_subchunk(y: i8, bytes: Bytes) -> Result<SubChunk> {
    parse_subchunk_with_mode(y, bytes, SubChunkDecodeMode::FullIndices)
}

/// Parse subchunk with mode.
pub fn parse_subchunk_with_mode(y: i8, bytes: Bytes, mode: SubChunkDecodeMode) -> Result<SubChunk> {
    let version = bytes.first().copied();
    let format = match version {
        Some(0 | 2..=7) => LegacySubChunk::parse(bytes.clone()).map_or_else(
            |_| SubChunkFormat::Raw { version, bytes },
            SubChunkFormat::LegacySubChunk,
        ),
        Some(version @ 1) => parse_exact_palette_storages(&bytes, 1, 1, mode).map_or_else(
            |_| SubChunkFormat::Raw {
                version: Some(version),
                bytes,
            },
            |storages| SubChunkFormat::Paletted { version, storages },
        ),
        Some(version @ 8..=u8::MAX) => parse_paletted_subchunk(version, &bytes, mode)
            .unwrap_or_else(|_| SubChunkFormat::Raw {
                version: Some(version),
                bytes,
            }),
        _ => SubChunkFormat::Raw { version, bytes },
    };
    Ok(SubChunk { y, format })
}

fn parse_consecutive_nbt(bytes: &[u8]) -> Result<Vec<EntityData>> {
    parse_consecutive_root_nbt(bytes)
        .map(|tags| tags.into_iter().map(|tag| EntityData { tag }).collect())
}

fn parse_paletted_subchunk(
    version: u8,
    bytes: &[u8],
    mode: SubChunkDecodeMode,
) -> Result<SubChunkFormat> {
    let Some(storage_count) = bytes.get(1).copied() else {
        return Err(BedrockWorldError::UnsupportedChunkFormat(
            "paletted subchunk is missing storage count".to_string(),
        ));
    };
    let offsets: &[usize] = if version == 9 { &[3, 2] } else { &[2] };
    for offset in offsets {
        if let Ok(storages) = parse_exact_palette_storages(bytes, *offset, storage_count, mode) {
            return Ok(SubChunkFormat::Paletted { version, storages });
        }
    }
    Err(BedrockWorldError::UnsupportedChunkFormat(
        "unsupported paletted subchunk layout".to_string(),
    ))
}

fn parse_exact_palette_storages(
    bytes: &[u8],
    offset: usize,
    storage_count: u8,
    mode: SubChunkDecodeMode,
) -> Result<Vec<BlockPalette>> {
    let (storages, consumed) = parse_palette_storages(bytes, offset, storage_count, mode)?;
    if consumed != bytes.len() {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "palette storage ended at byte {consumed} but payload has {} bytes",
            bytes.len()
        )));
    }
    Ok(storages)
}

fn parse_palette_storages(
    bytes: &[u8],
    mut offset: usize,
    storage_count: u8,
    mode: SubChunkDecodeMode,
) -> Result<(Vec<BlockPalette>, usize)> {
    let mut storages = Vec::with_capacity(usize::from(storage_count));
    for _ in 0..storage_count {
        let header = *bytes.get(offset).ok_or_else(|| {
            BedrockWorldError::UnsupportedChunkFormat(
                "palette storage header is missing".to_string(),
            )
        })?;
        offset += 1;

        let bits_per_block = header >> 1;
        if !matches!(bits_per_block, 0 | 1 | 2 | 3 | 4 | 5 | 6 | 8 | 16) {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "unsupported bits-per-block value: {bits_per_block}"
            )));
        }

        let word_count = packed_word_count(bits_per_block);
        let words_byte_len = word_count.checked_mul(4).ok_or_else(|| {
            BedrockWorldError::UnsupportedChunkFormat("palette word count overflowed".to_string())
        })?;
        let words_bytes = bytes.get(offset..offset + words_byte_len).ok_or_else(|| {
            BedrockWorldError::UnsupportedChunkFormat(
                "palette block indices are truncated".to_string(),
            )
        })?;
        offset += words_byte_len;

        let palette_len = if bits_per_block == 0 {
            1
        } else {
            let palette_len = read_i32_at(bytes, offset)?;
            offset += 4;
            if palette_len < 0 {
                return Err(BedrockWorldError::UnsupportedChunkFormat(
                    "palette length cannot be negative".to_string(),
                ));
            }
            let palette_len = usize::try_from(palette_len).map_err(|_| {
                BedrockWorldError::UnsupportedChunkFormat("palette length overflowed".to_string())
            })?;
            if palette_len > MAX_SUBCHUNK_PALETTE_LEN {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "palette length {palette_len} exceeds maximum {MAX_SUBCHUNK_PALETTE_LEN}"
                )));
            }
            palette_len
        };
        let mut states = Vec::with_capacity(palette_len);
        for _ in 0..palette_len {
            let (tag, consumed) = parse_root_nbt_with_consumed(&bytes[offset..])?;
            offset += consumed;
            states.push(block_state_from_nbt(tag));
        }
        let mut counts = matches!(
            mode,
            SubChunkDecodeMode::CountsOnly | SubChunkDecodeMode::FullIndices
        )
        .then(|| vec![0_u16; palette_len]);
        let (indices, packed_indices) = match mode {
            SubChunkDecodeMode::FullIndices => {
                let indices = unpack_palette_indices(words_bytes, bits_per_block, palette_len)?;
                for index in &indices {
                    if let Some(count) = counts
                        .as_mut()
                        .and_then(|counts| counts.get_mut(usize::from(*index)))
                    {
                        *count = count.saturating_add(1);
                    }
                }
                (Some(indices), None)
            }
            SubChunkDecodeMode::CountsOnly => {
                count_packed_palette_indices(
                    words_bytes,
                    bits_per_block,
                    palette_len,
                    counts.as_deref_mut().ok_or_else(|| {
                        BedrockWorldError::Validation(
                            "counts-only decode did not allocate palette counts".to_string(),
                        )
                    })?,
                )?;
                (None, None)
            }
            SubChunkDecodeMode::SurfaceColumns | SubChunkDecodeMode::PackedIndices => (
                None,
                Some(PackedPaletteIndices {
                    bytes: Bytes::copy_from_slice(words_bytes),
                    bits_per_block,
                    palette_len,
                }),
            ),
        };
        storages.push(BlockPalette {
            states,
            indices,
            packed_indices,
            counts,
        });
    }
    Ok((storages, offset))
}

fn count_packed_palette_indices(
    words_bytes: &[u8],
    bits_per_block: u8,
    palette_len: usize,
    counts: &mut [u16],
) -> Result<()> {
    if bits_per_block == 0 {
        if let Some(count) = counts.first_mut() {
            *count = 4096;
        }
        return Ok(());
    }
    let values_per_word = usize::from(32 / bits_per_block);
    let mask = (1_u32 << bits_per_block) - 1;
    let mut decoded = 0usize;
    for word_bytes in words_bytes.chunks_exact(4) {
        let word = u32::from_le_bytes(
            word_bytes
                .try_into()
                .map_err(|_| BedrockWorldError::CorruptWorld("bad palette word".to_string()))?,
        );
        for item_index in 0..values_per_word {
            if decoded == 4096 {
                return Ok(());
            }
            let value = ((word >> (item_index * usize::from(bits_per_block))) & mask) as u16;
            if usize::from(value) >= palette_len {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "palette index {value} exceeds palette length {palette_len}"
                )));
            }
            if let Some(count) = counts.get_mut(usize::from(value)) {
                *count = count.saturating_add(1);
            }
            decoded = decoded.saturating_add(1);
        }
    }
    if decoded == 4096 {
        Ok(())
    } else {
        Err(BedrockWorldError::UnsupportedChunkFormat(
            "palette block indices are truncated".to_string(),
        ))
    }
}

fn packed_word_count(bits_per_block: u8) -> usize {
    if bits_per_block == 0 {
        return 0;
    }
    let values_per_word = usize::from(32 / bits_per_block);
    4096_usize.div_ceil(values_per_word)
}

fn unpack_palette_indices(
    words_bytes: &[u8],
    bits_per_block: u8,
    palette_len: usize,
) -> Result<Vec<u16>> {
    if bits_per_block == 0 {
        return Ok(vec![0; 4096]);
    }
    let values_per_word = usize::from(32 / bits_per_block);
    let mask = (1_u32 << bits_per_block) - 1;
    let mut indices = Vec::with_capacity(4096);
    for word_bytes in words_bytes.chunks_exact(4) {
        let word = u32::from_le_bytes(
            word_bytes
                .try_into()
                .map_err(|_| BedrockWorldError::CorruptWorld("bad palette word".to_string()))?,
        );
        for item_index in 0..values_per_word {
            if indices.len() == 4096 {
                break;
            }
            let value = ((word >> (item_index * usize::from(bits_per_block))) & mask) as u16;
            if palette_len > 0 && usize::from(value) >= palette_len {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "palette index {value} exceeds palette length {palette_len}"
                )));
            }
            indices.push(value);
        }
    }
    if indices.len() != 4096 {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "palette produced {} block indices instead of 4096",
            indices.len()
        )));
    }
    Ok(indices)
}

fn block_state_from_nbt(tag: NbtTag) -> BlockState {
    let NbtTag::Compound(root) = tag else {
        return BlockState {
            name: "<invalid>".to_string(),
            states: BTreeMap::new(),
            version: None,
        };
    };
    block_state_from_nbt_root(root)
}

fn block_state_from_nbt_root(root: IndexMap<String, NbtTag>) -> BlockState {
    let mut name = None;
    let mut fallback_name = None;
    let mut states_tag = None;
    let mut fallback_states_tag = None;
    let mut saw_states_tag = false;
    let mut version = None;
    let mut fallback_version = None;
    for (key, value) in root {
        match (key.as_str(), value) {
            ("name", NbtTag::String(value)) => name = Some(value),
            ("Name", NbtTag::String(value)) => fallback_name = Some(value),
            ("states", value) => {
                saw_states_tag = true;
                states_tag = Some(value);
            }
            ("States", value) => fallback_states_tag = Some(value),
            ("version", value) => version = int_from_tag(value),
            ("Version", value) => fallback_version = int_from_tag(value),
            _ => {}
        }
    }
    let name = name
        .or(fallback_name)
        .unwrap_or_else(|| "<unknown>".to_string());
    let states = match if saw_states_tag {
        states_tag
    } else {
        fallback_states_tag
    } {
        Some(NbtTag::Compound(values)) => values.into_iter().collect(),
        _ => BTreeMap::new(),
    };
    let version = version.or(fallback_version);
    BlockState {
        name,
        states,
        version,
    }
}

fn int_from_tag(tag: NbtTag) -> Option<i32> {
    match tag {
        NbtTag::Byte(value) => Some(i32::from(value)),
        NbtTag::Short(value) => Some(i32::from(value)),
        NbtTag::Int(value) => Some(value),
        _ => None,
    }
}

fn read_i32_at(bytes: &[u8], offset: usize) -> Result<i32> {
    let slice: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| {
            BedrockWorldError::UnsupportedChunkFormat("i32 field is truncated".to_string())
        })?
        .try_into()
        .map_err(|_| BedrockWorldError::UnsupportedChunkFormat("bad i32 field".to_string()))?;
    Ok(i32::from_le_bytes(slice))
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| BedrockWorldError::InvalidKey("chunk key is truncated".to_string()))?;
    let slice: [u8; 4] = slice
        .try_into()
        .map_err(|_| BedrockWorldError::InvalidKey("invalid i32 field".to_string()))?;
    Ok(i32::from_le_bytes(slice))
}

fn parse_i64_suffix(key: &[u8], prefix: &[u8]) -> Option<i64> {
    let suffix = key.strip_prefix(prefix)?;
    let bytes: [u8; 8] = suffix.try_into().ok()?;
    Some(i64::from_le_bytes(bytes))
}

fn parse_chunk_pos_suffix(key: &[u8], prefix: &[u8]) -> Option<ChunkPos> {
    let suffix = key.strip_prefix(prefix)?;
    match suffix.len() {
        8 => Some(ChunkPos {
            x: read_i32_optional(suffix, 0)?,
            z: read_i32_optional(suffix, 4)?,
            dimension: Dimension::Overworld,
        }),
        12 => Some(ChunkPos {
            x: read_i32_optional(suffix, 0)?,
            z: read_i32_optional(suffix, 4)?,
            dimension: Dimension::from_id(read_i32_optional(suffix, 8)?),
        }),
        _ => None,
    }
}

fn read_i32_optional(bytes: &[u8], offset: usize) -> Option<i32> {
    let slice: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(i32::from_le_bytes(slice))
}

fn nibble_at(bytes: &[u8], index: usize) -> Option<u8> {
    let byte = *bytes.get(index / 2)?;
    Some(if index.is_multiple_of(2) {
        byte & 0x0f
    } else {
        byte >> 4
    })
}

fn ascii_suffix(key: &[u8], prefix: &[u8]) -> Option<String> {
    let suffix = key.strip_prefix(prefix)?;
    if suffix.iter().all(u8::is_ascii_graphic) {
        return Some(String::from_utf8_lossy(suffix).into_owned());
    }
    None
}

fn parse_village_key(key: &[u8]) -> Option<ParsedVillageKey> {
    let raw = std::str::from_utf8(key).ok()?;
    let parts = raw.split('_').collect::<Vec<_>>();
    if !matches!(parts.as_slice(), ["VILLAGE", ..]) || !matches!(parts.len(), 3 | 4) {
        return None;
    }
    let (dimension, tail) = match parts.as_slice() {
        ["VILLAGE", dimension, _, _] => {
            let dimension = match *dimension {
                "Overworld" => Dimension::Overworld,
                "Nether" => Dimension::Nether,
                "TheEnd" => Dimension::End,
                _ => return None,
            };
            (Some(dimension), &parts[2..])
        }
        ["VILLAGE", _, _] => (None, &parts[1..]),
        _ => return None,
    };
    let uuid = tail[0];
    if uuid.len() != 36 {
        return None;
    }
    let kind = match tail[1] {
        "INFO" => VillageRecordKind::Info,
        "DWELLERS" => VillageRecordKind::Dwellers,
        "PLAYERS" => VillageRecordKind::Players,
        "POI" => VillageRecordKind::Poi,
        _ => VillageRecordKind::Unknown,
    };
    Some(ParsedVillageKey {
        raw: raw.to_string(),
        dimension,
        uuid: uuid.to_string(),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::serialize_root_nbt;

    #[test]
    fn chunk_key_roundtrips_overworld_and_subchunk() {
        let pos = ChunkPos {
            x: -3,
            z: 7,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, -4);
        let encoded = key.encode();

        assert_eq!(encoded.len(), 10);
        assert_eq!(ChunkKey::decode(&encoded).expect("decode"), key);
        assert_eq!(key.encode_inline().as_bytes(), encoded.as_ref());
        assert_eq!(
            BedrockDbKeyKind::classify(&encoded),
            BedrockDbKeyKind::Chunk(ChunkRecordTag::SubChunkPrefix)
        );
    }

    #[test]
    fn key_kind_classification_handles_common_non_chunk_keys_without_decoding() {
        assert_eq!(
            BedrockDbKeyKind::classify(b"~local_player"),
            BedrockDbKeyKind::LocalPlayer
        );
        assert_eq!(
            BedrockDbKeyKind::classify(b"player_123"),
            BedrockDbKeyKind::RemotePlayer
        );
        assert_eq!(
            BedrockDbKeyKind::classify(b"actorprefix12345678"),
            BedrockDbKeyKind::ActorPrefix
        );
        assert_eq!(
            BedrockDbKeyKind::classify(b"unclassified"),
            BedrockDbKeyKind::Other
        );
    }

    #[test]
    fn chunk_key_roundtrips_dimension_key() {
        let pos = ChunkPos {
            x: 1,
            z: 2,
            dimension: Dimension::Nether,
        };
        let key = ChunkKey::new(pos, ChunkRecordTag::Version);
        let encoded = key.encode();

        assert_eq!(encoded.len(), 13);
        assert_eq!(ChunkKey::decode(&encoded).expect("decode"), key);
    }

    #[test]
    fn bedrock_db_key_decodes_actor_and_digp_keys() {
        let mut actor_key = b"actorprefix".to_vec();
        actor_key.extend_from_slice(&42_i64.to_le_bytes());
        assert_eq!(
            BedrockDbKey::decode(&actor_key),
            BedrockDbKey::ActorPrefix { actor_id: 42 }
        );

        let mut digp_key = b"digp".to_vec();
        digp_key.extend_from_slice(&1_i32.to_le_bytes());
        digp_key.extend_from_slice(&(-2_i32).to_le_bytes());
        assert_eq!(
            BedrockDbKey::decode(&digp_key),
            BedrockDbKey::ActorDigest {
                pos: ChunkPos {
                    x: 1,
                    z: -2,
                    dimension: Dimension::Overworld
                }
            }
        );
    }

    #[test]
    fn bedrock_db_key_encodes_documented_global_shapes() {
        let map_id = MapRecordId::new("42").expect("map id");
        assert_eq!(map_id.storage_key().as_ref(), b"map_42");
        assert_eq!(
            MapRecordId::from_storage_key(b"map_42"),
            Some(map_id.clone())
        );
        assert_eq!(
            BedrockDbKey::Map("42".to_string()).encode().as_deref(),
            Some(&b"map_42"[..])
        );

        let pos = ChunkPos {
            x: 7,
            z: -8,
            dimension: Dimension::End,
        };
        let digest = ActorDigestKey::new(pos).storage_key();
        assert_eq!(
            ActorDigestKey::from_storage_key(&digest),
            Some(ActorDigestKey::new(pos))
        );
        assert_eq!(
            BedrockDbKey::Global(GlobalRecordKind::Scoreboard)
                .encode()
                .as_deref(),
            Some(&b"scoreboard"[..])
        );
        assert_eq!(
            BedrockDbKey::decode(b"TheEnd"),
            BedrockDbKey::Global(GlobalRecordKind::Dimension(Dimension::End))
        );
    }

    #[test]
    fn chunk_record_tags_align_with_bedrock_level_reference() {
        let expected = [
            (0x2b, ChunkRecordTag::Data3D),
            (0x2c, ChunkRecordTag::Version),
            (0x2d, ChunkRecordTag::Data2D),
            (0x2e, ChunkRecordTag::Data2DLegacy),
            (0x2f, ChunkRecordTag::SubChunkPrefix),
            (0x30, ChunkRecordTag::LegacyTerrain),
            (0x31, ChunkRecordTag::BlockEntity),
            (0x32, ChunkRecordTag::Entity),
            (0x33, ChunkRecordTag::PendingTicks),
            (0x34, ChunkRecordTag::BlockExtraData),
            (0x35, ChunkRecordTag::BiomeState),
            (0x36, ChunkRecordTag::FinalizedState),
            (0x37, ChunkRecordTag::ConversionData),
            (0x38, ChunkRecordTag::BorderBlocks),
            (0x39, ChunkRecordTag::HardcodedSpawners),
            (0x3a, ChunkRecordTag::RandomTicks),
            (0x3b, ChunkRecordTag::Checksums),
            (0x3c, ChunkRecordTag::GenerationSeed),
            (0x3d, ChunkRecordTag::GeneratedPreCavesAndCliffsBlending),
            (0x3e, ChunkRecordTag::BlendingBiomeHeight),
            (0x3f, ChunkRecordTag::MetaDataHash),
            (0x40, ChunkRecordTag::BlendingData),
            (0x41, ChunkRecordTag::ActorDigestVersion),
            (0x76, ChunkRecordTag::VersionOld),
        ];
        for (byte, tag) in expected {
            assert_eq!(ChunkRecordTag::from_byte(byte), tag);
            assert_eq!(tag.byte(), byte);
        }
    }

    #[test]
    fn bedrock_db_key_decodes_specific_ascii_keys_before_plain_keys() {
        assert_eq!(
            BedrockDbKey::decode(b"map_42"),
            BedrockDbKey::Map("42".to_string())
        );
        assert!(matches!(
            BedrockDbKey::decode(b"VILLAGE_12345678-1234-1234-1234-123456789abc_INFO"),
            BedrockDbKey::Village(_)
        ));
        assert!(matches!(
            BedrockDbKey::decode(b"LevelChunkMetaDataDictionary"),
            BedrockDbKey::Global(GlobalRecordKind::LevelChunkMetaDataDictionary)
        ));
    }

    #[test]
    fn chunk_pos_matches_bedrock_level_height_ranges() {
        let overworld = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        assert_eq!(overworld.y_range(ChunkVersion::Old), (0, 255));
        assert_eq!(overworld.y_range(ChunkVersion::New), (-64, 319));
        assert_eq!(overworld.subchunk_index_range(ChunkVersion::New), (-4, 19));
        assert_eq!(
            BlockPos {
                x: -1,
                y: 64,
                z: -1
            }
            .to_chunk_pos(Dimension::Overworld),
            ChunkPos {
                x: -1,
                z: -1,
                dimension: Dimension::Overworld
            }
        );
    }

    #[test]
    fn legacy_terrain_exposes_old_leveldb_arrays() {
        let mut bytes = vec![0; LEGACY_TERRAIN_VALUE_LEN];
        let block_index = LegacyTerrain::block_index(1, 2, 3).expect("block index");
        let column_index = 3 * 16 + 1;
        assert_eq!(block_index, 2_434);
        assert_eq!(LegacyTerrain::column_index(1, 3), Some(column_index));
        bytes[block_index] = 42;
        bytes[LEGACY_TERRAIN_BLOCK_DATA_OFFSET + block_index / 2] = 0xba;
        bytes[LEGACY_TERRAIN_SKY_LIGHT_OFFSET + block_index / 2] = 0xc7;
        bytes[LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET + block_index / 2] = 0xd5;
        bytes[LEGACY_TERRAIN_HEIGHTMAP_OFFSET + column_index] = 99;
        bytes[LEGACY_TERRAIN_BIOME_OFFSET + column_index * 4
            ..LEGACY_TERRAIN_BIOME_OFFSET + column_index * 4 + 4]
            .copy_from_slice(&[12, 0xab, 0xcd, 0xef]);

        let terrain = LegacyTerrain::parse(Bytes::from(bytes)).expect("legacy terrain");

        assert_eq!(terrain.block_id_at(1, 2, 3), Some(42));
        assert_eq!(terrain.block_data_at(1, 2, 3), Some(0x0a));
        assert_eq!(terrain.sky_light_at(1, 2, 3), Some(0x07));
        assert_eq!(terrain.block_light_at(1, 2, 3), Some(0x05));
        assert_eq!(terrain.height_at(1, 3), Some(99));
        assert_eq!(terrain.biome_color_at(1, 3), Some(0x00ab_cdef));
        assert_eq!(
            terrain.biome_sample_at(1, 3),
            Some(LegacyBiomeSample {
                biome_id: 12,
                red: 0xab,
                green: 0xcd,
                blue: 0xef,
            })
        );
        assert!(LegacyTerrain::parse(Bytes::from_static(b"short")).is_err());
    }

    #[test]
    fn legacy_subchunk_decodes_block_ids_metadata_and_light() {
        let mut bytes = vec![0; LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN];
        bytes[0] = 2;
        let index = LegacySubChunk::block_index(4, 5, 6).expect("block index");
        assert_eq!(index, 1_125);
        bytes[1 + index] = 7;
        bytes[1 + LEGACY_SUBCHUNK_BLOCK_COUNT + index / 2] = 0xc0;
        bytes[1 + LEGACY_SUBCHUNK_BLOCK_COUNT + LEGACY_SUBCHUNK_BLOCK_COUNT / 2 + index / 2] = 0xe0;
        bytes[1 + LEGACY_SUBCHUNK_BLOCK_COUNT + LEGACY_SUBCHUNK_BLOCK_COUNT + index / 2] = 0xa0;

        let subchunk = parse_subchunk(0, Bytes::from(bytes)).expect("parse legacy subchunk");

        let SubChunkFormat::LegacySubChunk(legacy) = &subchunk.format else {
            panic!("expected legacy subchunk");
        };
        assert_eq!(legacy.version(), 2);
        assert_eq!(legacy.block_id_at(4, 5, 6), Some(7));
        assert_eq!(legacy.block_data_at(4, 5, 6), Some(0x0c));
        assert_eq!(legacy.sky_light_at(4, 5, 6), Some(0x0e));
        assert_eq!(legacy.block_light_at(4, 5, 6), Some(0x0a));
        assert_eq!(subchunk.legacy_block_id_at(4, 5, 6), Some(7));
    }

    #[test]
    fn paletted_subchunk_v1_uses_single_storage_without_count_byte() {
        let mut bytes = build_paletted_subchunk(8, None, 4, 4);
        bytes.remove(1);
        bytes[0] = 1;

        let subchunk = parse_subchunk(0, Bytes::from(bytes)).expect("parse v1 palette");

        let SubChunkFormat::Paletted { version, storages } = subchunk.format else {
            panic!("expected v1 paletted subchunk");
        };
        assert_eq!(version, 1);
        assert_eq!(storages.len(), 1);
        assert_eq!(storages[0].indices.as_ref().expect("indices").len(), 4096);
    }

    #[test]
    fn paletted_subchunk_decodes_supported_bits_per_block() {
        for bits_per_block in [0, 1, 2, 3, 4, 5, 6, 8, 16] {
            let bytes = build_paletted_subchunk(8, None, bits_per_block, 4);

            let subchunk = parse_subchunk(0, Bytes::from(bytes)).expect("parse");

            let SubChunkFormat::Paletted { storages, .. } = subchunk.format else {
                panic!("expected paletted subchunk for {bits_per_block} bits");
            };
            assert_eq!(storages.len(), 1);
            assert_eq!(storages[0].indices.as_ref().expect("indices").len(), 4096);
            assert_eq!(
                storages[0]
                    .counts
                    .as_ref()
                    .expect("full indices retain counts")
                    .iter()
                    .sum::<u16>(),
                4096
            );
        }
    }

    #[test]
    fn paletted_subchunk_counts_only_drops_indices_but_keeps_counts() {
        let bytes = build_paletted_subchunk(8, None, 4, 4);

        let subchunk =
            parse_subchunk_with_mode(0, Bytes::from(bytes), SubChunkDecodeMode::CountsOnly)
                .expect("parse");

        let SubChunkFormat::Paletted { storages, .. } = subchunk.format else {
            panic!("expected paletted subchunk");
        };
        assert!(storages[0].indices.is_none());
        assert_eq!(
            storages[0]
                .counts
                .as_ref()
                .expect("counts-only retains counts")
                .iter()
                .sum::<u16>(),
            4096
        );
    }

    #[test]
    fn surface_columns_keep_random_access_without_full_indices() {
        let bytes = build_paletted_subchunk(8, None, 4, 4);
        let full = parse_subchunk_with_mode(
            0,
            Bytes::from(bytes.clone()),
            SubChunkDecodeMode::FullIndices,
        )
        .expect("parse full indices");
        let surface =
            parse_subchunk_with_mode(0, Bytes::from(bytes), SubChunkDecodeMode::SurfaceColumns)
                .expect("parse surface columns");

        let SubChunkFormat::Paletted {
            storages: surface_storages,
            ..
        } = &surface.format
        else {
            panic!("expected surface paletted subchunk");
        };
        assert!(surface_storages[0].indices.is_none());
        assert!(surface_storages[0].packed_indices.is_some());
        assert!(surface_storages[0].counts.is_none());

        for (x, y, z) in [(0, 0, 0), (1, 2, 3), (15, 15, 15), (7, 9, 4)] {
            assert_eq!(
                full.block_state_at(x, y, z),
                surface.block_state_at(x, y, z)
            );
        }
    }

    #[test]
    fn paletted_subchunk_v9_accepts_embedded_y_byte() {
        let bytes = build_paletted_subchunk(9, Some(-4), 4, 4);

        let subchunk = parse_subchunk(-4, Bytes::from(bytes)).expect("parse");

        let SubChunkFormat::Paletted { storages, .. } = subchunk.format else {
            panic!("expected paletted v9 subchunk");
        };
        assert_eq!(storages[0].states.len(), 4);
    }

    #[test]
    fn paletted_subchunk_v9_accepts_positive_embedded_y_that_looks_like_storage_header() {
        let bytes = build_paletted_subchunk(9, Some(8), 4, 4);

        let subchunk = parse_subchunk(8, Bytes::from(bytes)).expect("parse");

        let SubChunkFormat::Paletted { storages, .. } = &subchunk.format else {
            panic!("expected paletted v9 subchunk");
        };
        assert_eq!(storages[0].states.len(), 4);
        assert_eq!(
            subchunk
                .block_state_at(1, 2, 3)
                .expect("block state at x=1 y=2 z=3")
                .name,
            "minecraft:block_2"
        );
    }

    #[test]
    fn paletted_subchunk_v9_falls_back_to_legacy_layout_without_embedded_y() {
        let bytes = build_paletted_subchunk(9, None, 4, 4);

        let subchunk = parse_subchunk(8, Bytes::from(bytes)).expect("parse");

        let SubChunkFormat::Paletted { storages, .. } = &subchunk.format else {
            panic!("expected paletted v9 subchunk");
        };
        assert_eq!(storages[0].states.len(), 4);
        assert_eq!(
            subchunk
                .block_state_at(1, 2, 3)
                .expect("block state at x=1 y=2 z=3")
                .name,
            "minecraft:block_2"
        );
    }

    #[test]
    fn paletted_subchunk_rejects_trailing_bytes_after_storage_payload() {
        let mut bytes = build_paletted_subchunk(8, None, 4, 4);
        bytes.push(0);

        let subchunk = parse_subchunk(0, Bytes::from(bytes)).expect("parse");

        assert!(matches!(subchunk.format, SubChunkFormat::Raw { .. }));
    }

    #[test]
    fn block_state_lookup_uses_xz_plane_storage_order() {
        let bytes = build_paletted_subchunk(8, None, 4, 8);
        let subchunk = parse_subchunk(0, Bytes::from(bytes)).expect("parse");

        assert_eq!(block_storage_index(1, 2, 3), 306);
        let state = subchunk
            .block_state_at(1, 2, 3)
            .expect("block state at x=1 y=2 z=3");

        assert_eq!(
            state.name,
            format!("minecraft:block_{}", block_storage_index(1, 2, 3) % 8)
        );
    }

    #[test]
    fn visible_block_state_lookup_uses_top_non_air_storage() {
        let subchunk = parse_subchunk(
            0,
            Bytes::from(build_two_storage_paletted_subchunk(
                "minecraft:stone",
                "minecraft:copper_block",
            )),
        )
        .expect("parse layered subchunk");

        assert_eq!(
            subchunk
                .block_state_at(1, 2, 3)
                .expect("storage zero state")
                .name,
            "minecraft:stone"
        );
        let visible = subchunk
            .visible_block_states_at(1, 2, 3)
            .map(|state| state.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(visible, ["minecraft:copper_block", "minecraft:stone"]);
        assert_eq!(
            subchunk
                .visible_block_state_at(1, 2, 3)
                .expect("visible state")
                .name,
            "minecraft:copper_block"
        );
    }

    #[test]
    fn visible_surface_state_iterator_reports_palette_positions() {
        let mut bytes = vec![8, 3];
        append_test_palette_storage(
            &mut bytes,
            &["minecraft:air", "minecraft:water"],
            |x, y, z| u16::from((x, y, z) == (1, 2, 3)),
        );
        append_test_palette_storage(
            &mut bytes,
            &["minecraft:air", "minecraft:short_grass"],
            |x, y, z| u16::from((x, y, z) == (1, 2, 3)),
        );
        append_test_palette_storage(
            &mut bytes,
            &["minecraft:air", "minecraft:stone"],
            |x, y, z| u16::from((x, y, z) == (1, 2, 3)),
        );

        let subchunk = parse_subchunk(0, Bytes::from(bytes)).expect("parse layered subchunk");
        let SubChunkFormat::Paletted { storages, .. } = &subchunk.format else {
            panic!("expected paletted subchunk");
        };

        assert_eq!(storages.len(), 3);
        let visible_entries = subchunk
            .visible_block_surface_states_at(1, 2, 3)
            .map(|entry| (entry.storage_index, entry.state.name.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            visible_entries,
            [
                (2, "minecraft:stone"),
                (1, "minecraft:short_grass"),
                (0, "minecraft:water")
            ]
        );
    }

    #[test]
    fn paletted_subchunk_v9_decodes_zero_bit_secondary_storage_without_palette_len() {
        let mut bytes = vec![9, 2, 4];
        append_test_palette_storage(
            &mut bytes,
            &["minecraft:air", "minecraft:stone"],
            |x, y, z| u16::from((x, y, z) == (4, 2, 4)),
        );
        append_zero_bit_palette_storage(&mut bytes, "minecraft:gold_block");

        let subchunk = parse_subchunk(4, Bytes::from(bytes)).expect("parse v9 layered subchunk");

        let SubChunkFormat::Paletted { storages, .. } = &subchunk.format else {
            panic!("expected paletted subchunk");
        };
        assert_eq!(storages.len(), 2);
        assert_eq!(storages[1].states.len(), 1);
        assert_eq!(storages[1].counts.as_deref(), Some(&[4096][..]));
        assert_eq!(
            subchunk
                .block_state_at(4, 2, 4)
                .expect("storage zero state")
                .name,
            "minecraft:stone"
        );
        assert_eq!(
            subchunk
                .visible_block_state_at(4, 2, 4)
                .expect("visible state")
                .name,
            "minecraft:gold_block"
        );
    }

    #[test]
    fn chunk_get_block_reads_decoded_paletted_subchunk() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::subchunk(pos, 0);
        let chunk = Chunk {
            pos,
            version: Some(8),
            records: vec![ChunkRecord {
                key,
                value: Bytes::from(build_paletted_subchunk(8, None, 4, 8)),
            }],
        };

        let state = chunk.get_block(1, 2, 3).expect("block state");

        assert_eq!(state.name, "minecraft:block_2");
    }

    fn build_paletted_subchunk(
        version: u8,
        embedded_y: Option<i8>,
        bits_per_block: u8,
        palette_len: usize,
    ) -> Vec<u8> {
        let palette_len = if bits_per_block == 0 { 1 } else { palette_len };
        let mut bytes = vec![version, 1];
        if let Some(y) = embedded_y {
            bytes.push(y as u8);
        }
        bytes.push(bits_per_block << 1);
        let values_per_word = 32_usize
            .checked_div(usize::from(bits_per_block))
            .unwrap_or(4096);
        let mut words = vec![0_u32; packed_word_count(bits_per_block)];
        if bits_per_block != 0 {
            for block_index in 0..4096 {
                let value = u32::try_from(block_index % palette_len).expect("palette index");
                let word_index = block_index / values_per_word;
                let bit_offset = (block_index % values_per_word) * usize::from(bits_per_block);
                words[word_index] |= value << bit_offset;
            }
        }
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        if bits_per_block != 0 {
            bytes.extend_from_slice(
                &i32::try_from(palette_len)
                    .expect("palette length")
                    .to_le_bytes(),
            );
        }
        for index in 0..palette_len {
            let tag = NbtTag::Compound(IndexMap::from([
                (
                    "name".to_string(),
                    NbtTag::String(format!("minecraft:block_{index}")),
                ),
                ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                ("version".to_string(), NbtTag::Int(1)),
            ]));
            bytes.extend_from_slice(&serialize_root_nbt(&tag).expect("serialize palette"));
        }
        bytes
    }

    fn append_zero_bit_palette_storage(bytes: &mut Vec<u8>, name: &str) {
        bytes.push(0);
        let tag = NbtTag::Compound(IndexMap::from([
            ("name".to_string(), NbtTag::String(name.to_string())),
            ("states".to_string(), NbtTag::Compound(IndexMap::new())),
            ("version".to_string(), NbtTag::Int(1)),
        ]));
        bytes.extend_from_slice(&serialize_root_nbt(&tag).expect("serialize palette"));
    }

    fn build_two_storage_paletted_subchunk(lower_name: &str, upper_name: &str) -> Vec<u8> {
        let mut bytes = vec![8, 2];
        append_test_palette_storage(&mut bytes, &["minecraft:air", lower_name], |x, y, z| {
            u16::from((x, y, z) == (1, 2, 3))
        });
        append_test_palette_storage(&mut bytes, &["minecraft:air", upper_name], |x, y, z| {
            u16::from((x, y, z) == (1, 2, 3))
        });
        bytes
    }

    fn append_test_palette_storage(
        bytes: &mut Vec<u8>,
        palette: &[&str],
        value_at: impl Fn(u8, u8, u8) -> u16,
    ) {
        let bits_per_block = 1_u8;
        let values_per_word = usize::from(32 / bits_per_block);
        let mut words = vec![0_u32; packed_word_count(bits_per_block)];
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                for local_y in 0..16_u8 {
                    let value = value_at(local_x, local_y, local_z);
                    if value == 0 {
                        continue;
                    }
                    let block_index = block_storage_index(local_x, local_y, local_z);
                    let word_index = block_index / values_per_word;
                    let bit_offset = (block_index % values_per_word) * usize::from(bits_per_block);
                    words[word_index] |= u32::from(value) << bit_offset;
                }
            }
        }
        bytes.push(bits_per_block << 1);
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(
            &i32::try_from(palette.len())
                .expect("test palette length")
                .to_le_bytes(),
        );
        for name in palette {
            let tag = NbtTag::Compound(IndexMap::from([
                ("name".to_string(), NbtTag::String((*name).to_string())),
                ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                ("version".to_string(), NbtTag::Int(1)),
            ]));
            bytes.extend_from_slice(&serialize_root_nbt(&tag).expect("serialize palette"));
        }
    }
}

#[cfg(test)]
mod actor_storage_key_tests {
    use super::ActorUid;

    #[test]
    fn unique_id_is_not_used_as_raw_actorprefix_suffix() {
        let unique_id = 0x0000_0002_1234_5678_i64;
        let storage = ActorUid::from_unique_id(unique_id);
        let expected_numeric = ((0xffff_ffff_u64 - 2) << 32) | 0x1234_5678;
        assert_eq!(storage.raw_storage_bytes(), expected_numeric.to_be_bytes());
        assert_ne!(storage.raw_storage_bytes(), unique_id.to_le_bytes());
    }
}
