//! Biome sampling and color blending for Bedrock scene materials.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct BiomeSample {
    pub(super) biome_id: Option<u32>,
    pub(super) legacy_color: Option<RgbaColor>,
    /// Biome grass tint averaged over the block and its eight neighbors (Bedrock blend rule).
    pub(super) grass_tint: Option<RgbaColor>,
    /// Biome foliage tint averaged over the block and its eight neighbors.
    pub(super) foliage_tint: Option<RgbaColor>,
    /// Biome dry-foliage tint (leaf litter, dry grass) averaged the same way.
    pub(super) dry_foliage_tint: Option<RgbaColor>,
}

impl BiomeSample {
    pub(super) const fn empty() -> Self {
        Self {
            biome_id: None,
            legacy_color: None,
            grass_tint: None,
            foliage_tint: None,
            dry_foliage_tint: None,
        }
    }
}
pub(super) fn biome_at_or_top(
    chunk: &ChunkData,
    local_x: u8,
    local_z: u8,
    y: i32,
) -> Option<BiomeSample> {
    if let Some(biome) = chunk
        .column_sample_at(local_x, local_z)
        .and_then(|sample| sample.biome)
    {
        return Some(terrain_biome_sample(biome));
    }
    if let Some(samples) = chunk.legacy_biomes.as_ref() {
        if let Some(sample) = samples[usize::from(local_z)][usize::from(local_x)] {
            return Some(BiomeSample {
                biome_id: Some(u32::from(sample.biome_id)),
                legacy_color: Some(RgbaColor::new(sample.red, sample.green, sample.blue, 255)),
                ..BiomeSample::empty()
            });
        }
    }
    biome_id_at_or_top(chunk, local_x, local_z, y).map(|biome_id| BiomeSample {
        biome_id: Some(biome_id),
        ..BiomeSample::empty()
    })
}

pub(super) fn terrain_biome_sample(biome: TerrainColumnBiome) -> BiomeSample {
    match biome {
        TerrainColumnBiome::Id(biome_id) => BiomeSample {
            biome_id: Some(biome_id),
            ..BiomeSample::empty()
        },
        TerrainColumnBiome::Legacy(sample) => BiomeSample {
            biome_id: Some(u32::from(sample.biome_id)),
            legacy_color: Some(RgbaColor::new(sample.red, sample.green, sample.blue, 255)),
            ..BiomeSample::empty()
        },
    }
}

/// Per-column grass/foliage tints averaged over the block and its eight neighbors.
///
/// Bedrock computes a block's biome color from the block and its eight neighbors, which fades
/// biome borders instead of stepping at biome-cell edges.
#[derive(Default)]
pub struct BiomeBlendField {
    columns: FxHashMap<(i32, i32), BiomeTints>,
}

/// The three biome tint channels Bedrock averages over a block and its eight neighbors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BiomeTints {
    pub grass: RgbaColor,
    pub foliage: RgbaColor,
    pub dry_foliage: RgbaColor,
}

impl BiomeBlendField {
    pub fn from_chunks(chunks: &[ChunkData]) -> Self {
        let palette = RenderPalette::builtin_shared();
        let mut samples = FxHashMap::with_capacity_and_hasher(
            chunks.len().saturating_mul(256),
            Default::default(),
        );
        for chunk in chunks {
            for (index, sample) in biome_columns(chunk).into_iter().enumerate() {
                let Some(sample) = sample else {
                    continue;
                };
                samples.insert(
                    (
                        chunk.pos.x.saturating_mul(16) + (index % 16) as i32,
                        chunk.pos.z.saturating_mul(16) + (index / 16) as i32,
                    ),
                    sample,
                );
            }
        }
        let mut tint_cache = FxHashMap::<(Option<u32>, Option<[u8; 4]>), BiomeTints>::default();
        let mut tint_of = |sample: &BiomeSample| {
            let key = (
                sample.biome_id,
                sample.legacy_color.map(RgbaColor::to_array),
            );
            if let Some(tint) = tint_cache.get(&key) {
                return *tint;
            }
            // Legacy saves only carry one biome colour; use it for every channel. Otherwise the
            // palette resolves each channel and falls back to its built-in default when the
            // biome has no entry, so a block outside any biome still gets a sensible colour.
            let tint = if let Some(legacy) = sample.legacy_color
                && !color_is_washed_out(legacy)
            {
                BiomeTints {
                    grass: legacy,
                    foliage: legacy,
                    dry_foliage: legacy,
                }
            } else {
                BiomeTints {
                    grass: palette.biome_grass_color(sample.biome_id),
                    foliage: palette.biome_foliage_color(sample.biome_id),
                    dry_foliage: palette
                        .surface_block_tint("minecraft:leaf_litter", sample.biome_id)
                        .unwrap_or_else(|| palette.biome_foliage_color(sample.biome_id)),
                }
            };
            tint_cache.insert(key, tint);
            tint
        };
        let tints = samples
            .iter()
            .map(|(&pos, sample)| (pos, tint_of(sample)))
            .collect::<FxHashMap<_, _>>();
        let mut columns = FxHashMap::with_capacity_and_hasher(tints.len(), Default::default());
        for (&(x, z), &own) in &tints {
            let mut sums = [[0_u32; 3]; 3];
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let tint = tints.get(&(x + dx, z + dz)).copied().unwrap_or(own);
                    for (channel, values) in [
                        tint.grass.to_array(),
                        tint.foliage.to_array(),
                        tint.dry_foliage.to_array(),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        for index in 0..3 {
                            sums[channel][index] += u32::from(values[index]);
                        }
                    }
                }
            }
            let average = |sums: [u32; 3]| {
                RgbaColor::new(
                    u8::try_from((sums[0] + 4) / 9).unwrap_or(u8::MAX),
                    u8::try_from((sums[1] + 4) / 9).unwrap_or(u8::MAX),
                    u8::try_from((sums[2] + 4) / 9).unwrap_or(u8::MAX),
                    255,
                )
            };
            columns.insert(
                (x, z),
                BiomeTints {
                    grass: average(sums[0]),
                    foliage: average(sums[1]),
                    dry_foliage: average(sums[2]),
                },
            );
        }
        Self { columns }
    }

    pub fn tints(&self, x: i32, z: i32) -> Option<BiomeTints> {
        self.columns.get(&(x, z)).copied()
    }
}

