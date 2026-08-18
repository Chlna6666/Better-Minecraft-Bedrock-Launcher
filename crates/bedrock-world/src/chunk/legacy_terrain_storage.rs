//! Lossless split of Minecraft Bedrock `LegacyTerrain` into fixed-array SubChunks plus `Data2DLegacy`.
//!
//! `LegacyTerrain` combines 16x128x16 numeric blocks, block metadata, sky/block light, a 16x16
//! height map and historical `[biome_id, red, green, blue]` samples in one value. This module only
//! changes that physical grouping. It does not upgrade numeric blocks to BlockStates or discard saved
//! biome colours.

use crate::biome::Biome2dLegacy;
use crate::chunk::{
    BedrockDbKey, ChunkKey, ChunkPos, ChunkRecordTag, LegacyBiomeSample, LegacySubChunkBuilder,
    LegacyTerrain, SubChunkVersion,
};
use crate::database::{StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use std::collections::BTreeSet;

const LEGACY_TERRAIN_SUBCHUNKS: usize = 8;

/// Summary of losslessly splitting `LegacyTerrain` records into separated Bedrock records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyTerrainSplitReport {
    /// Fixed-array SubChunk version explicitly selected by the caller.
    pub subchunk_version: SubChunkVersion,
    /// Number of `LegacyTerrain` source records converted.
    pub records: usize,
    /// Number of fixed-array SubChunk records staged.
    pub subchunks_written: usize,
    /// Number of `Data2DLegacy` records staged with preserved biome RGB samples.
    pub data2d_legacy_written: usize,
    /// Number of source `LegacyTerrain` records staged for deletion after successful conversion.
    pub legacy_terrain_removed: usize,
    /// Total bytes of new SubChunk and `Data2DLegacy` values staged before commit.
    pub staged_bytes: usize,
}

impl LegacyTerrainSplitReport {
    fn new(subchunk_version: SubChunkVersion) -> Self {
        Self {
            subchunk_version,
            records: 0,
            subchunks_written: 0,
            data2d_legacy_written: 0,
            legacy_terrain_removed: 0,
            staged_bytes: 0,
        }
    }
}

