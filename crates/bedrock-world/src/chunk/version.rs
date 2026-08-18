//! Minecraft Bedrock SubChunk V0 through V9 version byte handling.

use crate::chunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat};
use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Version byte stored at the beginning of a Minecraft Bedrock SubChunk payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubChunkVersion {
    /// SubChunk V0.
    V0,
    /// SubChunk V1.
    V1,
    /// SubChunk V2.
    V2,
    /// SubChunk V3.
    V3,
    /// SubChunk V4.
    V4,
    /// SubChunk V5.
    V5,
    /// SubChunk V6.
    V6,
    /// SubChunk V7.
    V7,
    /// SubChunk V8.
    V8,
    /// SubChunk V9.
    V9,
    /// A SubChunk version not implemented by this library.
    Unknown(u8),
}

impl SubChunkVersion {
    /// Reads the exact SubChunk version byte.
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

    /// Detects a SubChunk version from its payload without parsing block data.
    #[must_use]
    pub fn detect(bytes: &[u8]) -> Option<Self> {
        bytes.first().copied().map(Self::from_byte)
    }

    /// Returns the persisted version byte.
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
}

impl SubChunkFormat {
    /// Returns the actual version represented by this SubChunk payload.
    #[must_use]
    pub const fn version(&self) -> Option<SubChunkVersion> {
        match self {
            Self::LegacySubChunk(value) => Some(SubChunkVersion::from_byte(value.version())),
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

impl SubChunk {
    /// Reads a SubChunk by its persisted leading version byte.
    pub fn read(y: i8, bytes: Bytes, mode: SubChunkDecodeMode) -> Result<Self> {
        match SubChunkVersion::detect(&bytes) {
            Some(SubChunkVersion::V0) => crate::chunk::subchunk_v0::read(y, bytes, mode),
            Some(SubChunkVersion::V1) => crate::chunk::subchunk_v1::read(y, bytes, mode),
            Some(version @ (SubChunkVersion::V2
            | SubChunkVersion::V3
            | SubChunkVersion::V4
            | SubChunkVersion::V5
            | SubChunkVersion::V6
            | SubChunkVersion::V7)) => {
                crate::chunk::subchunk_v2_v7::read(version.byte(), y, bytes, mode)
            }
            Some(SubChunkVersion::V8) => crate::chunk::subchunk_v8::read(y, bytes, mode),
            Some(SubChunkVersion::V9) => crate::chunk::subchunk_v9::read(y, bytes, mode),
            Some(SubChunkVersion::Unknown(version)) => Ok(Self {
                y,
                format: SubChunkFormat::Raw {
                    version: Some(version),
                    bytes,
                },
            }),
            None => Ok(Self {
                y,
                format: SubChunkFormat::Raw {
                    version: None,
                    bytes,
                },
            }),
        }
    }

    /// Returns the actual persisted SubChunk version.
    #[must_use]
    pub const fn version(&self) -> Option<SubChunkVersion> {
        self.format.version()
    }

    /// Writes this SubChunk using the version it currently represents.
    pub fn write(&self) -> Result<Bytes> {
        match self.version() {
            Some(SubChunkVersion::V0) => self.write_v0(),
            Some(SubChunkVersion::V1) => self.write_v1(),
            Some(SubChunkVersion::V2) => self.write_v2(),
            Some(SubChunkVersion::V3) => self.write_v3(),
            Some(SubChunkVersion::V4) => self.write_v4(),
            Some(SubChunkVersion::V5) => self.write_v5(),
            Some(SubChunkVersion::V6) => self.write_v6(),
            Some(SubChunkVersion::V7) => self.write_v7(),
            Some(SubChunkVersion::V8) => self.write_v8(),
            Some(SubChunkVersion::V9) => self.write_v9(),
            Some(SubChunkVersion::Unknown(_)) => match &self.format {
                SubChunkFormat::Raw { bytes, .. } => Ok(bytes.clone()),
                _ => Err(BedrockWorldError::UnsupportedChunkFormat(
                    "unknown SubChunk version is not retained as raw bytes".to_string(),
                )),
            },
            None => Err(BedrockWorldError::UnsupportedChunkFormat(
                "value is not a SubChunk payload".to_string(),
            )),
        }
    }

    /// Writes this block data as SubChunk V0 when it is representable exactly.
    pub fn write_v0(&self) -> Result<Bytes> {
        crate::chunk::subchunk_v0::write(self)
    }

    /// Writes this block data as SubChunk V1 when it is representable exactly.
    pub fn write_v1(&self) -> Result<Bytes> {
        crate::chunk::subchunk_v1::write(self)
    }

    /// Writes this block data as SubChunk V2 when it is representable exactly.
    pub fn write_v2(&self) -> Result<Bytes> {
        crate::chunk::subchunk_v2_v7::write(2, self)
    }

    /// Writes this block data as SubChunk V3 when it is representable exactly.
    pub fn write_v3(&self) -> Result<Bytes> {
        crate::chunk::subchunk_v2_v7::write(3, self)
    }

    /// Writes this block data as SubChunk V4 when it is representable exactly.
    pub fn write_v4(&self) -> Result<Bytes> {
        crate::chunk::subchunk_v2_v7::write(4, self)
    }

    /// Writes this block data as SubChunk V5 when it is representable exactly.
    pub fn write_v5(&self) -> Result<Bytes> {
        crate::chunk::subchunk_v2_v7::write(5, self)
    }

    /// Writes this block data as SubChunk V6 when it is representable exactly.
    pub fn write_v6(&self) -> Result<Bytes> {
        crate::chunk::subchunk_v2_v7::write(6, self)
    }

    /// Writes this block data as SubChunk V7 when it is representable exactly.
    pub fn write_v7(&self) -> Result<Bytes> {
        crate::chunk::subchunk_v2_v7::write(7, self)
    }

    /// Writes this block data as SubChunk V8 when it is representable exactly.
    pub fn write_v8(&self) -> Result<Bytes> {
        crate::chunk::subchunk_v8::write(self)
    }

    /// Writes this block data as SubChunk V9 when it is representable exactly.
    pub fn write_v9(&self) -> Result<Bytes> {
        crate::chunk::subchunk_v9::write(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::LegacySubChunkBuilder;

    #[test]
    fn detects_v0_through_v9() {
        for version in 0_u8..=9 {
            assert_eq!(SubChunkVersion::from_byte(version).byte(), version);
        }
        assert_eq!(SubChunkVersion::from_byte(10), SubChunkVersion::Unknown(10));
    }

    #[test]
    fn v7_reads_and_writes_as_v7() {
        let raw = LegacySubChunkBuilder::new(7).unwrap().build().unwrap();
        let subchunk = SubChunk::read(0, raw.clone(), SubChunkDecodeMode::FullIndices).unwrap();
        assert_eq!(subchunk.version(), Some(SubChunkVersion::V7));
        assert_eq!(subchunk.write_v7().unwrap(), raw);
    }

    #[test]
    fn unknown_version_roundtrips_raw() {
        let raw = Bytes::from_static(&[10, 1, 0]);
        let subchunk = SubChunk::read(0, raw.clone(), SubChunkDecodeMode::FullIndices).unwrap();
        assert_eq!(subchunk.version(), Some(SubChunkVersion::Unknown(10)));
        assert_eq!(subchunk.write().unwrap(), raw);
    }
}
