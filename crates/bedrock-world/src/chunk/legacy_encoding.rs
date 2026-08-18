//! Allocation-bounded encoders for historical numeric Bedrock terrain formats.
//!
//! Builders mutate one contiguous byte buffer in the exact on-disk layout. They deliberately expose
//! numeric ID/data/light operations rather than pretending an arbitrary modern BlockState can be
//! downgraded safely without an authoritative reverse mapping.

use crate::chunk::legacy::{
    LEGACY_SUBCHUNK_BLOCK_COUNT, LEGACY_SUBCHUNK_MIN_VALUE_LEN,
    LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN, LEGACY_TERRAIN_BIOME_OFFSET,
    LEGACY_TERRAIN_BLOCK_DATA_OFFSET, LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET,
    LEGACY_TERRAIN_HEIGHTMAP_OFFSET, LEGACY_TERRAIN_SKY_LIGHT_OFFSET,
    LEGACY_TERRAIN_VALUE_LEN, LegacyBiomeSample, LegacySubChunk, LegacyTerrain,
};
use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;

/// Mutable, one-allocation encoder for the fixed 83,200-byte `LegacyTerrain` record.
#[derive(Debug, Clone)]
pub struct LegacyTerrainBuilder {
    bytes: Vec<u8>,
}

impl Default for LegacyTerrainBuilder {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl LegacyTerrainBuilder {
    /// Creates a zero-filled historical terrain record.
    ///
    /// Sky light is also zero until explicitly set or filled by the caller.
    #[must_use]
    pub fn zeroed() -> Self {
        Self {
            bytes: vec![0; LEGACY_TERRAIN_VALUE_LEN],
        }
    }

    /// Creates an editable byte-for-byte copy of an existing historical terrain record.
    #[must_use]
    pub fn from_terrain(source: &LegacyTerrain) -> Self {
        Self {
            bytes: source.raw().to_vec(),
        }
    }

    /// Returns the exact mutable output buffer for advanced codecs that already know the layout.
    #[must_use]
    pub fn as_raw_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    /// Sets one legacy numeric block ID and its four-bit metadata value atomically.
    pub fn set_block(
        &mut self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
        block_id: u8,
        block_data: u8,
    ) -> Result<()> {
        let index = terrain_block_index(local_x, local_y, local_z)?;
        set_nibble(
            &mut self.bytes[LEGACY_TERRAIN_BLOCK_DATA_OFFSET..LEGACY_TERRAIN_SKY_LIGHT_OFFSET],
            index,
            block_data,
            "LegacyTerrain block data",
        )?;
        self.bytes[index] = block_id;
        Ok(())
    }

    /// Sets one four-bit sky-light value.
    pub fn set_sky_light(
        &mut self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
        value: u8,
    ) -> Result<()> {
        let index = terrain_block_index(local_x, local_y, local_z)?;
        set_nibble(
            &mut self.bytes[LEGACY_TERRAIN_SKY_LIGHT_OFFSET..LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET],
            index,
            value,
            "LegacyTerrain sky light",
        )
    }

    /// Fills all sky-light nibbles with one value.
    pub fn fill_sky_light(&mut self, value: u8) -> Result<()> {
        fill_nibbles(
            &mut self.bytes[LEGACY_TERRAIN_SKY_LIGHT_OFFSET..LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET],
            value,
            "LegacyTerrain sky light",
        )
    }

    /// Sets one four-bit block-light value.
    pub fn set_block_light(
        &mut self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
        value: u8,
    ) -> Result<()> {
        let index = terrain_block_index(local_x, local_y, local_z)?;
        set_nibble(
            &mut self.bytes[LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET..LEGACY_TERRAIN_HEIGHTMAP_OFFSET],
            index,
            value,
            "LegacyTerrain block light",
        )
    }

    /// Fills all block-light nibbles with one value.
    pub fn fill_block_light(&mut self, value: u8) -> Result<()> {
        fill_nibbles(
            &mut self.bytes[LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET..LEGACY_TERRAIN_HEIGHTMAP_OFFSET],
            value,
            "LegacyTerrain block light",
        )
    }

    /// Sets one raw legacy heightmap byte.
    pub fn set_height(&mut self, local_x: u8, local_z: u8, height: u8) -> Result<()> {
        let column = terrain_column_index(local_x, local_z)?;
        self.bytes[LEGACY_TERRAIN_HEIGHTMAP_OFFSET + column] = height;
        Ok(())
    }

