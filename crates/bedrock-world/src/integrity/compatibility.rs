//! Bedrock world data compatibility and integrity classification.
//!
//! Historical Bedrock data is not treated as pending upgrade work. A known historical record is a
//! normal supported representation when this crate can read and write that representation. Upgrade
//! and downgrade are separate caller-requested operations and do not participate in this classification.

use crate::chunk::{ChunkRecord, ChunkRecordTag, SubChunkFormat, SubChunkVersion};
use crate::entity::ActorSource;
use crate::world::WorldFormat;
use serde::{Deserialize, Serialize};

/// Safety level of persisted Bedrock data relative to the implementations in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompatibilityLevel {
    /// The persisted representation is understood and can be round-tripped by its matching writer.
    Exact,
    /// The data can be inspected safely, but its raw representation should be retained for writes.
    ReadCompatible,
    /// The data uses a newer/unknown persisted representation. Raw bytes must be preserved.
    UnsupportedFuture,
    /// The data is malformed or internally inconsistent.
    Corrupt,
}

impl CompatibilityLevel {
    /// Returns whether structured reads are safe.
    #[must_use]
    pub const fn readable(self) -> bool {
        !matches!(self, Self::Corrupt)
    }

    /// Returns whether the same persisted representation can be rewritten directly.
    #[must_use]
    pub const fn directly_writable(self) -> bool {
        matches!(self, Self::Exact)
    }

    /// Returns whether raw bytes should be retained even when a structured view is available.
    #[must_use]
    pub const fn should_preserve_raw(self) -> bool {
        matches!(self, Self::ReadCompatible | Self::UnsupportedFuture)
    }
}

/// Actor records actually observed in Bedrock storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActorStorage {
    /// Chunk-scoped `Entity` records containing consecutive actor NBT roots.
    Entity,
    /// `digp<ChunkKey>` references with `actorprefix<ActorUniqueID>` payloads.
    DigpActorprefix,
    /// Both `Entity` and `digp`/`actorprefix` records are present.
    Mixed,
    /// No actor storage representation was observed.
    Unknown,
}

impl ActorStorage {
    /// Combines observed actor storage without discarding evidence of mixed worlds.
    #[must_use]
    pub const fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, value) | (value, Self::Unknown) => value,
            (Self::Entity, Self::Entity) => Self::Entity,
            (Self::DigpActorprefix, Self::DigpActorprefix) => Self::DigpActorprefix,
            (Self::Mixed, _) | (_, Self::Mixed) => Self::Mixed,
            _ => Self::Mixed,
        }
    }
}

impl ActorSource {
    /// Returns the Bedrock storage representation that produced this actor.
    #[must_use]
    pub const fn actor_storage(&self) -> ActorStorage {
        match self {
            Self::InlineChunk(_) => ActorStorage::Entity,
            Self::ActorPrefix(_) => ActorStorage::DigpActorprefix,
        }
    }
}

impl SubChunkFormat {
    /// Returns the safety level for this decoded SubChunk representation.
    #[must_use]
    pub const fn compatibility(&self) -> CompatibilityLevel {
        match self {
            Self::Raw {
                version: Some(version),
                ..
            } if *version > 9 => CompatibilityLevel::UnsupportedFuture,
            Self::Raw { .. } => CompatibilityLevel::ReadCompatible,
            Self::LegacySubChunk(_)
            | Self::LegacyTerrain
            | Self::FixedArrayV1
            | Self::Paletted { version: 0..=9, .. } => CompatibilityLevel::Exact,
            Self::Paletted { .. } => CompatibilityLevel::UnsupportedFuture,
        }
    }
}

/// Storage facts known immediately after opening a world folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldCapabilities {
    /// Detected physical world storage family.
    pub format: WorldFormat,
    /// Baseline safety inferred from the storage container alone.
    pub compatibility: CompatibilityLevel,
    /// Whether unknown records can be retained by the storage/parser stack.
    pub preserves_unknown_records: bool,
    /// Whether this is a pre-LevelDB `chunks.dat` world.
    pub pocket_chunks_dat: bool,
    /// Whether `LegacyTerrain` is expected.
    pub legacy_terrain: bool,
    /// Whether a Bedrock LevelDB is present.
    pub leveldb: bool,
}

impl WorldCapabilities {
    /// Builds baseline facts from the detected world storage family.
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
                compatibility: CompatibilityLevel::Exact,
                preserves_unknown_records: true,
                pocket_chunks_dat: false,
                legacy_terrain: true,
                leveldb: true,
            },
            WorldFormat::PocketChunksDat => Self {
                format,
                // The current chunks.dat backend intentionally exposes exact reads but is read-only.
                compatibility: CompatibilityLevel::ReadCompatible,
                preserves_unknown_records: true,
                pocket_chunks_dat: true,
                legacy_terrain: true,
                leveldb: false,
            },
        }
    }
}

impl WorldFormat {
    /// Returns baseline storage facts for this detected world family.
    #[must_use]
    pub const fn capabilities(self) -> WorldCapabilities {
        WorldCapabilities::from_format(self)
    }
}

