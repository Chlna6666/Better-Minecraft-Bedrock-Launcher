//! Bedrock world-format compatibility and capability classification.
//!
//! Compatibility is intentionally capability-based rather than assuming one global world version.
//! Real Bedrock worlds may contain records written by multiple game generations after partial
//! upgrades. Callers should inspect the concrete world/chunk/subchunk data and choose an explicit
//! [`WritePolicy`] before mutating historical or future-format data.

use crate::{ActorSource, ChunkRecord, ChunkRecordTag, SubChunkFormat, WorldFormat};
use serde::{Deserialize, Serialize};

/// Compatibility level of decoded Bedrock data relative to the codecs implemented by this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompatibilityLevel {
    /// The data is fully understood and may be round-tripped by the matching codec.
    Exact,
    /// The data can be decoded safely, but callers should preserve its historical representation.
    ReadCompatible,
    /// The data is understood but should be migrated before normal modern writes are performed.
    MigrationRequired,
    /// The data is from a newer/unknown format. Raw bytes must be preserved and destructive writes refused.
    UnsupportedFuture,
    /// The data is malformed or internally inconsistent.
    Corrupt,
}

impl CompatibilityLevel {
    /// Returns whether normal structured reads are safe.
    #[must_use]
    pub const fn readable(self) -> bool {
        !matches!(self, Self::Corrupt)
    }

    /// Returns whether a direct in-place structured rewrite is safe without migration.
    #[must_use]
    pub const fn directly_writable(self) -> bool {
        matches!(self, Self::Exact)
    }

    /// Returns whether raw bytes should be retained even when a structured view is available.
    #[must_use]
    pub const fn should_preserve_raw(self) -> bool {
        matches!(
            self,
            Self::ReadCompatible | Self::MigrationRequired | Self::UnsupportedFuture
        )
    }
}

/// Explicit mutation policy for historical or unknown Bedrock data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WritePolicy {
    /// Preserve the existing physical format. Mutations requiring format conversion are refused.
    #[default]
    Preserve,
    /// Explicitly migrate understood historical data to the caller's target format before writing.
    Migrate,
    /// Refuse all structured mutation. Useful for forensic/read-only tooling and unknown future worlds.
    Refuse,
}

impl WritePolicy {
    /// Returns whether this policy permits a mutation at the supplied compatibility level.
    #[must_use]
    pub const fn permits(self, compatibility: CompatibilityLevel) -> bool {
        match self {
            Self::Refuse => false,
            Self::Preserve => matches!(compatibility, CompatibilityLevel::Exact),
            Self::Migrate => matches!(
                compatibility,
                CompatibilityLevel::Exact
                    | CompatibilityLevel::ReadCompatible
                    | CompatibilityLevel::MigrationRequired
            ),
        }
    }
}

/// Physical actor-storage generation used by one decoded actor or world scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActorStorageModel {
    /// Legacy inline `Entity` chunk records containing consecutive actor NBT roots.
    LegacyInline,
    /// Modern `digp<ChunkKey>` digests referencing `actorprefix<ActorUniqueID>` records.
    ModernDigest,
    /// Both legacy and modern actor records are present in the same inspected scope.
    Mixed,
    /// No actor storage model could be established from the inspected data.
    Unknown,
}

impl ActorStorageModel {
    /// Combines two observed storage models without discarding evidence of mixed-format worlds.
    #[must_use]
    pub const fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, value) | (value, Self::Unknown) => value,
            (Self::LegacyInline, Self::LegacyInline) => Self::LegacyInline,
            (Self::ModernDigest, Self::ModernDigest) => Self::ModernDigest,
            (Self::Mixed, _) | (_, Self::Mixed) => Self::Mixed,
            _ => Self::Mixed,
        }
    }
}

impl ActorSource {
    /// Returns the physical actor storage model represented by this parsed source.
    #[must_use]
    pub const fn storage_model(&self) -> ActorStorageModel {
        match self {
            Self::InlineChunk(_) => ActorStorageModel::LegacyInline,
            Self::ActorPrefix(_) => ActorStorageModel::ModernDigest,
        }
    }
}

