//! Minecraft Bedrock `Data2D` biome data and explicit `Data3D` projection helpers.

use crate::biome::{Biome2d, Biome3d, BiomeStorage};
use crate::error::{BedrockWorldError, Result};
use std::collections::BTreeMap;
use std::ops::RangeInclusive;

/// Expands one `Data2D` biome map into `Data3D` sections selected by the caller.
///
/// Each 2D column biome is repeated through every requested 16-block section. The height map is
/// preserved exactly and no world/game version is guessed.
pub fn data2d_to_data3d(source: &Biome2d, subchunk_y: RangeInclusive<i8>) -> Result<Biome3d> {
    if source.height_map.len() != 256 || source.biomes.len() != 256 {
        return Err(BedrockWorldError::Validation(format!(
            "Data2D requires 256 height and biome entries, got {}/{}",
            source.height_map.len(),
            source.biomes.len()
        )));
    }
    let mut storages = Vec::new();
    for section_y in subchunk_y {
        let mut palette = Vec::<u32>::new();
        let mut palette_index = BTreeMap::<u8, u16>::new();
        let mut indices = Vec::with_capacity(4096);
        let mut counts = Vec::<u16>::new();
        for local_x in 0..16_usize {
            for local_z in 0..16_usize {
                let biome = source.biomes[local_z * 16 + local_x];
                let index = if let Some(index) = palette_index.get(&biome).copied() {
                    index
                } else {
                    let index = u16::try_from(palette.len()).map_err(|_| {
                        BedrockWorldError::Validation("biome palette index overflowed".to_string())
                    })?;
                    palette.push(u32::from(biome));
                    counts.push(0);
                    palette_index.insert(biome, index);
                    index
                };
                for _local_y in 0..16 {
                    indices.push(index);
                    let count = counts.get_mut(usize::from(index)).ok_or_else(|| {
                        BedrockWorldError::Validation(
                            "biome palette count index missing".to_string(),
                        )
                    })?;
                    *count = count.saturating_add(1);
                }
            }
        }
        storages.push(BiomeStorage {
            y: Some(i32::from(section_y) * 16),
            palette,
            indices: Some(indices),
            counts,
        });
    }
    Biome3d::new(source.height_map.clone(), storages)
}

/// Collapses `Data3D` to `Data2D` only when every vertical sample in each column is identical.
///
/// This is the lossless reverse of [`data2d_to_data3d`]. A vertically varying 3D biome column is
/// rejected because `Data2D` has no field capable of representing it.
pub fn data3d_to_data2d(source: &Biome3d) -> Result<Biome2d> {
    if source.height_map.len() != 256 {
        return Err(BedrockWorldError::Validation(format!(
            "Data3D height map has {} entries instead of 256",
            source.height_map.len()
        )));
    }
    if source.storages.is_empty() {
        return Err(BedrockWorldError::Validation(
            "Data3D has no biome storages".to_string(),
        ));
    }
    let mut biomes = vec![0_u8; 256];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            let mut selected = None::<u32>;
            for storage in &source.storages {
                for local_y in 0..16_u8 {
                    let biome =
                        storage
                            .biome_id_at(local_x, local_y, local_z)
                            .ok_or_else(|| {
                                BedrockWorldError::Validation(
                            "Data3D biome storage has no full indices for reverse Data2D write"
                                .to_string(),
                        )
                            })?;
                    match selected {
                        None => selected = Some(biome),
                        Some(expected) if expected == biome => {}
                        Some(expected) => {
                            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                                "Data3D column ({local_x},{local_z}) varies vertically ({expected} != {biome}); Data2D cannot represent it"
                            )));
                        }
                    }
                }
            }
            let biome = selected.ok_or_else(|| {
                BedrockWorldError::Validation("Data3D column has no biome value".to_string())
            })?;
            biomes[usize::from(local_z) * 16 + usize::from(local_x)] = u8::try_from(biome)
                .map_err(|_| {
                    BedrockWorldError::UnsupportedChunkFormat(format!(
                        "Data3D biome id {biome} does not fit the Data2D u8 representation"
                    ))
                })?;
        }
    }
    Biome2d::new(source.height_map.clone(), biomes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data2d_data3d_roundtrip_is_lossless_for_uniform_columns() {
        let mut biomes = vec![1_u8; 256];
        biomes[3 * 16 + 2] = 7;
        let source = Biome2d::new(vec![64; 256], biomes).unwrap();
        let data3d = data2d_to_data3d(&source, -4..=-3).unwrap();
        let restored = data3d_to_data2d(&data3d).unwrap();
        assert_eq!(restored.height_map, source.height_map);
        assert_eq!(restored.biomes, source.biomes);
    }
}
