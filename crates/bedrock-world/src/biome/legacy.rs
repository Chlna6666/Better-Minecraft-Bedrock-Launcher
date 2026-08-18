//! Historical `Data2DLegacy` biome/height-map representation.
//!
//! `Data2DLegacy` stores the same 256 signed 16-bit height values used by later `Data2D`, followed
//! by 256 four-byte biome samples in `[biome_id, red, green, blue]` order. The RGB components are
//! retained because old worlds may rely on the saved biome colour even after biome identifiers
//! changed meaning across game versions.

use crate::biome::{Biome2d, LegacyBiomeSample};
use crate::error::{BedrockWorldError, Result};

const HEIGHT_BYTES: usize = 256 * 2;
const BIOME_SAMPLE_BYTES: usize = 256 * 4;
/// Exact byte length of a `Data2DLegacy` value.
pub const DATA2D_LEGACY_VALUE_LEN: usize = HEIGHT_BYTES + BIOME_SAMPLE_BYTES;

/// Decoded historical `Data2DLegacy` biome record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Biome2dLegacy {
    /// Height values in `z * 16 + x` column order.
    pub height_map: Vec<i16>,
    /// Historical biome id and saved colour for each horizontal column.
    pub biomes: Vec<LegacyBiomeSample>,
}

impl Biome2dLegacy {
    /// Creates a validated historical 2D biome record.
    pub fn new(height_map: Vec<i16>, biomes: Vec<LegacyBiomeSample>) -> Result<Self> {
        if height_map.len() != 256 || biomes.len() != 256 {
            return Err(BedrockWorldError::Validation(format!(
                "Data2DLegacy requires 256 heights and 256 biome samples, got {}/{}",
                height_map.len(),
                biomes.len()
            )));
        }
        Ok(Self { height_map, biomes })
    }

    /// Parses the exact historical `Data2DLegacy` payload.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != DATA2D_LEGACY_VALUE_LEN {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "Data2DLegacy value must be {DATA2D_LEGACY_VALUE_LEN} bytes, got {}",
                bytes.len()
            )));
        }

        let mut height_map = Vec::with_capacity(256);
        for pair in bytes[..HEIGHT_BYTES].chunks_exact(2) {
            height_map.push(i16::from_le_bytes([pair[0], pair[1]]));
        }

        let mut biomes = Vec::with_capacity(256);
        for sample in bytes[HEIGHT_BYTES..].chunks_exact(4) {
            biomes.push(LegacyBiomeSample {
                biome_id: sample[0],
                red: sample[1],
                green: sample[2],
                blue: sample[3],
            });
        }
        Self::new(height_map, biomes)
    }

    /// Encodes this record without discarding saved historical biome colours.
    pub fn encode(&self) -> Result<Vec<u8>> {
        Self::new(self.height_map.clone(), self.biomes.clone())?;
        let mut bytes = Vec::with_capacity(DATA2D_LEGACY_VALUE_LEN);
        for height in &self.height_map {
            bytes.extend_from_slice(&height.to_le_bytes());
        }
        for biome in &self.biomes {
            bytes.extend_from_slice(&[biome.biome_id, biome.red, biome.green, biome.blue]);
        }
        Ok(bytes)
    }

    /// Returns the `Data2D` semantic view used when promoting this record to `Data3D`.
    ///
    /// Saved RGB values remain available on this object for preservation/diagnostics; modern
    /// `Data3D` stores biome identities rather than these legacy colour bytes.
    pub fn to_data2d(&self) -> Result<Biome2d> {
        Biome2d::new(
            self.height_map.clone(),
            self.biomes.iter().map(|sample| sample.biome_id).collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data2d_legacy_roundtrips_saved_colours() {
        let mut biomes = vec![
            LegacyBiomeSample {
                biome_id: 1,
                red: 2,
                green: 3,
                blue: 4,
            };
            256
        ];
        biomes[17] = LegacyBiomeSample {
            biome_id: 7,
            red: 0x11,
            green: 0x22,
            blue: 0x33,
        };
        let source = Biome2dLegacy::new(vec![64; 256], biomes).unwrap();
        let encoded = source.encode().unwrap();
        let decoded = Biome2dLegacy::parse(&encoded).unwrap();
        assert_eq!(decoded, source);
        assert_eq!(decoded.to_data2d().unwrap().biomes[17], 7);
    }
}
