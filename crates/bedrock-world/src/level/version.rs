//! World-folder version evidence from `level.dat` and persisted Minecraft Bedrock records.
//!
//! `level.dat` is useful version evidence, but partially upgraded and mixed-version worlds can contain
//! records from several storage generations at the same time. This module therefore reports literal
//! on-disk evidence instead of deriving one synthetic world format version.

use crate::chunk::{BedrockDbKey, ChunkRecordTag, SubChunkVersion};
use crate::error::{BedrockWorldError, Result};
use crate::storage::{StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::version::{GameVersion, LevelVersion};
use crate::world::{World, WorldFormat, StorageBackend};
use std::path::Path;

/// Count for one actual SubChunk version observed in a world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubChunkVersionCount {
    /// Persisted SubChunk version byte.
    pub version: SubChunkVersion,
    /// Number of SubChunk records using this version.
    pub records: usize,
}

/// Count for one persisted `LevelChunk` version byte observed under `Version`, `VersionOld` or
/// `LegacyVersion` chunk records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelChunkVersionCount {
    /// Exact persisted `LevelChunk` version byte.
    pub version: u8,
    /// Number of chunk version records carrying this byte.
    pub records: usize,
}

/// Actual version and storage-generation evidence observed in one Bedrock world folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldVersions {
    /// Version values read directly from `level.dat`.
    pub level: LevelVersion,
    /// Physical world storage family detected from the folder.
    pub world_format: WorldFormat,
    /// `LevelChunk` version bytes observed in `Version`, `VersionOld` and `LegacyVersion` records.
    pub level_chunk_versions: Vec<LevelChunkVersionCount>,
    /// Number of chunk version records with no readable leading version byte.
    pub unversioned_level_chunks: usize,
    /// SubChunk versions observed in database records, ordered by persisted version byte.
    pub subchunks: Vec<SubChunkVersionCount>,
    /// Number of SubChunk records with no readable leading version byte.
    pub unversioned_subchunks: usize,
    /// Number of `LegacyTerrain` records.
    pub legacy_terrain_records: usize,
    /// Number of historical `BlockExtraData` (`0x34`) second-block-layer records.
    pub block_extra_data_records: usize,
    /// Number of `Data2D` records.
    pub data2d_records: usize,
    /// Number of `Data2DLegacy` records.
    pub data2d_legacy_records: usize,
    /// Number of `Data3D` records.
    pub data3d_records: usize,
    /// Number of chunk `BlockEntity` records.
    pub block_entity_records: usize,
    /// Number of chunk-scoped `Entity` records.
    pub entity_records: usize,
    /// Number of `digp` records.
    pub digp_records: usize,
    /// Number of `actorprefix` records.
    pub actorprefix_records: usize,
    /// Number of chunk `ActorDigestVersion` records.
    pub actor_digest_version_records: usize,
    /// Number of chunk records carrying an unknown chunk tag byte.
    pub unknown_chunk_tag_records: usize,
    /// Number of database keys that cannot be classified by the Bedrock key decoder.
    pub unknown_database_key_records: usize,
}

impl WorldVersions {
    /// Returns the exact last-opened Minecraft Bedrock version from `level.dat`, when present.
    #[must_use]
    pub fn game_version(&self) -> Option<&GameVersion> {
        self.level.last_opened_with.as_ref()
    }

    /// Returns whether more than one persisted `LevelChunk` version byte exists.
    #[must_use]
    pub fn has_mixed_level_chunk_versions(&self) -> bool {
        self.level_chunk_versions.len() > 1
    }

    /// Returns whether more than one SubChunk version is persisted in this world.
    #[must_use]
    pub fn has_mixed_subchunk_versions(&self) -> bool {
        self.subchunks.len() > 1
    }

    /// Returns whether pre-SubChunk `LegacyTerrain` and SubChunk terrain coexist.
    #[must_use]
    pub fn has_mixed_terrain_storage(&self) -> bool {
        self.legacy_terrain_records != 0 && !self.subchunks.is_empty()
    }

    /// Returns whether more than one of `Data2DLegacy`, `Data2D` and `Data3D` is present.
    #[must_use]
    pub const fn has_mixed_biome_storage(&self) -> bool {
        let mut generations = 0_u8;
        if self.data2d_legacy_records != 0 {
            generations += 1;
        }
        if self.data2d_records != 0 {
            generations += 1;
        }
        if self.data3d_records != 0 {
            generations += 1;
        }
        generations > 1
    }

    /// Returns whether both chunk `Entity` and `digp`/`actorprefix` actor storage exist.
    #[must_use]
    pub const fn has_mixed_actor_storage(&self) -> bool {
        self.entity_records != 0 && (self.digp_records != 0 || self.actorprefix_records != 0)
    }

    /// Returns whether any unknown SubChunk version was observed.
    #[must_use]
    pub fn has_unknown_subchunk_version(&self) -> bool {
        self.subchunks
            .iter()
            .any(|entry| matches!(entry.version, SubChunkVersion::Unknown(_)))
    }

    /// Returns whether storage contains bytes whose structured meaning is newer or unknown to this
    /// library. Such records can still be retained by raw-preserving read/write paths.
    #[must_use]
    pub fn has_future_storage(&self) -> bool {
        self.has_unknown_subchunk_version()
            || self.unknown_chunk_tag_records != 0
            || self.unknown_database_key_records != 0
    }

    /// Returns whether concrete record evidence spans more than one Bedrock storage generation.
    #[must_use]
    pub fn has_mixed_version_storage(&self) -> bool {
        self.has_mixed_level_chunk_versions()
            || self.has_mixed_subchunk_versions()
            || self.has_mixed_terrain_storage()
            || self.has_mixed_biome_storage()
            || self.has_mixed_actor_storage()
    }
}

