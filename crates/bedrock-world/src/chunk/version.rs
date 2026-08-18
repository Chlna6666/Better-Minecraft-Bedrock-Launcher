//! Minecraft Bedrock SubChunk payload versions, conservative reads and version-preserving writes.

use crate::chunk::encoding::encode_paletted_subchunk_from_palettes;
use crate::chunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat};
use crate::error::{BedrockWorldError, Result};
use crate::version::ConversionCompatibility;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Version byte stored at the beginning of a Minecraft Bedrock SubChunk payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubChunkVersion {
    /// Fixed-array SubChunk version 0.
    V0,
    /// Paletted SubChunk version 1.
    V1,
    /// Historical fixed-array SubChunk version 2.
    V2,
    /// Historical fixed-array SubChunk version 3.
    V3,
    /// Historical fixed-array SubChunk version 4.
    V4,
    /// Historical fixed-array SubChunk version 5.
    V5,
    /// Historical fixed-array SubChunk version 6.
    V6,
    /// Historical fixed-array SubChunk version 7.
    V7,
    /// Paletted SubChunk version 8.
    V8,
    /// Paletted SubChunk version 9 with explicit Y in the payload.
    V9,
    /// A version byte not understood by this library.
    Unknown(u8),
}

impl SubChunkVersion {
    /// Decodes an on-disk SubChunk version byte.
    #[must_use]
    pub const fn from_byte(version: u8) -> Self {
        match version {
            0 => Self::V0,
            1 => Self::V1,
            2 => Self::V2,
            3 => Self::V3,
            4 => Self::V4,
            5 => Self::V5,
            6 => Self::V6,
            7 => Self::V7,
            8 => Self::V8,
            9 => Self::V9,
            other => Self::Unknown(other),
        }
    }

    /// Detects the version directly from a raw SubChunk payload.
    #[must_use]
    pub fn detect(bytes: &[u8]) -> Option<Self> {
        bytes.first().copied().map(Self::from_byte)
    }

    /// Returns the exact on-disk version byte.
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            Self::V0 => 0,
            Self::V1 => 1,
            Self::V2 => 2,
            Self::V3 => 3,
            Self::V4 => 4,
            Self::V5 => 5,
            Self::V6 => 6,
            Self::V7 => 7,
            Self::V8 => 8,
            Self::V9 => 9,
            Self::Unknown(version) => version,
        }
    }

    /// Returns whether this version uses the historical fixed-array representation.
    #[must_use]
    pub const fn is_legacy_fixed(self) -> bool {
        matches!(self, Self::V0 | Self::V2 | Self::V3 | Self::V4 | Self::V5 | Self::V6 | Self::V7)
    }

    /// Returns whether this version uses a palette representation understood by this library.
    #[must_use]
    pub const fn is_paletted(self) -> bool {
        matches!(self, Self::V1 | Self::V8 | Self::V9)
    }
}

impl SubChunk {
    /// Returns the actual persisted SubChunk version when the payload has one.
    #[must_use]
    pub const fn version(&self) -> Option<SubChunkVersion> {
        self.format.version()
    }

    /// Reports whether a requested version conversion is known to be lossless from this payload.
    #[must_use]
    pub fn conversion_compatibility(&self, target: SubChunkVersion) -> ConversionCompatibility {
        match self.version() {
            Some(source) if source == target => ConversionCompatibility::Lossless,
            Some(source) if source.is_paletted() && target.is_paletted() => {
                match &self.format {
                    SubChunkFormat::Paletted { storages, .. }
                        if !matches!(target, SubChunkVersion::V1) || storages.len() == 1 =>
                    {
                        ConversionCompatibility::Lossless
                    }
                    _ => ConversionCompatibility::Unsupported,
                }
            }
            _ => ConversionCompatibility::Unsupported,
        }
    }
}

impl SubChunkFormat {
    /// Returns the actual SubChunk payload version represented by this decoded value.
    #[must_use]
    pub const fn version(&self) -> Option<SubChunkVersion> {
        match self {
            Self::LegacySubChunk(subchunk) => Some(SubChunkVersion::from_byte(subchunk.version())),
            Self::FixedArrayV1 => Some(SubChunkVersion::V1),
            Self::Paletted { version, .. } => Some(SubChunkVersion::from_byte(*version)),
            Self::Raw { version, .. } => match version {
                Some(version) => Some(SubChunkVersion::from_byte(*version)),
                None => None,
            },
            Self::LegacyTerrain => None,
        }
    }
}

/// Reads a SubChunk by automatically inspecting its persisted version byte.
///
/// Known V0-V9 payloads delegate to the structured parser. Unknown versions are always retained raw
/// even when their bytes happen to resemble a known palette layout.
pub fn read_subchunk(
    y: i8,
    bytes: Bytes,
    mode: SubChunkDecodeMode,
) -> Result<SubChunk> {
    match SubChunkVersion::detect(&bytes) {
        Some(SubChunkVersion::Unknown(version)) => Ok(SubChunk {
            y,
            format: SubChunkFormat::Raw {
                version: Some(version),
                bytes,
            },
        }),
        _ => crate::chunk::subchunk::parse_subchunk_with_mode(y, bytes, mode),
    }
}

/// Serializes a SubChunk using the same persisted version that was read.
///
/// Legacy and unsupported raw payloads are retained byte-for-byte. Decoded paletted V1/V8/V9 data
/// is re-encoded in its original version. This function never upgrades or downgrades implicitly.
pub fn write_subchunk_preserving_version(subchunk: &SubChunk) -> Result<Bytes> {
    match &subchunk.format {
        SubChunkFormat::LegacySubChunk(legacy) => Ok(legacy.raw().clone()),
        SubChunkFormat::Paletted { version, storages } => {
            let storages = storages.iter().collect::<Vec<_>>();
            encode_paletted_subchunk_from_palettes(*version, subchunk.y, &storages)
        }
        SubChunkFormat::Raw { bytes, .. } => Ok(bytes.clone()),
        SubChunkFormat::FixedArrayV1 => Err(BedrockWorldError::UnsupportedChunkFormat(
            "FixedArrayV1 has no retained payload to write losslessly".to_string(),
        )),
        SubChunkFormat::LegacyTerrain => Err(BedrockWorldError::UnsupportedChunkFormat(
            "LegacyTerrain is a chunk record, not a SubChunk payload".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::LegacySubChunkBuilder;

    #[test]
    fn detects_all_known_subchunk_version_bytes() {
        for version in 0_u8..=9 {
            assert_eq!(SubChunkVersion::from_byte(version).byte(), version);
        }
        assert_eq!(SubChunkVersion::from_byte(12), SubChunkVersion::Unknown(12));
    }

    #[test]
    fn legacy_subchunk_preserving_write_is_byte_exact() {
        let raw = LegacySubChunkBuilder::new(7).unwrap().build().unwrap();
        let parsed = read_subchunk(0, raw.clone(), SubChunkDecodeMode::FullIndices).unwrap();
        assert_eq!(parsed.version(), Some(SubChunkVersion::V7));
        assert_eq!(write_subchunk_preserving_version(&parsed).unwrap(), raw);
    }

    #[test]
    fn unknown_future_subchunk_is_preserved_raw() {
        let raw = Bytes::from_static(&[10, 1, 0]);
        let parsed = read_subchunk(0, raw.clone(), SubChunkDecodeMode::FullIndices).unwrap();
        assert_eq!(parsed.version(), Some(SubChunkVersion::Unknown(10)));
        assert_eq!(write_subchunk_preserving_version(&parsed).unwrap(), raw);
    }
}
