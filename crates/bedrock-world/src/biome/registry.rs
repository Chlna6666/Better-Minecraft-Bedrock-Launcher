//! Versioned Minecraft Bedrock biome registry snapshots embedded as a compact binary table.
//!
//! Persisted `Data2D`, `Data2DLegacy`, and `Data3D` biome IDs remain raw world facts. This module
//! only interprets an ID against an explicitly selected Minecraft version; registry lookup never
//! rewrites world data and never falls back to a newer or older snapshot.

use crate::error::{BedrockWorldError, Result};
use crate::version::GameVersion;
use std::cmp::Ordering;
use std::sync::OnceLock;
use xxhash_rust::xxh3::xxh3_64;

const MAGIC: &[u8; 8] = b"BWRBIO01";
const FORMAT_VERSION: u16 = 1;
const HEADER_LEN: usize = 28;
const SNAPSHOT_RECORD_LEN: usize = 28;
const BIOME_RECORD_LEN: usize = 40;
const NAME_INDEX_RECORD_LEN: usize = 12;
const UNKNOWN_NETWORK_VERSION: u32 = u32::MAX;

const HAS_TEMPERATURE: u16 = 1 << 0;
const HAS_DOWNFALL: u16 = 1 << 1;
const HAS_FOLIAGE_SNOW: u16 = 1 << 2;
const HAS_DEPTH: u16 = 1 << 3;
const HAS_SCALE: u16 = 1 << 4;
const HAS_MAP_WATER_COLOR: u16 = 1 << 5;
const HAS_RAIN: u16 = 1 << 6;
const RAIN_VALUE: u16 = 1 << 7;
const KNOWN_FLAGS: u16 = HAS_TEMPERATURE
    | HAS_DOWNFALL
    | HAS_FOLIAGE_SNOW
    | HAS_DEPTH
    | HAS_SCALE
    | HAS_MAP_WATER_COLOR
    | HAS_RAIN
    | RAIN_VALUE;

const EMBEDDED_BYTES: &[u8] = include_bytes!("registry.bin");
static EMBEDDED_REGISTRY: OnceLock<std::result::Result<BiomeRegistry<'static>, String>> =
    OnceLock::new();

/// Returns the biome registry compiled into this `bedrock-world` build.
///
/// The packed binary is validated once per process. Lookups operate directly on immutable embedded
/// bytes with binary search; no runtime JSON parsing or whole-registry `HashMap` is created.
pub fn embedded_biome_registry() -> Result<&'static BiomeRegistry<'static>> {
    match EMBEDDED_REGISTRY.get_or_init(|| {
        BiomeRegistry::parse(EMBEDDED_BYTES).map_err(|error| error.to_string())
    }) {
        Ok(registry) => Ok(registry),
        Err(message) => Err(BedrockWorldError::CorruptWorld(format!(
            "embedded biome registry is invalid: {message}"
        ))),
    }
}

/// A validated, allocation-free view over one packed biome registry binary.
#[derive(Debug)]
pub struct BiomeRegistry<'a> {
    bytes: &'a [u8],
    snapshot_count: usize,
    biome_count: usize,
    name_index_count: usize,
    snapshots_offset: usize,
    biomes_offset: usize,
    name_index_offset: usize,
    strings_offset: usize,
    strings_len: usize,
}