    /// Sets one legacy biome ID plus its persisted RGB components.
    pub fn set_biome_sample(
        &mut self,
        local_x: u8,
        local_z: u8,
        sample: LegacyBiomeSample,
    ) -> Result<()> {
        let column = terrain_column_index(local_x, local_z)?;
        let offset = LEGACY_TERRAIN_BIOME_OFFSET + column * 4;
        self.bytes[offset..offset + 4]
            .copy_from_slice(&[sample.biome_id, sample.red, sample.green, sample.blue]);
        Ok(())
    }

    /// Finishes the record and revalidates the exact physical size before returning it.
    pub fn build(self) -> Result<LegacyTerrain> {
        LegacyTerrain::parse(Bytes::from(self.bytes))
    }
}

impl LegacyTerrain {
    /// Creates an editable one-allocation copy while preserving every byte not explicitly changed.
    #[must_use]
    pub fn to_builder(&self) -> LegacyTerrainBuilder {
        LegacyTerrainBuilder::from_terrain(self)
    }

    /// Returns the owned raw value. `Bytes::clone` is reference-counted and does not copy the payload.
    #[must_use]
    pub fn into_raw(self) -> Bytes {
        self.raw().clone()
    }
}

/// Mutable encoder for legacy fixed-array SubChunk versions `0` and `2..=7`.
#[derive(Debug, Clone)]
pub struct LegacySubChunkBuilder {
    bytes: Vec<u8>,
}

impl LegacySubChunkBuilder {
    /// Creates a zero-filled legacy subchunk.
    ///
    /// `with_light_arrays` selects the historical short form (`ID + data`) or long form
    /// (`ID + data + sky light + block light`).
    pub fn zeroed(version: u8, with_light_arrays: bool) -> Result<Self> {
        validate_legacy_subchunk_version(version)?;
        let len = if with_light_arrays {
            LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN
        } else {
            LEGACY_SUBCHUNK_MIN_VALUE_LEN
        };
        let mut bytes = vec![0; len];
        bytes[0] = version;
        Ok(Self { bytes })
    }

    /// Creates an editable byte-for-byte copy of an existing legacy subchunk.
    #[must_use]
    pub fn from_subchunk(source: &LegacySubChunk) -> Self {
        Self {
            bytes: source.raw().to_vec(),
        }
    }

    /// Returns the encoded legacy payload version.
    #[must_use]
    pub fn version(&self) -> u8 {
        self.bytes[0]
    }

    /// Returns whether this payload contains sky-light and block-light arrays.
    #[must_use]
    pub fn has_light_arrays(&self) -> bool {
        self.bytes.len() == LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN
    }

    /// Returns the exact mutable output buffer for advanced codecs. `build()` revalidates any edits.
    #[must_use]
    pub fn as_raw_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    /// Sets one legacy numeric block ID and its four-bit metadata value atomically.
    pub fn set_block(
        &mut self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
        block_id: u8,
        block_data: u8,
    ) -> Result<()> {
        let index = subchunk_block_index(local_x, local_y, local_z)?;
        let start = 1 + LEGACY_SUBCHUNK_BLOCK_COUNT;
        set_nibble(
            &mut self.bytes[start..start + LEGACY_SUBCHUNK_BLOCK_COUNT / 2],
            index,
            block_data,
            "legacy subchunk block data",
        )?;
        self.bytes[1 + index] = block_id;
        Ok(())
    }

    /// Sets one sky-light nibble. Returns an error when this payload intentionally omits light arrays.
    pub fn set_sky_light(
        &mut self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
        value: u8,
    ) -> Result<()> {
        let index = subchunk_block_index(local_x, local_y, local_z)?;
        let range = subchunk_sky_light_range(&self.bytes)?;
        set_nibble(
            &mut self.bytes[range],
            index,
            value,
            "legacy subchunk sky light",
        )
    }

    /// Fills the complete sky-light array with one nibble value.
    pub fn fill_sky_light(&mut self, value: u8) -> Result<()> {
        let range = subchunk_sky_light_range(&self.bytes)?;
        fill_nibbles(
            &mut self.bytes[range],
            value,
            "legacy subchunk sky light",
        )
    }

