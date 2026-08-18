//! World-folder version evidence from `level.dat` and persisted Bedrock database records.

use crate::chunk::{BedrockDbKey, ChunkRecordTag, SubChunkVersion};
use crate::database::{StorageReadOptions, StorageVisitorControl};
use crate::error::Result;
use crate::version::{GameVersion, LevelVersion};
use crate::world::{BedrockWorld, OpenOptions, WorldFormat, WorldStorageHandle};
use std::path::Path;
use std::sync::Arc;

/// Count for one actual SubChunk version observed in a world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubChunkVersionCount {
    /// Persisted SubChunk version byte.
    pub version: SubChunkVersion,
    /// Number of SubChunk records using this version.
    pub records: usize,
}

/// Actual version and record-generation evidence observed in one Bedrock world folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldVersions {
    /// Version values read directly from `level.dat`.
    pub level: LevelVersion,
    /// Physical world storage family detected from the folder.
    pub world_format: WorldFormat,
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
    /// Number of chunk-scoped `Entity` records.
    pub entity_records: usize,
    /// Number of `digp` records.
    pub digp_records: usize,
    /// Number of `actorprefix` records.
    pub actorprefix_records: usize,
}

impl WorldVersions {
    /// Returns the exact last-opened Minecraft Bedrock version from `level.dat`, when present.
    #[must_use]
    pub fn game_version(&self) -> Option<&GameVersion> {
        self.level.last_opened_with.as_ref()
    }

    /// Returns whether more than one SubChunk version is persisted in this world.
    #[must_use]
    pub fn has_mixed_subchunk_versions(&self) -> bool {
        self.subchunks.len() > 1
    }

    /// Returns whether both chunk `Entity` and `digp`/`actorprefix` actor storage exist.
    #[must_use]
    pub const fn has_mixed_actor_storage(&self) -> bool {
        self.entity_records != 0 && (self.digp_records != 0 || self.actorprefix_records != 0)
    }

    /// Returns whether any unknown SubChunk version newer than V9 was observed.
    #[must_use]
    pub fn has_unknown_subchunk_version(&self) -> bool {
        self.subchunks
            .iter()
            .any(|entry| matches!(entry.version, SubChunkVersion::Unknown(_)))
    }
}

impl BedrockWorld<Arc<dyn crate::database::WorldStorage>> {
    /// Opens a Bedrock world folder read-only with automatic storage-family detection.
    ///
    /// This is the normal developer entry point when no custom backend or write access is needed.
    pub fn open_auto_blocking(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_blocking(path, OpenOptions::default())
    }

    #[cfg(feature = "async")]
    /// Opens a Bedrock world folder read-only with automatic storage-family detection.
    pub async fn open_auto(path: impl AsRef<Path>) -> Result<Self> {
        Self::open(path, OpenOptions::default()).await
    }
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Reads the real version values persisted in this world's `level.dat`.
    pub fn level_version_blocking(&self) -> Result<LevelVersion> {
        LevelVersion::detect(&self.read_level_dat_blocking()?)
    }

    /// Scans persisted records once and returns the actual Bedrock versions/data generations present.
    ///
    /// The scan is observational only. It does not upgrade, downgrade or rewrite any record.
    pub fn versions_blocking(&self) -> Result<WorldVersions> {
        let level = self.level_version_blocking()?;
        let mut subchunk_counts = [0usize; 256];
        let mut unversioned_subchunks = 0usize;
        let mut legacy_terrain_records = 0usize;
        let mut block_extra_data_records = 0usize;
        let mut data2d_records = 0usize;
        let mut data2d_legacy_records = 0usize;
        let mut data3d_records = 0usize;
        let mut entity_records = 0usize;
        let mut digp_records = 0usize;
        let mut actorprefix_records = 0usize;

        self.storage().for_each_entry(
            StorageReadOptions::default(),
            &mut |key, value| {
                match BedrockDbKey::decode(key) {
                    BedrockDbKey::Chunk(chunk) => match chunk.tag {
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
                        ChunkRecordTag::Entity => {
                            entity_records = entity_records.saturating_add(1);
                        }
                        _ => {}
                    },
                    BedrockDbKey::ActorDigest { .. } => {
                        digp_records = digp_records.saturating_add(1);
                    }
                    BedrockDbKey::ActorPrefix { .. } => {
                        actorprefix_records = actorprefix_records.saturating_add(1);
                    }
                    _ => {}
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;

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
            subchunks,
            unversioned_subchunks,
            legacy_terrain_records,
            block_extra_data_records,
            data2d_records,
            data2d_legacy_records,
            data3d_records,
            entity_records,
            digp_records,
            actorprefix_records,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_count_keeps_actual_version_byte() {
        let count = SubChunkVersionCount {
            version: SubChunkVersion::V7,
            records: 3,
        };
        assert_eq!(count.version.byte(), 7);
        assert_eq!(count.records, 3);
    }
}
