//! Upgrade helpers for historical 2D biome records.

use crate::biome::{Biome2d, Biome3d, ParsedBiomeStorage};
use crate::error::{BedrockWorldError, Result};
use std::collections::BTreeMap;
use std::ops::RangeInclusive;

/// Promotes one legacy `Data2D` biome map into explicit 3D biome storages.
///
/// Each historical column biome is repeated vertically through every requested 16-block section.
/// The source height map is preserved exactly. Callers choose the section range from the world's
/// dimension/build-height rules instead of this function guessing a Minecraft version.
pub fn promote_data2d_to_data3d(
    source: &Biome2d,
    subchunk_y: RangeInclusive<i8>,
) -> Result<Biome3d> {
    if source.height_map.len() != 256 || source.biomes.len() != 256 {
        return Err(BedrockWorldError::Validation(format!(
            "Data2D migration requires 256 height and biome entries, got {}/{}",
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
                        BedrockWorldError::Validation("biome palette count index missing".to_string())
                    })?;
                    *count = count.saturating_add(1);
                }
            }
        }
        storages.push(ParsedBiomeStorage {
            y: Some(i32::from(section_y)),
            palette,
            indices: Some(indices),
            counts,
        });
    }
    Biome3d::new(source.height_map.clone(), storages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotion_preserves_columns_and_heightmap() {
        let mut biomes = vec![1_u8; 256];
        biomes[3 * 16 + 2] = 7;
        let source = Biome2d::new(vec![64; 256], biomes).unwrap();
        let migrated = promote_data2d_to_data3d(&source, -4..=-3).unwrap();
        assert_eq!(migrated.height_map, source.height_map);
        assert_eq!(migrated.storages.len(), 2);
        for storage in &migrated.storages {
            assert_eq!(storage.indices.as_ref().unwrap().len(), 4096);
            assert_eq!(storage.biome_id_at(2, 9, 3), Some(7));
            assert_eq!(storage.biome_id_at(0, 0, 0), Some(1));
        }
    }
}