impl<'a> BiomeRegistry<'a> {
    /// Parses and validates a packed biome registry binary.
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            return Err(corrupt(format!(
                "biome registry is {} bytes, smaller than the {HEADER_LEN}-byte header",
                bytes.len()
            )));
        }
        if bytes.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
            return Err(corrupt("biome registry magic does not match BWRBIO01"));
        }
        let format_version = read_u16(bytes, 8);
        if format_version != FORMAT_VERSION {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "biome registry format {format_version} is unsupported; expected {FORMAT_VERSION}"
            )));
        }
        if read_u16(bytes, 10) != 0 {
            return Err(corrupt("biome registry reserved header bits are non-zero"));
        }

        let snapshot_count = u32_as_usize(read_u32(bytes, 12), "snapshot count")?;
        let biome_count = u32_as_usize(read_u32(bytes, 16), "biome count")?;
        let name_index_count = u32_as_usize(read_u32(bytes, 20), "name-index count")?;
        let strings_len = u32_as_usize(read_u32(bytes, 24), "string-pool length")?;

        let snapshots_offset = HEADER_LEN;
        let biomes_offset = checked_advance(
            snapshots_offset,
            snapshot_count,
            SNAPSHOT_RECORD_LEN,
            "snapshot table",
        )?;
        let name_index_offset =
            checked_advance(biomes_offset, biome_count, BIOME_RECORD_LEN, "biome table")?;
        let strings_offset = checked_advance(
            name_index_offset,
            name_index_count,
            NAME_INDEX_RECORD_LEN,
            "name-index table",
        )?;
        let expected_len = strings_offset
            .checked_add(strings_len)
            .ok_or_else(|| corrupt("biome registry total length overflowed usize"))?;
        if expected_len != bytes.len() {
            return Err(corrupt(format!(
                "biome registry length is {}, header tables require {expected_len}",
                bytes.len()
            )));
        }

        let registry = Self {
            bytes,
            snapshot_count,
            biome_count,
            name_index_count,
            snapshots_offset,
            biomes_offset,
            name_index_offset,
            strings_offset,
            strings_len,
        };
        registry.validate()?;
        Ok(registry)
    }

    /// Returns the number of Minecraft-version snapshots in this registry.
    #[must_use]
    pub const fn snapshot_count(&self) -> usize {
        self.snapshot_count
    }

    /// Finds the exact registry snapshot for a persisted Minecraft game version.
    ///
    /// Missing trailing components are zero-filled only to match Bedrock version arrays such as
    /// `1.21` with the packed key `[1, 21, 0, 0]`. More than four components, negative values, or
    /// components outside `u16` are not representable and return `None` instead of being guessed.
    #[must_use]
    pub fn snapshot(&self, version: &GameVersion) -> Option<BiomeRegistrySnapshot<'a>> {
        let key = game_version_key(version)?;
        let mut low = 0_usize;
        let mut high = self.snapshot_count;
        while low < high {
            let middle = low + (high - low) / 2;
            let record = snapshot_record(self.bytes, self.snapshots_offset, middle);
            match record.version.cmp(&key) {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => {
                    return Some(BiomeRegistrySnapshot {
                        bytes: self.bytes,
                        biomes_offset: self.biomes_offset,
                        name_index_offset: self.name_index_offset,
                        strings_offset: self.strings_offset,
                        strings_len: self.strings_len,
                        record,
                    });
                }
            }
        }
        None
    }

    fn validate(&self) -> Result<()> {
        let mut previous_version = None::<[u16; 4]>;
        let mut expected_biome_start = 0_usize;
        let mut expected_name_index_start = 0_usize;

        for snapshot_index in 0..self.snapshot_count {
            let snapshot = snapshot_record(self.bytes, self.snapshots_offset, snapshot_index);
            if previous_version.is_some_and(|previous| snapshot.version <= previous) {
                return Err(corrupt(
                    "biome registry snapshots are not strictly sorted by Minecraft version",
                ));
            }
            previous_version = Some(snapshot.version);

            if snapshot.biome_start != expected_biome_start
                || snapshot.name_index_start != expected_name_index_start
            {
                return Err(corrupt("biome registry snapshot ranges are not contiguous"));
            }
            let biome_end = snapshot
                .biome_start
                .checked_add(snapshot.biome_count)
                .ok_or_else(|| corrupt("biome registry snapshot biome range overflowed"))?;
            let name_index_end = snapshot
                .name_index_start
                .checked_add(snapshot.name_index_count)
                .ok_or_else(|| corrupt("biome registry snapshot name-index range overflowed"))?;
            if biome_end > self.biome_count || name_index_end > self.name_index_count {
                return Err(corrupt(
                    "biome registry snapshot range exceeds its backing table",
                ));
            }
            if snapshot.name_index_count != snapshot.biome_count {
                return Err(corrupt(
                    "biome registry snapshot must contain one name index per biome",
                ));
            }

            validate_biomes(
                self.bytes,
                self.biomes_offset,
                self.strings_offset,
                self.strings_len,
                snapshot.biome_start,
                biome_end,
            )?;
            validate_name_index(
                self.bytes,
                self.biomes_offset,
                self.name_index_offset,
                self.strings_offset,
                self.strings_len,
                snapshot.biome_start,
                biome_end,
                snapshot.name_index_start,
                name_index_end,
            )?;

            expected_biome_start = biome_end;
            expected_name_index_start = name_index_end;
        }

        if expected_biome_start != self.biome_count
            || expected_name_index_start != self.name_index_count
        {
            return Err(corrupt(
                "biome registry contains table records not owned by any snapshot",
            ));
        }
        Ok(())
    }
}