/// Preflights every `LegacyTerrain` record and stages an exact fixed-array/Data2DLegacy replacement.
///
/// The target SubChunk must be one of the real fixed-array numeric versions V0 or V2 through V7.
/// All eight 16-block vertical sections preserve numeric ID, four-bit metadata, sky light and block
/// light. The source height bytes are promoted exactly to `i16`, and all saved biome RGB components
/// are retained in `Data2DLegacy`. Existing destination SubChunk or biome records are treated as
/// conflicts rather than overwritten.
pub(crate) fn stage_legacy_terrain_split(
    storage: &dyn WorldStorage,
    subchunk_version: SubChunkVersion,
) -> Result<(StorageBatch, LegacyTerrainSplitReport)> {
    let version_byte = match subchunk_version {
        SubChunkVersion::V0 => 0,
        SubChunkVersion::V2 => 2,
        SubChunkVersion::V3 => 3,
        SubChunkVersion::V4 => 4,
        SubChunkVersion::V5 => 5,
        SubChunkVersion::V6 => 6,
        SubChunkVersion::V7 => 7,
        other => {
            return Err(BedrockWorldError::Validation(format!(
                "LegacyTerrain lossless split requires fixed-array SubChunk V0 or V2-V7, got {other:?}"
            )));
        }
    };

    // Keep only affected chunk positions. A second key-only pass checks destinations just for these
    // chunks, avoiding a large set containing every SubChunk key in a modern world.
    let mut legacy_positions = BTreeSet::<ChunkPos>::new();
    storage.for_each_key(StorageReadOptions::default(), &mut |raw_key| {
        if let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key)
            && key.tag == ChunkRecordTag::LegacyTerrain
        {
            legacy_positions.insert(key.pos);
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    if legacy_positions.is_empty() {
        return Ok((
            StorageBatch::new(),
            LegacyTerrainSplitReport::new(subchunk_version),
        ));
    }

    storage.for_each_key(StorageReadOptions::default(), &mut |raw_key| {
        let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key) else {
            return Ok(StorageVisitorControl::Continue);
        };
        if !legacy_positions.contains(&key.pos) {
            return Ok(StorageVisitorControl::Continue);
        }
        match key.tag {
            ChunkRecordTag::SubChunkPrefix => {
                let y = key.subchunk_y.ok_or_else(|| {
                    BedrockWorldError::CorruptWorld(format!(
                        "SubChunkPrefix key {key:?} has no SubChunk Y byte"
                    ))
                })?;
                if (0..LEGACY_TERRAIN_SUBCHUNKS as i8).contains(&y) {
                    return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                        "cannot split LegacyTerrain at chunk {:?}: destination SubChunk Y {y} already exists",
                        key.pos
                    )));
                }
            }
            tag @ (ChunkRecordTag::Data2DLegacy
            | ChunkRecordTag::Data2D
            | ChunkRecordTag::Data3D) => {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "cannot split LegacyTerrain at chunk {:?}: destination biome record {tag:?} already exists",
                    key.pos
                )));
            }
            _ => {}
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    let mut batch = StorageBatch::new();
    let mut report = LegacyTerrainSplitReport::new(subchunk_version);

    for pos in legacy_positions {
        let source_key = ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode();
        let source = storage.get(&source_key)?.ok_or_else(|| {
            BedrockWorldError::ConcurrentWrite(format!(
                "LegacyTerrain at chunk {pos:?} disappeared after the preflight key scan"
            ))
        })?;
        let terrain = LegacyTerrain::parse(source)?;

        for section in 0_u8..LEGACY_TERRAIN_SUBCHUNKS as u8 {
            let mut builder = LegacySubChunkBuilder::zeroed(version_byte, true)?;
            let base_y = section * 16;
            for local_x in 0_u8..16 {
                for local_z in 0_u8..16 {
                    for local_y in 0_u8..16 {
                        let source_y = base_y + local_y;
                        let block_id = terrain.block_id_at(local_x, source_y, local_z).ok_or_else(|| {
                            BedrockWorldError::CorruptWorld(format!(
                                "LegacyTerrain block coordinate unexpectedly missing at ({local_x},{source_y},{local_z})"
                            ))
                        })?;
                        let block_data = terrain.block_data_at(local_x, source_y, local_z).ok_or_else(|| {
                            BedrockWorldError::CorruptWorld(format!(
                                "LegacyTerrain block data unexpectedly missing at ({local_x},{source_y},{local_z})"
                            ))
                        })?;
                        let sky_light = terrain.sky_light_at(local_x, source_y, local_z).ok_or_else(|| {
                            BedrockWorldError::CorruptWorld(format!(
                                "LegacyTerrain sky light unexpectedly missing at ({local_x},{source_y},{local_z})"
                            ))
                        })?;
                        let block_light = terrain.block_light_at(local_x, source_y, local_z).ok_or_else(|| {
                            BedrockWorldError::CorruptWorld(format!(
                                "LegacyTerrain block light unexpectedly missing at ({local_x},{source_y},{local_z})"
                            ))
                        })?;
                        builder.set_block(local_x, local_y, local_z, block_id, block_data)?;
                        builder.set_sky_light(local_x, local_y, local_z, sky_light)?;
                        builder.set_block_light(local_x, local_y, local_z, block_light)?;
                    }
                }
            }
            let value = builder.build()?.into_raw();
            let y = i8::try_from(section).map_err(|_| {
                BedrockWorldError::Validation("LegacyTerrain section index exceeds i8".to_string())
            })?;
            report.subchunks_written = report.subchunks_written.saturating_add(1);
            report.staged_bytes = report.staged_bytes.saturating_add(value.len());
            batch.put(ChunkKey::subchunk(pos, y).encode(), value);
        }

        let heights = terrain
            .heightmap()
            .iter()
            .copied()
            .map(i16::from)
            .collect::<Vec<_>>();
        let biomes = terrain
            .biomes()
            .chunks_exact(4)
            .map(|sample| LegacyBiomeSample {
                biome_id: sample[0],
                red: sample[1],
                green: sample[2],
                blue: sample[3],
            })
            .collect::<Vec<_>>();
        let data2d_legacy = Biome2dLegacy::new(heights, biomes)?.encode()?;
        report.data2d_legacy_written = report.data2d_legacy_written.saturating_add(1);
        report.staged_bytes = report.staged_bytes.saturating_add(data2d_legacy.len());
        batch.put(
            ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy).encode(),
            data2d_legacy,
        );
        batch.delete(source_key);
        report.legacy_terrain_removed = report.legacy_terrain_removed.saturating_add(1);
        report.records = report.records.saturating_add(1);
    }

    Ok((batch, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{
        Dimension, LegacyTerrainBuilder, SubChunk, SubChunkDecodeMode, SubChunkFormat,
    };
    use crate::database::MemoryStorage;

    #[test]
    fn legacy_terrain_split_preserves_blocks_light_height_and_biome_rgb() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: -2,
            z: 5,
            dimension: Dimension::Overworld,
        };
        let mut builder = LegacyTerrainBuilder::zeroed();
        builder.set_block(3, 37, 9, 42, 11).unwrap();
        builder.set_sky_light(3, 37, 9, 13).unwrap();
        builder.set_block_light(3, 37, 9, 7).unwrap();
        builder.set_height(3, 9, 101).unwrap();
        builder
            .set_biome_sample(
                3,
                9,
                LegacyBiomeSample {
                    biome_id: 8,
                    red: 0x11,
                    green: 0x22,
                    blue: 0x33,
                },
            )
            .unwrap();
        let source_key = ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode();
        storage
            .put(&source_key, builder.build().unwrap().raw())
            .unwrap();

        let (batch, report) = stage_legacy_terrain_split(&storage, SubChunkVersion::V7).unwrap();
        assert_eq!(report.records, 1);
        assert_eq!(report.subchunks_written, 8);
        assert_eq!(report.data2d_legacy_written, 1);
        assert!(storage.get(&source_key).unwrap().is_some());

        storage.write_batch(&batch).unwrap();
        assert!(storage.get(&source_key).unwrap().is_none());

        let section_y = 37_i8 / 16;
        let value = storage
            .get(&ChunkKey::subchunk(pos, section_y).encode())
            .unwrap()
            .unwrap();
        let subchunk = SubChunk::read(section_y, value, SubChunkDecodeMode::FullIndices).unwrap();
        let SubChunkFormat::LegacySubChunk(legacy) = subchunk.format else {
            panic!("split target must remain legacy numeric");
        };
        assert_eq!(legacy.block_id_at(3, 37 % 16, 9), Some(42));
        assert_eq!(legacy.block_data_at(3, 37 % 16, 9), Some(11));
        assert_eq!(legacy.sky_light_at(3, 37 % 16, 9), Some(13));
        assert_eq!(legacy.block_light_at(3, 37 % 16, 9), Some(7));

        let biome_bytes = storage
            .get(&ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy).encode())
            .unwrap()
            .unwrap();
        let biome = Biome2dLegacy::parse(&biome_bytes).unwrap();
        let column = 9_usize * 16 + 3;
        assert_eq!(biome.height_map[column], 101);
        assert_eq!(biome.biomes[column].biome_id, 8);
        assert_eq!(biome.biomes[column].red, 0x11);
        assert_eq!(biome.biomes[column].green, 0x22);
        assert_eq!(biome.biomes[column].blue, 0x33);
    }

    #[test]
    fn destination_conflict_aborts_before_storage_write() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 1,
            z: 2,
            dimension: Dimension::Overworld,
        };
        let source_key = ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode();
        storage
            .put(&source_key, LegacyTerrainBuilder::zeroed().build().unwrap().raw())
            .unwrap();
        storage
            .put(&ChunkKey::subchunk(pos, 0).encode(), &[7, 0])
            .unwrap();

        assert!(stage_legacy_terrain_split(&storage, SubChunkVersion::V7).is_err());
        assert!(storage.get(&source_key).unwrap().is_some());
    }
}