/// Returns the topmost biome sample of each 16x16 column in Bedrock's `z * 16 + x` order.
pub(super) fn biome_columns(chunk: &ChunkData) -> Vec<Option<BiomeSample>> {
    let mut columns = vec![None; 256];
    if let Some(samples) = chunk.column_samples.as_ref() {
        for local_z in 0..16u8 {
            for local_x in 0..16u8 {
                if let Some(biome) = samples
                    .get(local_x, local_z)
                    .and_then(|sample| sample.biome)
                {
                    columns[usize::from(local_z) * 16 + usize::from(local_x)] =
                        Some(terrain_biome_sample(biome));
                }
            }
        }
    }
    if let Some(samples) = chunk.legacy_biomes.as_ref() {
        for local_z in 0..16u8 {
            for local_x in 0..16u8 {
                let index = usize::from(local_z) * 16 + usize::from(local_x);
                if columns[index].is_some() {
                    continue;
                }
                if let Some(sample) = samples[usize::from(local_z)][usize::from(local_x)] {
                    columns[index] = Some(BiomeSample {
                        biome_id: Some(u32::from(sample.biome_id)),
                        legacy_color: Some(RgbaColor::new(
                            sample.red,
                            sample.green,
                            sample.blue,
                            255,
                        )),
                        ..BiomeSample::empty()
                    });
                }
            }
        }
    }
    // Walk storages from the top down and fill only columns that no higher layer owns. Empty
    // overlay storages are skipped by their palette, so the 16x16 scan stays cheap.
    for storage in chunk.biome_data.values().rev() {
        if columns.iter().all(Option::is_some) {
            break;
        }
        if !storage
            .palette
            .iter()
            .any(|id| non_empty_biome_id(Some(*id)).is_some())
        {
            continue;
        }
        for local_z in 0..16u8 {
            for local_x in 0..16u8 {
                let index = usize::from(local_z) * 16 + usize::from(local_x);
                if columns[index].is_some() {
                    continue;
                }
                let biome_id = (0..16_u8).rev().find_map(|local_y| {
                    non_empty_biome_id(storage.biome_id_at(local_x, local_y, local_z))
                });
                if let Some(biome_id) = biome_id {
                    columns[index] = Some(BiomeSample {
                        biome_id: Some(biome_id),
                        ..BiomeSample::empty()
                    });
                }
            }
        }
    }
    columns
}

/// Returns a block's biome sample with the 3x3-averaged tints attached when a field is present.
pub(super) fn block_biome_sample(
    chunk: &ChunkData,
    local_x: u8,
    local_z: u8,
    y: i32,
    blend: Option<&BiomeBlendField>,
) -> Option<BiomeSample> {
    let sample = biome_at_or_top(chunk, local_x, local_z, y);
    let Some(blend) = blend else {
        return sample;
    };
    let x = chunk.pos.x.saturating_mul(16) + i32::from(local_x);
    let z = chunk.pos.z.saturating_mul(16) + i32::from(local_z);
    let Some(tints) = blend.tints(x, z) else {
        return sample;
    };
    let mut sample = sample.unwrap_or(BiomeSample::empty());
    sample.grass_tint = Some(tints.grass);
    sample.foliage_tint = Some(tints.foliage);
    sample.dry_foliage_tint = Some(tints.dry_foliage);
    Some(sample)
}

pub(super) fn biome_id_at_or_top(
    chunk: &ChunkData,
    local_x: u8,
    local_z: u8,
    y: i32,
) -> Option<u32> {
    biome_id_at(chunk, local_x, local_z, y).or_else(|| {
        chunk.biome_data.values().rev().find_map(|storage| {
            if storage.y.is_none() {
                return non_empty_biome_id(storage.biome_id_at(local_x, 0, local_z));
            }
            (0..16_u8).rev().find_map(|local_y| {
                non_empty_biome_id(storage.biome_id_at(local_x, local_y, local_z))
            })
        })
    })
}

pub(super) fn biome_id_at(chunk: &ChunkData, local_x: u8, local_z: u8, y: i32) -> Option<u32> {
    chunk
        .biome_data
        .get(&biome_storage_bucket_y(y))
        .or_else(|| chunk.biome_data.values().next())
        .and_then(|storage| non_empty_biome_id(biome_id_from_storage(storage, local_x, local_z, y)))
}

pub(super) fn biome_id_from_storage(
    storage: &BiomeStorage,
    local_x: u8,
    local_z: u8,
    y: i32,
) -> Option<u32> {
    let local_y = if let Some(start_y) = storage.y {
        u8::try_from(y - start_y).ok()?
    } else {
        0
    };
    storage.biome_id_at(local_x, local_y, local_z)
}

pub(super) fn biome_storage_bucket_y(y: i32) -> i32 {
    y.div_euclid(16) * 16
}

pub(super) fn non_empty_biome_id(id: Option<u32>) -> Option<u32> {
    id.filter(|id| *id != 0 && *id != u32::MAX)
}
