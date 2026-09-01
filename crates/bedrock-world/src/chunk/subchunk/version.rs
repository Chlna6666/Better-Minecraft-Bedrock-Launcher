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
            Some(SubChunkVersion::V0) => super::v0::read(y, bytes, mode),
            Some(SubChunkVersion::V1) => super::v1::read(y, bytes, mode),
            Some(
                version @ (SubChunkVersion::V2
                | SubChunkVersion::V3
                | SubChunkVersion::V4
                | SubChunkVersion::V5
                | SubChunkVersion::V6
                | SubChunkVersion::V7),
            ) => super::v2_v7::read(version.byte(), y, bytes, mode),
            Some(SubChunkVersion::V8) => super::v8::read(y, bytes, mode),
            Some(SubChunkVersion::V9) => super::v9::read(y, bytes, mode),
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
    ///
    /// Normal round-trips use this method so a V5 remains V5 and a V8 remains V8. Cross-version
    /// writes happen only through [`Self::encode`].
    pub fn write(&self) -> Result<Bytes> {
        let version = self.version().ok_or_else(|| {
            BedrockWorldError::UnsupportedChunkFormat(
                "value is not a versioned SubChunk payload".to_string(),
            )
        })?;
        self.encode(version)
    }

    /// Writes this block data using an explicitly selected Minecraft Bedrock SubChunk version.
    ///
    /// This method never infers a target from a game version and never silently changes data to make
    /// an older representation fit. The concrete V0-V9 writer decides whether the current block data
    /// is exactly representable in that persisted format. For example, V8 and V9 paletted data can be
    /// rewritten between those versions, while writing paletted BlockStates to V2-V7 currently fails
    /// until an authoritative BlockState-to-numeric-id/meta reverse mapping is supplied.
    ///
    /// Unknown target versions cannot be synthesized. They may only be written when this value already
    /// holds raw bytes for that exact unknown version, preserving those bytes verbatim.
    pub fn encode(&self, target: SubChunkVersion) -> Result<Bytes> {
        match target {
            SubChunkVersion::V0 => self.write_v0(),
            SubChunkVersion::V1 => self.write_v1(),
            SubChunkVersion::V2 => self.write_v2(),
            SubChunkVersion::V3 => self.write_v3(),
            SubChunkVersion::V4 => self.write_v4(),
            SubChunkVersion::V5 => self.write_v5(),
            SubChunkVersion::V6 => self.write_v6(),
            SubChunkVersion::V7 => self.write_v7(),
            SubChunkVersion::V8 => self.write_v8(),
            SubChunkVersion::V9 => self.write_v9(),
            SubChunkVersion::Unknown(target_version) => match &self.format {
                SubChunkFormat::Raw {
                    version: Some(source_version),
                    bytes,
                } if *source_version == target_version => Ok(bytes.clone()),
                _ => Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "cannot synthesize unknown SubChunk V{target_version}; only identical retained raw bytes may be written"
                ))),
            },
        }
    }

    /// Writes this block data as SubChunk V0 when it is representable exactly.
    pub fn write_v0(&self) -> Result<Bytes> {
        super::v0::write(self)
    }

    /// Writes this block data as SubChunk V1 when it is representable exactly.
    pub fn write_v1(&self) -> Result<Bytes> {
        super::v1::write(self)
    }

    /// Writes this block data as SubChunk V2 when it is representable exactly.
    pub fn write_v2(&self) -> Result<Bytes> {
        super::v2_v7::write(2, self)
    }

    /// Writes this block data as SubChunk V3 when it is representable exactly.
    pub fn write_v3(&self) -> Result<Bytes> {
        super::v2_v7::write(3, self)
    }

    /// Writes this block data as SubChunk V4 when it is representable exactly.
    pub fn write_v4(&self) -> Result<Bytes> {
        super::v2_v7::write(4, self)
    }

    /// Writes this block data as SubChunk V5 when it is representable exactly.
    pub fn write_v5(&self) -> Result<Bytes> {
        super::v2_v7::write(5, self)
    }

    /// Writes this block data as SubChunk V6 when it is representable exactly.
    pub fn write_v6(&self) -> Result<Bytes> {
        super::v2_v7::write(6, self)
    }

    /// Writes this block data as SubChunk V7 when it is representable exactly.
    pub fn write_v7(&self) -> Result<Bytes> {
        super::v2_v7::write(7, self)
    }

    /// Writes this block data as SubChunk V8 when it is representable exactly.
    pub fn write_v8(&self) -> Result<Bytes> {
        super::v8::write(self)
    }

    /// Writes this block data as SubChunk V9 when it is representable exactly.
    pub fn write_v9(&self) -> Result<Bytes> {
        super::v9::write(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{BlockPalette, BlockState, LegacySubChunkBuilder};
    use std::collections::BTreeMap;

    fn paletted_subchunk(version: u8, y: i8) -> SubChunk {
        let air = BlockState {
            name: "minecraft:air".to_string(),
            states: BTreeMap::new(),
            version: Some(18_168_865),
        };
        SubChunk {
            y,
            format: SubChunkFormat::Paletted {
                version,
                storages: vec![BlockPalette::with_unpacked_indices(
                    vec![air],
                    vec![0; 4096],
                    Some(vec![4096_u16]),
                )],
            },
        }
    }

    #[test]
    fn detects_v0_through_v9() {
        for version in 0_u8..=9 {
            assert_eq!(SubChunkVersion::from_byte(version).byte(), version);
        }
        assert_eq!(SubChunkVersion::from_byte(10), SubChunkVersion::Unknown(10));
    }

    #[test]
    fn v7_reads_and_writes_as_v7() {
        let legacy = LegacySubChunkBuilder::zeroed(7, false)
            .unwrap()
            .build()
            .unwrap();
        let raw = legacy.raw().clone();
        let subchunk = SubChunk::read(0, raw.clone(), SubChunkDecodeMode::FullIndices).unwrap();
        assert_eq!(subchunk.version(), Some(SubChunkVersion::V7));
        assert_eq!(subchunk.write().unwrap(), raw);
    }

    #[test]
    fn legacy_subchunk_versions_roundtrip_without_implicit_upgrade() {
        for version in [0_u8, 2, 3, 4, 5, 6, 7] {
            let legacy = LegacySubChunkBuilder::zeroed(version, false)
                .unwrap()
                .build()
                .unwrap();
            let raw = legacy.raw().clone();
            let subchunk = SubChunk::read(0, raw.clone(), SubChunkDecodeMode::FullIndices).unwrap();
            assert_eq!(
                subchunk.version(),
                Some(SubChunkVersion::from_byte(version))
            );
            assert_eq!(subchunk.write().unwrap(), raw);
        }
    }

    #[test]
    fn paletted_subchunk_versions_roundtrip_through_their_native_writers() {
        for version in [1_u8, 8, 9] {
            let source = paletted_subchunk(version, -2);
            let encoded = source
                .encode(SubChunkVersion::from_byte(version))
                .unwrap();
            let parsed =
                SubChunk::read(-2, encoded.clone(), SubChunkDecodeMode::FullIndices).unwrap();
            assert_eq!(parsed.version(), Some(SubChunkVersion::from_byte(version)));
            assert_eq!(parsed.write().unwrap(), encoded);
        }
    }

    #[test]
    fn explicit_v8_v9_writes_preserve_paletted_block_data() {
        let subchunk = paletted_subchunk(8, -2);
        let v8 = subchunk
            .encode(SubChunkVersion::V8)
            .expect("write V8");
        assert_eq!(v8.first().copied(), Some(8));

        let parsed = SubChunk::read(-2, v8, SubChunkDecodeMode::FullIndices).expect("read V8");
        let v9 = parsed
            .encode(SubChunkVersion::V9)
            .expect("write V9");
        assert_eq!(v9.first().copied(), Some(9));

        let parsed = SubChunk::read(-2, v9, SubChunkDecodeMode::FullIndices).expect("read V9");
        let v8_again = parsed
            .encode(SubChunkVersion::V8)
            .expect("write V8 again");
        assert_eq!(v8_again.first().copied(), Some(8));
        let parsed =
            SubChunk::read(-2, v8_again, SubChunkDecodeMode::FullIndices).expect("read V8 again");
        assert_eq!(
            parsed
                .block_state_at(0, 0, 0)
                .map(|state| state.name.as_str()),
            Some("minecraft:air")
        );
    }

    #[test]
    fn legacy_cross_version_write_refuses_missing_numeric_reverse_mapping() {
        let legacy = LegacySubChunkBuilder::zeroed(7, false)
            .unwrap()
            .build()
            .unwrap();
        let raw = legacy.raw().clone();
        let subchunk = SubChunk::read(0, raw, SubChunkDecodeMode::FullIndices).unwrap();
        assert!(subchunk.encode(SubChunkVersion::V2).is_err());
    }

    #[test]
    fn unknown_version_only_roundtrips_the_same_raw_target() {
        let raw = Bytes::from_static(&[10, 1, 0]);
        let subchunk = SubChunk::read(0, raw.clone(), SubChunkDecodeMode::FullIndices).unwrap();
        assert_eq!(subchunk.version(), Some(SubChunkVersion::Unknown(10)));
        assert_eq!(subchunk.write().unwrap(), raw);
        assert_eq!(
            subchunk
                .encode(SubChunkVersion::Unknown(10))
                .unwrap(),
            raw
        );
        assert!(
            subchunk
                .encode(SubChunkVersion::Unknown(11))
                .is_err()
        );

        let generic = SubChunk::read(0, raw.clone(), SubChunkDecodeMode::FullIndices).unwrap();
        assert_eq!(generic.version(), Some(SubChunkVersion::Unknown(10)));
        assert_eq!(generic.write().unwrap(), raw);
    }
}