/// Historical subchunk codec family inferred from the on-disk payload version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubChunkCodecKind {
    /// Legacy fixed-array version 0 payload.
    LegacyV0,
    /// Paletted version 1 payload.
    PalettedV1,
    /// Historical fixed-array versions 2 through 7.
    LegacyV2ToV7(u8),
    /// Paletted version 8 payload.
    PalettedV8,
    /// Paletted version 9 payload carrying an explicit subchunk Y byte.
    PalettedV9,
    /// A version newer than the codecs known by this crate. Raw bytes must be preserved.
    UnknownFuture(u8),
    /// A historical/invalid version not covered by the known codec families.
    UnknownLegacy(u8),
    /// No version byte was available.
    Unknown,
}

impl SubChunkCodecKind {
    /// Classifies a raw subchunk version byte without decoding the payload.
    #[must_use]
    pub const fn from_version(version: Option<u8>) -> Self {
        match version {
            Some(0) => Self::LegacyV0,
            Some(1) => Self::PalettedV1,
            Some(version @ 2..=7) => Self::LegacyV2ToV7(version),
            Some(8) => Self::PalettedV8,
            Some(9) => Self::PalettedV9,
            Some(version @ 10..) => Self::UnknownFuture(version),
            None => Self::Unknown,
        }
    }

    /// Returns the compatibility level implied by this codec family.
    #[must_use]
    pub const fn compatibility(self) -> CompatibilityLevel {
        match self {
            Self::PalettedV8 | Self::PalettedV9 => CompatibilityLevel::Exact,
            Self::LegacyV0 | Self::PalettedV1 | Self::LegacyV2ToV7(_) => {
                CompatibilityLevel::MigrationRequired
            }
            Self::UnknownFuture(_) => CompatibilityLevel::UnsupportedFuture,
            Self::UnknownLegacy(_) | Self::Unknown => CompatibilityLevel::ReadCompatible,
        }
    }
}

impl SubChunkFormat {
    /// Returns the historical codec family represented by this decoded subchunk value.
    #[must_use]
    pub const fn codec_kind(&self) -> SubChunkCodecKind {
        match self {
            Self::LegacySubChunk(subchunk) => {
                SubChunkCodecKind::from_version(Some(subchunk.version()))
            }
            Self::LegacyTerrain => SubChunkCodecKind::UnknownLegacy(0xff),
            Self::FixedArrayV1 => SubChunkCodecKind::PalettedV1,
            Self::Paletted { version, .. } => SubChunkCodecKind::from_version(Some(*version)),
            Self::Raw { version, .. } => SubChunkCodecKind::from_version(*version),
        }
    }

    /// Returns the compatibility level for this decoded subchunk.
    #[must_use]
    pub const fn compatibility(&self) -> CompatibilityLevel {
        match self {
            Self::Raw {
                version: Some(version),
                ..
            } if *version > 9 => CompatibilityLevel::UnsupportedFuture,
            Self::Raw { .. } => CompatibilityLevel::ReadCompatible,
            Self::LegacySubChunk(_) | Self::LegacyTerrain | Self::FixedArrayV1 => {
                CompatibilityLevel::MigrationRequired
            }
            Self::Paletted { version: 8 | 9, .. } => CompatibilityLevel::Exact,
            Self::Paletted { version, .. } if *version > 9 => {
                CompatibilityLevel::UnsupportedFuture
            }
            Self::Paletted { .. } => CompatibilityLevel::ReadCompatible,
        }
    }
}

/// Coarse storage capabilities known immediately after opening a world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldCapabilities {
    /// Detected physical world storage format.
    pub format: WorldFormat,
    /// Baseline compatibility inferred from the storage format alone.
    pub compatibility: CompatibilityLevel,
    /// Whether raw unknown records can be preserved by the storage/parser stack.
    pub preserves_unknown_records: bool,
    /// Whether the storage belongs to the pre-LevelDB `chunks.dat` family.
    pub pocket_chunks_dat: bool,
    /// Whether legacy LevelDB `LegacyTerrain` records are expected.
    pub legacy_terrain: bool,
    /// Whether normal modern LevelDB records are available.
    pub leveldb: bool,
}