impl<S> World<S>
where
    S: StorageBackend,
{
    /// Reads the real version values persisted in this world's `level.dat`.
    pub fn level_version(&self) -> Result<LevelVersion> {
        LevelVersion::detect(&self.read_level_dat()?)
    }

    /// Scans persisted records once and returns actual Bedrock version/storage-generation evidence.
    ///
    /// The scan is observational only. It does not upgrade, downgrade, normalise or rewrite records.
    pub fn versions(&self) -> Result<WorldVersions> {
        let level = self.level_version()?;
        let mut level_chunk_version_counts = [0usize; 256];
        let mut unversioned_level_chunks = 0usize;
        let mut subchunk_counts = [0usize; 256];
        let mut unversioned_subchunks = 0usize;
        let mut legacy_terrain_records = 0usize;
        let mut block_extra_data_records = 0usize;
        let mut data2d_records = 0usize;
        let mut data2d_legacy_records = 0usize;
        let mut data3d_records = 0usize;
        let mut block_entity_records = 0usize;
        let mut entity_records = 0usize;
        let mut digp_records = 0usize;
        let mut actorprefix_records = 0usize;
        let mut actor_digest_version_records = 0usize;
        let mut unknown_chunk_tag_records = 0usize;
        let mut unknown_database_key_records = 0usize;

        self.storage()
            .for_each_entry(StorageReadOptions::default(), &mut |key, value| {
                match BedrockDbKey::decode(key) {
                    BedrockDbKey::Chunk(chunk) => match chunk.tag {
                        ChunkRecordTag::Version
                        | ChunkRecordTag::VersionOld
                        | ChunkRecordTag::LegacyVersion => {
                            if let Some(version) = value.first().copied() {
                                let slot = &mut level_chunk_version_counts[usize::from(version)];
                                *slot = slot.saturating_add(1);
                            } else {
                                unversioned_level_chunks =
                                    unversioned_level_chunks.saturating_add(1);
                            }
                        }
                        ChunkRecordTag::SubChunkPrefix => {
                            if let Some(version) = value.first().copied() {
                                let slot = &mut subchunk_counts[usize::from(version)];
                                *slot = slot.saturating_add(1);
                            } else {
                                unversioned_subchunks = unversioned_subchunks.saturating_add(1);
                            }
                        }
                        ChunkRecordTag::LegacyTerrain => {
                            legacy_terrain_records = legacy_terrain_records.saturating_add(1);
                        }
                        ChunkRecordTag::BlockExtraData => {
                            block_extra_data_records = block_extra_data_records.saturating_add(1);
                        }
                        ChunkRecordTag::Data2D => {
                            data2d_records = data2d_records.saturating_add(1);
                        }
                        ChunkRecordTag::Data2DLegacy => {
                            data2d_legacy_records = data2d_legacy_records.saturating_add(1);
                        }
                        ChunkRecordTag::Data3D => {
                            data3d_records = data3d_records.saturating_add(1);
                        }
                        ChunkRecordTag::BlockEntity => {
                            block_entity_records = block_entity_records.saturating_add(1);
                        }
                        ChunkRecordTag::Entity => {
                            entity_records = entity_records.saturating_add(1);
                        }
                        ChunkRecordTag::ActorDigestVersion => {
                            actor_digest_version_records =
                                actor_digest_version_records.saturating_add(1);
                        }
                        ChunkRecordTag::Unknown(_) => {
                            unknown_chunk_tag_records = unknown_chunk_tag_records.saturating_add(1);
                        }
                        _ => {}
                    },
                    BedrockDbKey::ActorDigest { .. } => {
                        digp_records = digp_records.saturating_add(1);
                    }
                    BedrockDbKey::ActorPrefix { .. } => {
                        actorprefix_records = actorprefix_records.saturating_add(1);
                    }
                    BedrockDbKey::Unknown(_) => {
                        unknown_database_key_records =
                            unknown_database_key_records.saturating_add(1);
                    }
                    _ => {}
                }
                Ok(StorageVisitorControl::Continue)
            })?;

        let level_chunk_versions = level_chunk_version_counts
            .into_iter()
            .enumerate()
            .filter_map(|(version, records)| {
                (records != 0).then_some(LevelChunkVersionCount {
                    version: version as u8,
                    records,
                })
            })
            .collect();
        let subchunks = subchunk_counts
            .into_iter()
            .enumerate()
            .filter_map(|(version, records)| {
                (records != 0).then_some(SubChunkVersionCount {
                    version: SubChunkVersion::from_byte(version as u8),
                    records,
                })
            })
            .collect();

        Ok(WorldVersions {
            level,
            world_format: self.format(),
            level_chunk_versions,
            unversioned_level_chunks,
            subchunks,
            unversioned_subchunks,
            legacy_terrain_records,
            block_extra_data_records,
            data2d_records,
            data2d_legacy_records,
            data3d_records,
            block_entity_records,
            entity_records,
            digp_records,
            actorprefix_records,
            actor_digest_version_records,
            unknown_chunk_tag_records,
            unknown_database_key_records,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_counts_keep_actual_bytes() {
        let subchunk = SubChunkVersionCount {
            version: SubChunkVersion::V7,
            records: 3,
        };
        let level_chunk = LevelChunkVersionCount {
            version: 40,
            records: 8,
        };
        assert_eq!(subchunk.version.byte(), 7);
        assert_eq!(subchunk.records, 3);
        assert_eq!(level_chunk.version, 40);
        assert_eq!(level_chunk.records, 8);
    }
}
