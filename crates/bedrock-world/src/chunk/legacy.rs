//! Historical numeric terrain and legacy subchunk representations.

use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;

/// Number of block ID entries in an old 16x128x16 `LegacyTerrain` value.
pub const LEGACY_TERRAIN_BLOCK_COUNT: usize = 16 * 128 * 16;
/// Exact byte length of an old LevelDB `LegacyTerrain` value including the 1024-byte biome/RGB tail.
pub const LEGACY_TERRAIN_VALUE_LEN: usize = 83_200;
/// Number of block entries in a 16x16x16 legacy subchunk.
pub const LEGACY_SUBCHUNK_BLOCK_COUNT: usize = 16 * 16 * 16;
/// Minimum byte length of a legacy subchunk without light arrays.
pub const LEGACY_SUBCHUNK_MIN_VALUE_LEN: usize =
    1 + LEGACY_SUBCHUNK_BLOCK_COUNT + LEGACY_SUBCHUNK_BLOCK_COUNT / 2;
/// Byte length of a legacy subchunk with sky and block light arrays.
pub const LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN: usize =
    LEGACY_SUBCHUNK_MIN_VALUE_LEN + LEGACY_SUBCHUNK_BLOCK_COUNT;

pub(crate) const LEGACY_TERRAIN_BLOCK_DATA_OFFSET: usize = LEGACY_TERRAIN_BLOCK_COUNT;
pub(crate) const LEGACY_TERRAIN_SKY_LIGHT_OFFSET: usize =
    LEGACY_TERRAIN_BLOCK_DATA_OFFSET + LEGACY_TERRAIN_BLOCK_COUNT / 2;
pub(crate) const LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET: usize =
    LEGACY_TERRAIN_SKY_LIGHT_OFFSET + LEGACY_TERRAIN_BLOCK_COUNT / 2;
pub(crate) const LEGACY_TERRAIN_HEIGHTMAP_OFFSET: usize =
    LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET + LEGACY_TERRAIN_BLOCK_COUNT / 2;
pub(crate) const LEGACY_TERRAIN_BIOME_OFFSET: usize = LEGACY_TERRAIN_HEIGHTMAP_OFFSET + 16 * 16;
/// Exact byte length of the terrain core used by pre-LevelDB Pocket Edition `chunks.dat`.
///
/// This form contains block IDs, metadata, sky light, block light and the 16x16 height map, but no
/// persisted biome id/RGB samples. The library preserves that absence instead of synthesising bytes.
pub const POCKET_TERRAIN_VALUE_LEN: usize = LEGACY_TERRAIN_BIOME_OFFSET;

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
    /// Returns the saved RGB value as `0xRRGGBB`.
    pub const fn rgb_u32(self) -> u32 {
        ((self.red as u32) << 16) | ((self.green as u32) << 8) | self.blue as u32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Decoded view over historical 16x128x16 numeric terrain.
///
/// Both the pre-LevelDB Pocket Edition terrain core and the later LevelDB `LegacyTerrain` value are
/// accepted. The shorter Pocket form intentionally has no biome samples; callers can inspect that via
/// [`Self::has_biome_samples`].
pub struct LegacyTerrain {
    bytes: Bytes,
}

impl LegacyTerrain {
    /// Parses historical terrain bytes without inventing fields absent from the source representation.
    pub fn parse(bytes: Bytes) -> Result<Self> {
        if !matches!(
            bytes.len(),
            POCKET_TERRAIN_VALUE_LEN | LEGACY_TERRAIN_VALUE_LEN
        ) {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "historical terrain value must be {POCKET_TERRAIN_VALUE_LEN} bytes (Pocket chunks.dat core) or {LEGACY_TERRAIN_VALUE_LEN} bytes (LevelDB LegacyTerrain), got {}",
                bytes.len()
            )));
        }
        Ok(Self { bytes })
    }

    #[must_use]
    /// Returns the complete raw terrain bytes exactly as supplied.
    pub fn raw(&self) -> &Bytes {
        &self.bytes
    }

    #[must_use]
    /// Returns whether this source actually persists all 256 `[biome_id, red, green, blue]` samples.
    pub fn has_biome_samples(&self) -> bool {
        self.bytes.len() == LEGACY_TERRAIN_VALUE_LEN
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
    /// Returns persisted legacy biome samples as `[biome_id, red, green, blue]` columns.
    ///
    /// Pre-LevelDB Pocket `chunks.dat` does not contain this tail, so this returns an empty slice for
    /// that source form rather than a synthetic default biome.
    pub fn biomes(&self) -> &[u8] {
        self.bytes
            .get(LEGACY_TERRAIN_BIOME_OFFSET..LEGACY_TERRAIN_VALUE_LEN)
            .unwrap_or(&[])
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
    /// Returns the legacy biome sample for a local column when the source persisted biome data.
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
    /// Returns the legacy RGB biome color for a local column when present.
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
    /// Returns whether this payload persists both legacy sky-light and block-light nibble arrays.
    pub fn has_light_arrays(&self) -> bool {
        self.bytes.len() == LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN
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
        if !self.has_light_arrays() {
            return None;
        }
        let start = 1 + LEGACY_SUBCHUNK_BLOCK_COUNT + LEGACY_SUBCHUNK_BLOCK_COUNT / 2;
        let end = start + LEGACY_SUBCHUNK_BLOCK_COUNT / 2;
        Some(&self.bytes[start..end])
    }

    #[must_use]
    /// Returns packed 4-bit block-light values when present.
    pub fn block_light(&self) -> Option<&[u8]> {
        if !self.has_light_arrays() {
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

fn nibble_at(bytes: &[u8], index: usize) -> Option<u8> {
    let byte = *bytes.get(index / 2)?;
    Some(if index.is_multiple_of(2) {
        byte & 0x0f
    } else {
        byte >> 4
    })
}