impl WorldCapabilities {
    /// Builds baseline capabilities from the detected world storage format.
    #[must_use]
    pub const fn from_format(format: WorldFormat) -> Self {
        match format {
            WorldFormat::LevelDb => Self {
                format,
                compatibility: CompatibilityLevel::Exact,
                preserves_unknown_records: true,
                pocket_chunks_dat: false,
                legacy_terrain: false,
                leveldb: true,
            },
            WorldFormat::LevelDbLegacyTerrain => Self {
                format,
                compatibility: CompatibilityLevel::MigrationRequired,
                preserves_unknown_records: true,
                pocket_chunks_dat: false,
                legacy_terrain: true,
                leveldb: true,
            },
            WorldFormat::PocketChunksDat => Self {
                format,
                compatibility: CompatibilityLevel::MigrationRequired,
                preserves_unknown_records: true,
                pocket_chunks_dat: true,
                legacy_terrain: true,
                leveldb: false,
            },
        }
    }
}

impl WorldFormat {
    /// Returns baseline capabilities for this detected storage format.
    ///
    /// Real worlds may contain mixed historical records, so callers performing writes should also
    /// inspect per-chunk capabilities rather than treating this as a global schema version.
    #[must_use]
    pub const fn capabilities(self) -> WorldCapabilities {
        WorldCapabilities::from_format(self)
    }
}

/// Capabilities and historical-format evidence collected from one chunk's records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkCapabilities {
    /// Overall compatibility of the inspected chunk.
    pub compatibility: CompatibilityLevel,
    /// Recommended actor storage model based on inline entity evidence in this chunk.
    pub actor_storage: ActorStorageModel,
    /// Whether a modern/legacy chunk version record is present.
    pub has_version_record: bool,
    /// Whether a `LegacyTerrain` record is present.
    pub has_legacy_terrain: bool,
    /// Whether a legacy inline `Entity` record is present.
    pub has_legacy_inline_entities: bool,
    /// Whether `Data2D`/`Data2DLegacy` is present.
    pub has_2d_biomes: bool,
    /// Whether modern `Data3D` is present.
    pub has_3d_biomes: bool,
    /// Whether unknown chunk record tags were preserved.
    pub has_unknown_records: bool,
    /// Subchunk codec families observed from record payload version bytes.
    pub subchunk_codecs: Vec<SubChunkCodecKind>,
}

impl ChunkCapabilities {
    /// Inspects raw chunk records without mutating or normalising them.
    #[must_use]
    pub fn inspect(records: &[ChunkRecord]) -> Self {
        let mut capabilities = Self {
            compatibility: CompatibilityLevel::Exact,
            actor_storage: ActorStorageModel::Unknown,
            has_version_record: false,
            has_legacy_terrain: false,
            has_legacy_inline_entities: false,
            has_2d_biomes: false,
            has_3d_biomes: false,
            has_unknown_records: false,
            subchunk_codecs: Vec::new(),
        };

        for record in records {
            match record.key.tag {
                ChunkRecordTag::Version
                | ChunkRecordTag::VersionOld
                | ChunkRecordTag::LegacyVersion => capabilities.has_version_record = true,
                ChunkRecordTag::LegacyTerrain => {
                    capabilities.has_legacy_terrain = true;
                    capabilities.compatibility = merge_compatibility(
                        capabilities.compatibility,
                        CompatibilityLevel::MigrationRequired,
                    );
                }
                ChunkRecordTag::Entity => {
                    capabilities.has_legacy_inline_entities = true;
                    capabilities.actor_storage = capabilities
                        .actor_storage
                        .merge(ActorStorageModel::LegacyInline);
                    capabilities.compatibility = merge_compatibility(
                        capabilities.compatibility,
                        CompatibilityLevel::MigrationRequired,
                    );
                }
                ChunkRecordTag::Data2D | ChunkRecordTag::Data2DLegacy => {
                    capabilities.has_2d_biomes = true;
                }
                ChunkRecordTag::Data3D => capabilities.has_3d_biomes = true,
                ChunkRecordTag::SubChunkPrefix => {
                    let codec = SubChunkCodecKind::from_version(record.value.first().copied());
                    capabilities.compatibility =
                        merge_compatibility(capabilities.compatibility, codec.compatibility());
                    capabilities.subchunk_codecs.push(codec);
                }
                ChunkRecordTag::Unknown(_) => {
                    capabilities.has_unknown_records = true;
                    capabilities.compatibility = merge_compatibility(
                        capabilities.compatibility,
                        CompatibilityLevel::ReadCompatible,
                    );
                }
                _ => {}
            }
        }
        capabilities
            .subchunk_codecs
            .sort_by_key(|codec| codec_sort_key(*codec));
        capabilities.subchunk_codecs.dedup();
        capabilities
    }