/// One exact Minecraft-version view inside a [`BiomeRegistry`].
///
/// This value contains only table offsets and a borrowed byte slice. Copying it does not allocate.
#[derive(Debug, Clone, Copy)]
pub struct BiomeRegistrySnapshot<'a> {
    bytes: &'a [u8],
    biomes_offset: usize,
    name_index_offset: usize,
    strings_offset: usize,
    strings_len: usize,
    record: SnapshotRecord,
}

impl<'a> BiomeRegistrySnapshot<'a> {
    /// Returns the four packed Minecraft version components used to select this snapshot.
    #[must_use]
    pub const fn minecraft_version(&self) -> [u16; 4] {
        self.record.version
    }

    /// Returns the network protocol version when the generator knew it.
    #[must_use]
    pub const fn network_version(&self) -> Option<u32> {
        if self.record.network_version == UNKNOWN_NETWORK_VERSION {
            None
        } else {
            Some(self.record.network_version)
        }
    }

    /// Returns the number of vanilla biome definitions in this snapshot.
    #[must_use]
    pub const fn biome_count(&self) -> usize {
        self.record.biome_count
    }

    /// Finds a vanilla biome by its persisted/runtime numeric ID.
    #[must_use]
    pub fn biome_by_id(&self, id: u32) -> Option<BiomeDefinition<'a>> {
        let mut low = self.record.biome_start;
        let mut high = self.record.biome_start + self.record.biome_count;
        while low < high {
            let middle = low + (high - low) / 2;
            let record = biome_record(self.bytes, self.biomes_offset, middle);
            match record.id.cmp(&id) {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => {
                    return definition_from_record(
                        self.bytes,
                        self.strings_offset,
                        self.strings_len,
                        record,
                    );
                }
            }
        }
        None
    }

    /// Finds a vanilla biome by its full identifier such as `minecraft:plains`.
    #[must_use]
    pub fn biome_by_name(&self, name: &str) -> Option<BiomeDefinition<'a>> {
        let hash = xxh3_64(name.as_bytes());
        let start = self.record.name_index_start;
        let end = start + self.record.name_index_count;
        let mut low = start;
        let mut high = end;
        while low < high {
            let middle = low + (high - low) / 2;
            if name_index_record(self.bytes, self.name_index_offset, middle).hash < hash {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let mut index = low;
        while index < end {
            let entry = name_index_record(self.bytes, self.name_index_offset, index);
            if entry.hash != hash {
                break;
            }
            let record = biome_record(self.bytes, self.biomes_offset, entry.biome_index);
            let definition = definition_from_record(
                self.bytes,
                self.strings_offset,
                self.strings_len,
                record,
            )?;
            if definition.name == name {
                return Some(definition);
            }
            index += 1;
        }
        None
    }
}

/// A vanilla biome definition resolved from a specific Minecraft-version registry snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiomeDefinition<'a> {
    id: u32,
    name: &'a str,
    temperature: Option<f32>,
    downfall: Option<f32>,
    foliage_snow: Option<f32>,
    depth: Option<f32>,
    scale: Option<f32>,
    map_water_color: Option<i32>,
    rain: Option<bool>,
}

