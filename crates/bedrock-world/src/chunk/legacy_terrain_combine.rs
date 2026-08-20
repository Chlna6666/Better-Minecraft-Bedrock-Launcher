//! Exact recombination of fixed-array Bedrock SubChunks plus `Data2DLegacy` into `LegacyTerrain`.

use crate::biome::Biome2dLegacy;
use crate::chunk::{
    BedrockDbKey, ChunkKey, ChunkPos, ChunkRecordTag, LegacySubChunk, LegacyTerrainBuilder,
    SubChunk, SubChunkDecodeMode, SubChunkFormat,
};
use crate::database::{StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use std::collections::{BTreeMap, BTreeSet};

const LEGACY_TERRAIN_SUBCHUNKS: usize = 8;
const COMPLETE_SECTION_MASK: u8 = 0xff;

/// Summary of exactly recombining separated legacy chunk data into `LegacyTerrain`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LegacyTerrainCombineReport {
    /// Number of `LegacyTerrain` records staged.
    pub records: usize,
    /// Number of fixed-array source SubChunk records consumed.
    pub subchunks_removed: usize,
    /// Number of source `Data2DLegacy` records consumed.
    pub data2d_legacy_removed: usize,
    /// Total `LegacyTerrain` value bytes staged before commit.
    pub staged_bytes: usize,
}

/// Preflights and stages the exact reverse of the lossless `LegacyTerrain` split.
///
/// A candidate chunk must contain one `Data2DLegacy` record and all eight fixed-array numeric
/// SubChunks Y=0..7. Every SubChunk must carry both historical light arrays. Any SubChunk outside the
/// 0..127 `LegacyTerrain` height range, any paletted source, an existing `LegacyTerrain`, another biome
/// generation, or an `i16` height outside `0..=255` makes the operation fail before any storage write.
pub(crate) fn stage_legacy_terrain_combine(
    storage: &dyn WorldStorage,
) -> Result<(StorageBatch, LegacyTerrainCombineReport)> {
    let mut candidates = BTreeSet::<ChunkPos>::new();
    storage.for_each_key(StorageReadOptions::default(), &mut |raw_key| {
        if let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key)
            && key.tag == ChunkRecordTag::Data2DLegacy
        {
            candidates.insert(key.pos);
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    if candidates.is_empty() {
        return Ok((StorageBatch::new(), LegacyTerrainCombineReport::default()));
    }

    let mut masks = BTreeMap::<ChunkPos, u8>::new();
    storage.for_each_key(StorageReadOptions::default(), &mut |raw_key| {
        let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(raw_key) else {
            return Ok(StorageVisitorControl::Continue);
        };
        if !candidates.contains(&key.pos) {
            return Ok(StorageVisitorControl::Continue);
        }
        match key.tag {
            ChunkRecordTag::LegacyTerrain => {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "cannot combine chunk {:?}: LegacyTerrain already exists",
                    key.pos
                )));
            }
            ChunkRecordTag::Data2D | ChunkRecordTag::Data3D => {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "cannot combine chunk {:?}: competing biome record {:?} exists",
                    key.pos, key.tag
                )));
            }
            ChunkRecordTag::SubChunkPrefix => {
                let y = key.subchunk_y.ok_or_else(|| {
                    BedrockWorldError::CorruptWorld(format!(
                        "SubChunkPrefix key {key:?} has no SubChunk Y byte"
                    ))
                })?;
                if !(0..LEGACY_TERRAIN_SUBCHUNKS as i8).contains(&y) {
                    return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                        "cannot combine chunk {:?}: SubChunk Y {y} lies outside LegacyTerrain 0..127 height",
                        key.pos
                    )));
                }
                let bit = 1_u8 << u32::try_from(y).map_err(|_| {
                    BedrockWorldError::CorruptWorld(format!(
                        "negative legacy SubChunk index {y} reached combine bitset"
                    ))
                })?;
                *masks.entry(key.pos).or_insert(0) |= bit;
            }
            _ => {}
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    for pos in &candidates {
        let mask = masks.get(pos).copied().unwrap_or(0);
        if mask != COMPLETE_SECTION_MASK {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "cannot combine chunk {pos:?}: fixed-array SubChunk section mask is 0x{mask:02x}, expected 0xff for Y=0..7"
            )));
        }
    }

    let mut batch = StorageBatch::new();
    let mut report = LegacyTerrainCombineReport::default();
    for pos in candidates {
        let biome_key = ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy).encode();
        let biome_raw = storage.get(&biome_key)?.ok_or_else(|| {
            BedrockWorldError::ConcurrentWrite(format!(
                "Data2DLegacy at chunk {pos:?} disappeared after preflight"
            ))
        })?;
        let biome = Biome2dLegacy::parse(&biome_raw)?;
        let mut terrain = LegacyTerrainBuilder::zeroed();

        for local_z in 0_u8..16 {
            for local_x in 0_u8..16 {
                let column = usize::from(local_z) * 16 + usize::from(local_x);
                let height = u8::try_from(biome.height_map[column]).map_err(|_| {
                    BedrockWorldError::UnsupportedChunkFormat(format!(
                        "Data2DLegacy height {} at chunk {pos:?} column ({local_x},{local_z}) cannot fit LegacyTerrain u8 height",
                        biome.height_map[column]
                    ))
                })?;
                terrain.set_height(local_x, local_z, height)?;
                terrain.set_biome_sample(local_x, local_z, biome.biomes[column])?;
            }
        }

        for section in 0_i8..LEGACY_TERRAIN_SUBCHUNKS as i8 {
            let key = ChunkKey::subchunk(pos, section).encode();
            let raw = storage.get(&key)?.ok_or_else(|| {
                BedrockWorldError::ConcurrentWrite(format!(
                    "SubChunk Y {section} at chunk {pos:?} disappeared after preflight"
                ))
            })?;
            let parsed = SubChunk::read(section, raw, SubChunkDecodeMode::FullIndices)?;
            let SubChunkFormat::LegacySubChunk(legacy) = parsed.format else {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "cannot combine chunk {pos:?} SubChunk Y {section}: source is not fixed-array V0/V2-V7"
                )));
            };
            copy_legacy_subchunk_into_terrain(&legacy, section, &mut terrain, pos)?;
            batch.delete(key);
            report.subchunks_removed = report.subchunks_removed.saturating_add(1);
        }

        let target = terrain.build()?.into_raw();
        report.records = report.records.saturating_add(1);
        report.data2d_legacy_removed = report.data2d_legacy_removed.saturating_add(1);
        report.staged_bytes = report.staged_bytes.saturating_add(target.len());
        batch.put(
            ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
            target,
        );
        batch.delete(biome_key);
    }

    Ok((batch, report))
}