/// Persisted Bedrock data observed in one chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkCapabilities {
    /// Overall safety of the inspected records.
    pub compatibility: CompatibilityLevel,
    /// Actor storage observed in this chunk.
    pub actor_storage: ActorStorage,
    /// Whether a chunk version record is present.
    pub has_version_record: bool,
    /// Whether a `LegacyTerrain` record is present.
    pub has_legacy_terrain: bool,
    /// Whether a chunk-scoped `Entity` record is present.
    pub has_entity: bool,
    /// Whether `Data2D`/`Data2DLegacy` is present.
    pub has_data2d: bool,
    /// Whether `Data3D` is present.
    pub has_data3d: bool,
    /// Whether unknown chunk record tags were retained.
    pub has_unknown_records: bool,
    /// Whether a SubChunk record had no readable leading version byte.
    pub has_unversioned_subchunk: bool,
    /// Actual SubChunk version bytes observed in this chunk.
    pub subchunk_versions: Vec<SubChunkVersion>,
}

impl ChunkCapabilities {
    /// Inspects raw chunk records without mutating or normalising them.
    #[must_use]
    pub fn inspect(records: &[ChunkRecord]) -> Self {
        let mut capabilities = Self {
            compatibility: CompatibilityLevel::Exact,
            actor_storage: ActorStorage::Unknown,
            has_version_record: false,
            has_legacy_terrain: false,
            has_entity: false,
            has_data2d: false,
            has_data3d: false,
            has_unknown_records: false,
            has_unversioned_subchunk: false,
            subchunk_versions: Vec::new(),
        };

        for record in records {
            match record.key.tag {
                ChunkRecordTag::Version
                | ChunkRecordTag::VersionOld
                | ChunkRecordTag::LegacyVersion => capabilities.has_version_record = true,
                ChunkRecordTag::LegacyTerrain => capabilities.has_legacy_terrain = true,
                ChunkRecordTag::Entity => {
                    capabilities.has_entity = true;
                    capabilities.actor_storage = capabilities.actor_storage.merge(ActorStorage::Entity);
                }
                ChunkRecordTag::Data2D | ChunkRecordTag::Data2DLegacy => {
                    capabilities.has_data2d = true;
                }
                ChunkRecordTag::Data3D => capabilities.has_data3d = true,
                ChunkRecordTag::SubChunkPrefix => match SubChunkVersion::detect(&record.value) {
                    Some(version @ SubChunkVersion::Unknown(_)) => {
                        capabilities.compatibility = merge_compatibility(
                            capabilities.compatibility,
                            CompatibilityLevel::UnsupportedFuture,
                        );
                        capabilities.subchunk_versions.push(version);
                    }
                    Some(version) => capabilities.subchunk_versions.push(version),
                    None => {
                        capabilities.has_unversioned_subchunk = true;
                        capabilities.compatibility = merge_compatibility(
                            capabilities.compatibility,
                            CompatibilityLevel::ReadCompatible,
                        );
                    }
                },
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
        capabilities.subchunk_versions.sort_by_key(|version| version.byte());
        capabilities.subchunk_versions.dedup();
        capabilities
    }

    /// Returns whether all inspected records are safe for same-representation structured writes.
    #[must_use]
    pub const fn directly_writable(&self) -> bool {
        self.compatibility.directly_writable()
    }
}

const fn merge_compatibility(
    left: CompatibilityLevel,
    right: CompatibilityLevel,
) -> CompatibilityLevel {
    use CompatibilityLevel::{Corrupt, Exact, ReadCompatible, UnsupportedFuture};
    match (left, right) {
        (Corrupt, _) | (_, Corrupt) => Corrupt,
        (UnsupportedFuture, _) | (_, UnsupportedFuture) => UnsupportedFuture,
        (ReadCompatible, _) | (_, ReadCompatible) => ReadCompatible,
        (Exact, Exact) => Exact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{ChunkKey, ChunkPos, Dimension};
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
    fn historical_known_records_are_normal_supported_data() {
        let capabilities = ChunkCapabilities::inspect(&[
            record(ChunkRecordTag::LegacyTerrain, None, &[0]),
            record(ChunkRecordTag::Entity, None, &[0]),
            record(ChunkRecordTag::SubChunkPrefix, Some(0), &[7, 0, 0]),
        ]);
        assert_eq!(capabilities.compatibility, CompatibilityLevel::Exact);
        assert_eq!(capabilities.actor_storage, ActorStorage::Entity);
        assert_eq!(capabilities.subchunk_versions, vec![SubChunkVersion::V7]);
    }

    #[test]
    fn future_subchunk_requires_raw_preservation() {
        let capabilities = ChunkCapabilities::inspect(&[record(
            ChunkRecordTag::SubChunkPrefix,
            Some(0),
            &[10, 1, 0],
        )]);
        assert_eq!(
            capabilities.compatibility,
            CompatibilityLevel::UnsupportedFuture
        );
        assert_eq!(
            capabilities.subchunk_versions,
            vec![SubChunkVersion::Unknown(10)]
        );
    }
}