impl<'a> BiomeDefinition<'a> {
    /// Returns the numeric biome ID emitted by the source BDS version.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// Returns the full vanilla biome identifier.
    #[must_use]
    pub const fn name(&self) -> &'a str {
        self.name
    }

    /// Returns the BDS temperature value when exported by the source tool.
    #[must_use]
    pub const fn temperature(&self) -> Option<f32> {
        self.temperature
    }

    /// Returns the BDS downfall value when exported by the source tool.
    #[must_use]
    pub const fn downfall(&self) -> Option<f32> {
        self.downfall
    }

    /// Returns the BDS foliage-snow value when exported by the source tool.
    #[must_use]
    pub const fn foliage_snow(&self) -> Option<f32> {
        self.foliage_snow
    }

    /// Returns the BDS depth value when exported by the source tool.
    #[must_use]
    pub const fn depth(&self) -> Option<f32> {
        self.depth
    }

    /// Returns the BDS scale value when exported by the source tool.
    #[must_use]
    pub const fn scale(&self) -> Option<f32> {
        self.scale
    }

    /// Returns the BDS map-water ARGB color when exported by the source tool.
    #[must_use]
    pub const fn map_water_color(&self) -> Option<i32> {
        self.map_water_color
    }

    /// Returns whether the biome supports rain when exported by the source tool.
    #[must_use]
    pub const fn rain(&self) -> Option<bool> {
        self.rain
    }
}