fn copy_legacy_subchunk_into_terrain(
    source: &LegacySubChunk,
    section: i8,
    target: &mut LegacyTerrainBuilder,
    pos: ChunkPos,
) -> Result<()> {
    if !source.has_light_arrays() {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "cannot combine chunk {pos:?} SubChunk Y {section}: source omits sky/block light arrays required by LegacyTerrain"
        )));
    }
    let section_u8 = u8::try_from(section).map_err(|_| {
        BedrockWorldError::CorruptWorld(format!(
            "negative section {section} reached LegacyTerrain combine"
        ))
    })?;
    let base_y = section_u8 * 16;
    for local_x in 0_u8..16 {
        for local_z in 0_u8..16 {
            for local_y in 0_u8..16 {
                let target_y = base_y + local_y;
                let block_id = source
                    .block_id_at(local_x, local_y, local_z)
                    .ok_or_else(|| {
                        BedrockWorldError::CorruptWorld(
                            "legacy SubChunk block id missing".to_string(),
                        )
                    })?;
                let block_data =
                    source
                        .block_data_at(local_x, local_y, local_z)
                        .ok_or_else(|| {
                            BedrockWorldError::CorruptWorld(
                                "legacy SubChunk block data missing".to_string(),
                            )
                        })?;
                let sky_light =
                    source
                        .sky_light_at(local_x, local_y, local_z)
                        .ok_or_else(|| {
                            BedrockWorldError::CorruptWorld(
                                "legacy SubChunk sky light missing".to_string(),
                            )
                        })?;
                let block_light = source
                    .block_light_at(local_x, local_y, local_z)
                    .ok_or_else(|| {
                        BedrockWorldError::CorruptWorld(
                            "legacy SubChunk block light missing".to_string(),
                        )
                    })?;
                target.set_block(local_x, target_y, local_z, block_id, block_data)?;
                target.set_sky_light(local_x, target_y, local_z, sky_light)?;
                target.set_block_light(local_x, target_y, local_z, block_light)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biome::{Biome2dLegacy, LegacyBiomeSample};
    use crate::chunk::{Dimension, LegacySubChunkBuilder, LegacyTerrain};
    use crate::database::MemoryStorage;

    fn seed_exact_source(storage: &MemoryStorage, pos: ChunkPos) {
        for section in 0_i8..8 {
            let mut builder = LegacySubChunkBuilder::zeroed(7, true).unwrap();
            if section == 2 {
                builder.set_block(3, 5, 9, 42, 11).unwrap();
                builder.set_sky_light(3, 5, 9, 13).unwrap();
                builder.set_block_light(3, 5, 9, 7).unwrap();
            }
            storage
                .put(
                    &ChunkKey::subchunk(pos, section).encode(),
                    builder.build().unwrap().raw(),
                )
                .unwrap();
        }
        let mut biomes = vec![
            LegacyBiomeSample {
                biome_id: 1,
                red: 2,
                green: 3,
                blue: 4,
            };
            256
        ];
        let column = 9 * 16 + 3;
        biomes[column] = LegacyBiomeSample {
            biome_id: 8,
            red: 0x11,
            green: 0x22,
            blue: 0x33,
        };
        let biome = Biome2dLegacy::new(vec![101; 256], biomes).unwrap();
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy).encode(),
                &biome.encode().unwrap(),
            )
            .unwrap();
    }

    #[test]
    fn separated_legacy_records_recombine_exactly_before_commit() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 4,
            z: -3,
            dimension: Dimension::Overworld,
        };
        seed_exact_source(&storage, pos);
        let terrain_key = ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode();

        let (batch, report) = stage_legacy_terrain_combine(&storage).unwrap();
        assert_eq!(report.records, 1);
        assert_eq!(report.subchunks_removed, 8);
        assert!(storage.get(&terrain_key).unwrap().is_none());
        storage.write_batch(&batch).unwrap();

        let terrain = LegacyTerrain::parse(storage.get(&terrain_key).unwrap().unwrap()).unwrap();
        assert_eq!(terrain.block_id_at(3, 37, 9), Some(42));
        assert_eq!(terrain.block_data_at(3, 37, 9), Some(11));
        assert_eq!(terrain.sky_light_at(3, 37, 9), Some(13));
        assert_eq!(terrain.block_light_at(3, 37, 9), Some(7));
        assert_eq!(terrain.height_at(3, 9), Some(101));
        let biome = terrain.biome_sample_at(3, 9).unwrap();
        assert_eq!(biome.biome_id, 8);
        assert_eq!((biome.red, biome.green, biome.blue), (0x11, 0x22, 0x33));
    }

    #[test]
    fn out_of_range_height_refuses_recombine() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        for section in 0_i8..8 {
            storage
                .put(
                    &ChunkKey::subchunk(pos, section).encode(),
                    LegacySubChunkBuilder::zeroed(7, true)
                        .unwrap()
                        .build()
                        .unwrap()
                        .raw(),
                )
                .unwrap();
        }
        let biome = Biome2dLegacy::new(
            vec![300; 256],
            vec![
                LegacyBiomeSample {
                    biome_id: 1,
                    red: 2,
                    green: 3,
                    blue: 4,
                };
                256
            ],
        )
        .unwrap();
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy).encode(),
                &biome.encode().unwrap(),
            )
            .unwrap();

        assert!(stage_legacy_terrain_combine(&storage).is_err());
        assert!(
            storage
                .get(&ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode())
                .unwrap()
                .is_none()
        );
    }
}