    /// Sets one block-light nibble. Returns an error when this payload intentionally omits light arrays.
    pub fn set_block_light(
        &mut self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
        value: u8,
    ) -> Result<()> {
        let index = subchunk_block_index(local_x, local_y, local_z)?;
        let range = subchunk_block_light_range(&self.bytes)?;
        set_nibble(
            &mut self.bytes[range],
            index,
            value,
            "legacy subchunk block light",
        )
    }

    /// Fills the complete block-light array with one nibble value.
    pub fn fill_block_light(&mut self, value: u8) -> Result<()> {
        let range = subchunk_block_light_range(&self.bytes)?;
        fill_nibbles(
            &mut self.bytes[range],
            value,
            "legacy subchunk block light",
        )
    }

    /// Finishes and revalidates the historical payload version and exact physical size.
    pub fn build(self) -> Result<LegacySubChunk> {
        LegacySubChunk::parse(Bytes::from(self.bytes))
    }
}

impl LegacySubChunk {
    /// Creates an editable one-allocation copy while preserving omitted/present light-array shape.
    #[must_use]
    pub fn to_builder(&self) -> LegacySubChunkBuilder {
        LegacySubChunkBuilder::from_subchunk(self)
    }

    /// Returns the owned raw payload. `Bytes::clone` is reference-counted and does not copy payload.
    #[must_use]
    pub fn into_raw(self) -> Bytes {
        self.raw().clone()
    }
}

fn terrain_block_index(local_x: u8, local_y: u8, local_z: u8) -> Result<usize> {
    LegacyTerrain::block_index(local_x, local_y, local_z).ok_or_else(|| {
        BedrockWorldError::Validation(format!(
            "LegacyTerrain coordinates out of range: ({local_x}, {local_y}, {local_z})"
        ))
    })
}

fn terrain_column_index(local_x: u8, local_z: u8) -> Result<usize> {
    LegacyTerrain::column_index(local_x, local_z).ok_or_else(|| {
        BedrockWorldError::Validation(format!(
            "LegacyTerrain column coordinates out of range: ({local_x}, {local_z})"
        ))
    })
}

fn subchunk_block_index(local_x: u8, local_y: u8, local_z: u8) -> Result<usize> {
    LegacySubChunk::block_index(local_x, local_y, local_z).ok_or_else(|| {
        BedrockWorldError::Validation(format!(
            "legacy subchunk coordinates out of range: ({local_x}, {local_y}, {local_z})"
        ))
    })
}

fn validate_legacy_subchunk_version(version: u8) -> Result<()> {
    if matches!(version, 0 | 2..=7) {
        Ok(())
    } else {
        Err(BedrockWorldError::Validation(format!(
            "legacy subchunk encoder only supports versions 0 and 2..=7, got {version}"
        )))
    }
}

fn subchunk_sky_light_range(bytes: &[u8]) -> Result<std::ops::Range<usize>> {
    if bytes.len() != LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN {
        return Err(BedrockWorldError::Validation(
            "legacy subchunk payload omits light arrays".to_string(),
        ));
    }
    let start = 1 + LEGACY_SUBCHUNK_BLOCK_COUNT + LEGACY_SUBCHUNK_BLOCK_COUNT / 2;
    Ok(start..start + LEGACY_SUBCHUNK_BLOCK_COUNT / 2)
}

fn subchunk_block_light_range(bytes: &[u8]) -> Result<std::ops::Range<usize>> {
    if bytes.len() != LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN {
        return Err(BedrockWorldError::Validation(
            "legacy subchunk payload omits light arrays".to_string(),
        ));
    }
    let start = 1 + LEGACY_SUBCHUNK_BLOCK_COUNT + LEGACY_SUBCHUNK_BLOCK_COUNT;
    Ok(start..start + LEGACY_SUBCHUNK_BLOCK_COUNT / 2)
}

fn set_nibble(bytes: &mut [u8], index: usize, value: u8, context: &str) -> Result<()> {
    if value > 0x0f {
        return Err(BedrockWorldError::Validation(format!(
            "{context} must fit four bits, got {value}"
        )));
    }
    let byte = bytes.get_mut(index / 2).ok_or_else(|| {
        BedrockWorldError::Validation(format!("{context} index {index} is out of range"))
    })?;
    if index.is_multiple_of(2) {
        *byte = (*byte & 0xf0) | value;
    } else {
        *byte = (*byte & 0x0f) | (value << 4);
    }
    Ok(())
}

fn fill_nibbles(bytes: &mut [u8], value: u8, context: &str) -> Result<()> {
    if value > 0x0f {
        return Err(BedrockWorldError::Validation(format!(
            "{context} must fit four bits, got {value}"
        )));
    }
    bytes.fill(value | (value << 4));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_terrain_builder_roundtrips_numeric_fields() {
        let mut builder = LegacyTerrainBuilder::zeroed();
        builder.set_block(15, 127, 15, 42, 13).unwrap();
        builder.set_sky_light(15, 127, 15, 15).unwrap();
        builder.set_block_light(15, 127, 15, 7).unwrap();
        builder.set_height(15, 15, 99).unwrap();
        builder
            .set_biome_sample(
                15,
                15,
                LegacyBiomeSample {
                    biome_id: 5,
                    red: 11,
                    green: 22,
                    blue: 33,
                },
            )
            .unwrap();
        let terrain = builder.build().unwrap();
        assert_eq!(terrain.raw().len(), LEGACY_TERRAIN_VALUE_LEN);
        assert_eq!(terrain.block_id_at(15, 127, 15), Some(42));
        assert_eq!(terrain.block_data_at(15, 127, 15), Some(13));
        assert_eq!(terrain.sky_light_at(15, 127, 15), Some(15));
        assert_eq!(terrain.block_light_at(15, 127, 15), Some(7));
        assert_eq!(terrain.height_at(15, 15), Some(99));
        assert_eq!(terrain.biome_sample_at(15, 15).unwrap().biome_id, 5);
    }

    #[test]
    fn legacy_subchunk_builder_roundtrips_all_supported_fields() {
        for version in [0, 2, 3, 4, 5, 6, 7] {
            let mut builder = LegacySubChunkBuilder::zeroed(version, true).unwrap();
            builder.set_block(15, 15, 15, 200, 14).unwrap();
            builder.set_sky_light(15, 15, 15, 12).unwrap();
            builder.set_block_light(15, 15, 15, 6).unwrap();
            let subchunk = builder.build().unwrap();
            assert_eq!(subchunk.version(), version);
            assert_eq!(subchunk.block_id_at(15, 15, 15), Some(200));
            assert_eq!(subchunk.block_data_at(15, 15, 15), Some(14));
            assert_eq!(subchunk.sky_light_at(15, 15, 15), Some(12));
            assert_eq!(subchunk.block_light_at(15, 15, 15), Some(6));
        }
    }

    #[test]
    fn short_legacy_subchunk_preserves_omitted_light_arrays() {
        let mut builder = LegacySubChunkBuilder::zeroed(7, false).unwrap();
        builder.set_block(1, 2, 3, 4, 5).unwrap();
        assert!(builder.set_sky_light(1, 2, 3, 15).is_err());
        let subchunk = builder.build().unwrap();
        assert!(subchunk.sky_light().is_none());
        assert!(subchunk.block_light().is_none());
        assert_eq!(subchunk.raw().len(), LEGACY_SUBCHUNK_MIN_VALUE_LEN);
    }

    #[test]
    fn raw_version_edits_are_revalidated_on_build() {
        let mut builder = LegacySubChunkBuilder::zeroed(7, false).unwrap();
        builder.as_raw_mut()[0] = 8;
        assert_eq!(builder.version(), 8);
        assert!(builder.build().is_err());
    }

    #[test]
    fn legacy_subchunk_builder_rejects_paletted_versions() {
        for version in [1, 8, 9, 10] {
            assert!(LegacySubChunkBuilder::zeroed(version, false).is_err());
        }
    }

    #[test]
    fn builders_reject_values_outside_nibble_range_without_partial_write() {
        let mut terrain = LegacyTerrainBuilder::zeroed();
        assert!(terrain.set_block(0, 0, 0, 9, 16).is_err());
        assert_eq!(terrain.as_raw_mut()[0], 0);
        assert!(terrain.fill_sky_light(16).is_err());

        let mut subchunk = LegacySubChunkBuilder::zeroed(7, true).unwrap();
        assert!(subchunk.set_block(0, 0, 0, 9, 16).is_err());
        assert_eq!(subchunk.as_raw_mut()[1], 0);
        assert!(subchunk.set_block_light(0, 0, 0, 16).is_err());
    }
}