#[derive(Debug, Clone, Copy)]
struct SnapshotRecord {
    version: [u16; 4],
    network_version: u32,
    biome_start: usize,
    biome_count: usize,
    name_index_start: usize,
    name_index_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct BiomeRecord {
    id: u32,
    name_offset: usize,
    name_len: usize,
    flags: u16,
    temperature: f32,
    downfall: f32,
    foliage_snow: f32,
    depth: f32,
    scale: f32,
    map_water_color: i32,
    reserved: u32,
}

#[derive(Debug, Clone, Copy)]
struct NameIndexRecord {
    hash: u64,
    biome_index: usize,
}

fn validate_biomes(
    bytes: &[u8],
    biomes_offset: usize,
    strings_offset: usize,
    strings_len: usize,
    start: usize,
    end: usize,
) -> Result<()> {
    let mut previous_id = None::<u32>;
    for index in start..end {
        let record = biome_record(bytes, biomes_offset, index);
        if previous_id.is_some_and(|previous| record.id <= previous) {
            return Err(corrupt(
                "biome IDs are not strictly sorted inside a snapshot",
            ));
        }
        previous_id = Some(record.id);
        if record.flags & !KNOWN_FLAGS != 0 {
            return Err(corrupt(format!(
                "biome {} uses unknown registry flag bits 0x{:04x}",
                record.id,
                record.flags & !KNOWN_FLAGS
            )));
        }
        if record.flags & RAIN_VALUE != 0 && record.flags & HAS_RAIN == 0 {
            return Err(corrupt(format!(
                "biome {} stores a rain value without the rain-presence flag",
                record.id
            )));
        }
        if record.reserved != 0 {
            return Err(corrupt(format!(
                "biome {} has non-zero reserved bytes",
                record.id
            )));
        }
        for (flag, field, value) in [
            (HAS_TEMPERATURE, "temperature", record.temperature),
            (HAS_DOWNFALL, "downfall", record.downfall),
            (HAS_FOLIAGE_SNOW, "foliage_snow", record.foliage_snow),
            (HAS_DEPTH, "depth", record.depth),
            (HAS_SCALE, "scale", record.scale),
        ] {
            if record.flags & flag != 0 && !value.is_finite() {
                return Err(corrupt(format!(
                    "biome {} field {field} is not finite",
                    record.id
                )));
            }
        }
        let name = name_from_record(bytes, strings_offset, strings_len, record)?;
        if name.is_empty() {
            return Err(corrupt(format!(
                "biome {} has an empty identifier",
                record.id
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_name_index(
    bytes: &[u8],
    biomes_offset: usize,
    name_index_offset: usize,
    strings_offset: usize,
    strings_len: usize,
    biome_start: usize,
    biome_end: usize,
    start: usize,
    end: usize,
) -> Result<()> {
    let mut previous_name_key = None::<(u64, &str)>;
    for index in start..end {
        let entry = name_index_record(bytes, name_index_offset, index);
        if entry.biome_index < biome_start || entry.biome_index >= biome_end {
            return Err(corrupt("biome name index points outside its snapshot"));
        }
        let biome = biome_record(bytes, biomes_offset, entry.biome_index);
        let name = name_from_record(bytes, strings_offset, strings_len, biome)?;
        if xxh3_64(name.as_bytes()) != entry.hash {
            return Err(corrupt("biome name-index hash does not match its string"));
        }
        if previous_name_key.is_some_and(|previous| (entry.hash, name) <= previous) {
            return Err(corrupt(
                "biome name index is not strictly sorted or contains a duplicate name",
            ));
        }
        previous_name_key = Some((entry.hash, name));
    }
    Ok(())
}

fn snapshot_record(bytes: &[u8], table_offset: usize, index: usize) -> SnapshotRecord {
    let offset = table_offset + index * SNAPSHOT_RECORD_LEN;
    SnapshotRecord {
        version: [
            read_u16(bytes, offset),
            read_u16(bytes, offset + 2),
            read_u16(bytes, offset + 4),
            read_u16(bytes, offset + 6),
        ],
        network_version: read_u32(bytes, offset + 8),
        biome_start: read_u32(bytes, offset + 12) as usize,
        biome_count: read_u32(bytes, offset + 16) as usize,
        name_index_start: read_u32(bytes, offset + 20) as usize,
        name_index_count: read_u32(bytes, offset + 24) as usize,
    }
}

fn biome_record(bytes: &[u8], table_offset: usize, index: usize) -> BiomeRecord {
    let offset = table_offset + index * BIOME_RECORD_LEN;
    BiomeRecord {
        id: read_u32(bytes, offset),
        name_offset: read_u32(bytes, offset + 4) as usize,
        name_len: read_u16(bytes, offset + 8) as usize,
        flags: read_u16(bytes, offset + 10),
        temperature: read_f32(bytes, offset + 12),
        downfall: read_f32(bytes, offset + 16),
        foliage_snow: read_f32(bytes, offset + 20),
        depth: read_f32(bytes, offset + 24),
        scale: read_f32(bytes, offset + 28),
        map_water_color: read_i32(bytes, offset + 32),
        reserved: read_u32(bytes, offset + 36),
    }
}

fn name_index_record(bytes: &[u8], table_offset: usize, index: usize) -> NameIndexRecord {
    let offset = table_offset + index * NAME_INDEX_RECORD_LEN;
    NameIndexRecord {
        hash: read_u64(bytes, offset),
        biome_index: read_u32(bytes, offset + 8) as usize,
    }
}

fn name_from_record<'a>(
    bytes: &'a [u8],
    strings_offset: usize,
    strings_len: usize,
    record: BiomeRecord,
) -> Result<&'a str> {
    let start = record.name_offset;
    let end = start
        .checked_add(record.name_len)
        .ok_or_else(|| corrupt("biome registry name range overflowed"))?;
    if end > strings_len {
        return Err(corrupt(
            "biome registry name points outside the string pool",
        ));
    }
    std::str::from_utf8(&bytes[strings_offset + start..strings_offset + end])
        .map_err(|error| corrupt(format!("biome registry name is not valid UTF-8: {error}")))
}

fn definition_from_record<'a>(
    bytes: &'a [u8],
    strings_offset: usize,
    strings_len: usize,
    record: BiomeRecord,
) -> Option<BiomeDefinition<'a>> {
    let name = name_from_record(bytes, strings_offset, strings_len, record).ok()?;
    Some(BiomeDefinition {
        id: record.id,
        name,
        temperature: flag_value(record.flags, HAS_TEMPERATURE, record.temperature),
        downfall: flag_value(record.flags, HAS_DOWNFALL, record.downfall),
        foliage_snow: flag_value(record.flags, HAS_FOLIAGE_SNOW, record.foliage_snow),
        depth: flag_value(record.flags, HAS_DEPTH, record.depth),
        scale: flag_value(record.flags, HAS_SCALE, record.scale),
        map_water_color: (record.flags & HAS_MAP_WATER_COLOR != 0)
            .then_some(record.map_water_color),
        rain: (record.flags & HAS_RAIN != 0).then_some(record.flags & RAIN_VALUE != 0),
    })
}

fn game_version_key(version: &GameVersion) -> Option<[u16; 4]> {
    if version.components().len() > 4 {
        return None;
    }
    let mut key = [0_u16; 4];
    for (index, component) in version.components().iter().enumerate() {
        key[index] = u16::try_from(*component).ok()?;
    }
    Some(key)
}

fn flag_value(flags: u16, flag: u16, value: f32) -> Option<f32> {
    (flags & flag != 0).then_some(value)
}

fn u32_as_usize(value: u32, field: &str) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| corrupt(format!("biome registry {field} overflowed usize")))
}

fn checked_advance(
    start: usize,
    count: usize,
    record_len: usize,
    table: &str,
) -> Result<usize> {
    let bytes = count
        .checked_mul(record_len)
        .ok_or_else(|| corrupt(format!("biome registry {table} length overflowed")))?;
    start
        .checked_add(bytes)
        .ok_or_else(|| corrupt(format!("biome registry {table} offset overflowed")))
}

fn corrupt(message: impl Into<String>) -> BedrockWorldError {
    BedrockWorldError::CorruptWorld(message.into())
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

fn read_f32(bytes: &[u8], offset: usize) -> f32 {
    f32::from_bits(read_u32(bytes, offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_registry() -> Vec<u8> {
        let name = b"minecraft:plains";
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());

        for component in [1_u16, 26, 44, 3] {
            bytes.extend_from_slice(&component.to_le_bytes());
        }
        bytes.extend_from_slice(&2168_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());

        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&(HAS_TEMPERATURE | HAS_RAIN | RAIN_VALUE).to_le_bytes());
        bytes.extend_from_slice(&0.8_f32.to_le_bytes());
        bytes.extend_from_slice(&0_f32.to_le_bytes());
        bytes.extend_from_slice(&0_f32.to_le_bytes());
        bytes.extend_from_slice(&0_f32.to_le_bytes());
        bytes.extend_from_slice(&0_f32.to_le_bytes());
        bytes.extend_from_slice(&0_i32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());

        bytes.extend_from_slice(&xxh3_64(name).to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(name);
        bytes
    }

    #[test]
    fn resolves_by_exact_version_id_and_name() {
        let bytes = sample_registry();
        let registry = BiomeRegistry::parse(&bytes).unwrap();
        let version = GameVersion::new(vec![1, 26, 44, 3]).unwrap();
        let snapshot = registry.snapshot(&version).unwrap();
        assert_eq!(snapshot.network_version(), Some(2168));
        let by_id = snapshot.biome_by_id(1).unwrap();
        assert_eq!(by_id.name(), "minecraft:plains");
        assert_eq!(by_id.temperature(), Some(0.8));
        assert_eq!(by_id.rain(), Some(true));
        assert_eq!(snapshot.biome_by_name("minecraft:plains"), Some(by_id));
    }

    #[test]
    fn embedded_registry_asset_is_structurally_valid() {
        embedded_biome_registry().unwrap();
    }

    #[test]
    fn unknown_version_and_unknown_biome_are_not_guessed() {
        let bytes = sample_registry();
        let registry = BiomeRegistry::parse(&bytes).unwrap();
        let unknown = GameVersion::new(vec![1, 26, 44, 4]).unwrap();
        assert!(registry.snapshot(&unknown).is_none());

        let known = GameVersion::new(vec![1, 26, 44, 3]).unwrap();
        let snapshot = registry.snapshot(&known).unwrap();
        assert!(snapshot.biome_by_id(999_999).is_none());
    }
}
