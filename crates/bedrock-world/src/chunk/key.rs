//! Bedrock LevelDB keys used by chunks and other world records.

use super::position::{ChunkPos, Dimension};
use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Bedrock chunk record tag byte used in LevelDB chunk keys.
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
/// Classified Bedrock LevelDB key.
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
    pub fn unchecked(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    /// Returns the id suffix without the `map_` storage prefix.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    /// Encodes this id as the LevelDB key `map_<id>`.
    pub fn storage_key(&self) -> Bytes {
        Bytes::from(format!("map_{}", self.0))
    }

    #[must_use]
    /// Decodes a LevelDB map key into an id suffix.
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
pub struct ActorUid(pub i64);

impl ActorUid {
    #[must_use]
    /// Derives the modern actor storage token from the NBT `UniqueID`.
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
/// Known non-chunk global records in a Bedrock LevelDB world.
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
    /// Encodes this global kind as an exact LevelDB key.
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