    /// Returns whether the supplied write policy permits a structured rewrite of this chunk.
    #[must_use]
    pub const fn writable_with(&self, policy: WritePolicy) -> bool {
        policy.permits(self.compatibility)
    }
}

const fn merge_compatibility(
    left: CompatibilityLevel,
    right: CompatibilityLevel,
) -> CompatibilityLevel {
    use CompatibilityLevel::{
        Corrupt, Exact, MigrationRequired, ReadCompatible, UnsupportedFuture,
    };
    match (left, right) {
        (Corrupt, _) | (_, Corrupt) => Corrupt,
        (UnsupportedFuture, _) | (_, UnsupportedFuture) => UnsupportedFuture,
        (MigrationRequired, _) | (_, MigrationRequired) => MigrationRequired,
        (ReadCompatible, _) | (_, ReadCompatible) => ReadCompatible,
        (Exact, Exact) => Exact,
    }
}

const fn codec_sort_key(codec: SubChunkCodecKind) -> u16 {
    match codec {
        SubChunkCodecKind::LegacyV0 => 0,
        SubChunkCodecKind::PalettedV1 => 1,
        SubChunkCodecKind::LegacyV2ToV7(version) => version as u16,
        SubChunkCodecKind::PalettedV8 => 8,
        SubChunkCodecKind::PalettedV9 => 9,
        SubChunkCodecKind::UnknownFuture(version) => 0x100 + version as u16,
        SubChunkCodecKind::UnknownLegacy(version) => 0x200 + version as u16,
        SubChunkCodecKind::Unknown => 0xffff,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChunkKey, ChunkPos, Dimension};
    use bytes::Bytes;

    fn record(tag: ChunkRecordTag, subchunk_y: Option<i8>, value: &'static [u8]) -> ChunkRecord {
        ChunkRecord {
            key: ChunkKey {
                pos: ChunkPos {
                    x: 0,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                tag,
                subchunk_y,
            },
            value: Bytes::from_static(value),
        }
    }

    #[test]
    fn modern_v9_chunk_is_directly_writable() {
        let capabilities = ChunkCapabilities::inspect(&[
            record(ChunkRecordTag::Version, None, &[40]),
            record(ChunkRecordTag::Data3D, None, &[0]),
            record(ChunkRecordTag::SubChunkPrefix, Some(0), &[9, 1, 0]),
        ]);
        assert_eq!(capabilities.compatibility, CompatibilityLevel::Exact);
        assert!(capabilities.writable_with(WritePolicy::Preserve));
    }

    #[test]
    fn legacy_and_future_data_are_not_preserve_writable() {
        let legacy = ChunkCapabilities::inspect(&[
            record(ChunkRecordTag::LegacyTerrain, None, &[0]),
            record(ChunkRecordTag::Entity, None, &[0]),
        ]);
        assert_eq!(
            legacy.compatibility,
            CompatibilityLevel::MigrationRequired
        );
        assert!(!legacy.writable_with(WritePolicy::Preserve));
        assert!(legacy.writable_with(WritePolicy::Migrate));

        let future = ChunkCapabilities::inspect(&[record(
            ChunkRecordTag::SubChunkPrefix,
            Some(0),
            &[10, 1, 0],
        )]);
        assert_eq!(
            future.compatibility,
            CompatibilityLevel::UnsupportedFuture
        );
        assert!(!future.writable_with(WritePolicy::Migrate));
    }
}
