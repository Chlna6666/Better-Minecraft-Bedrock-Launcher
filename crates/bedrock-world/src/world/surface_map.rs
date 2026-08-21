//! Compact business-level 2D surface-map queries.
//!
//! This API intentionally does not expose `SubChunk`, full 3D block indices, block entities or
//! general `ChunkData`. The public contract is always exact: execution hints are an implementation
//! detail and may never change the returned surface. The output is a fixed 16x16 column plane plus
//! a per-chunk deduplicated material table.
//!
//! The current loader keeps the existing exact `ChunkData` projection internally as a compatibility
//! bridge. The public layout is intentionally independent so the next optimization stage can write
//! palette/material ids directly during surface projection without changing consumers.

use super::{
    BedrockWorld, BiomeDataRequirement, ChunkDataRequest, ChunkLoadOptions, ChunkLoadPriority,
    ChunkLoadStats, ExactSurfaceSubchunkPolicy, TerrainColumnBiome, TerrainSampleSource,
    WorldPipelineOptions, WorldStorageHandle, WorldThreadingOptions,
};
use crate::chunk::{BlockState, ChunkPos, SubChunkDecodeMode};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use crate::storage::StorageCachePolicy;
use std::collections::BTreeMap;

const SURFACE_COLUMN_COUNT: usize = 16 * 16;
const NO_MATERIAL: u16 = u16::MAX;
const NO_HEIGHT: i16 = i16::MIN;

/// Compact material referenced by one or more exact 2D map columns.
///
/// `version` from the general `BlockState` representation is intentionally omitted because it does
/// not affect 2D palette selection. State properties are retained because render palettes may define
/// state-specific color overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceMapMaterial {
    /// Canonical Bedrock block identifier.
    pub name: String,
    /// State properties needed for exact state-sensitive palette selection.
    pub states: BTreeMap<String, NbtTag>,
}

impl SurfaceMapMaterial {
    fn from_state(state: &BlockState) -> Self {
        Self {
            name: state.name.clone(),
            states: state.states.clone(),
        }
    }

    fn matches_state(&self, state: &BlockState) -> bool {
        self.name == state.name && self.states == state.states
    }
}

/// One compact exact 2D terrain column in local `z * 16 + x` order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceMapColumn {
    /// Y coordinate of the visible surface block.
    pub surface_y: i16,
    /// Material id of the visible surface block.
    pub surface_material: u16,
    /// Y coordinate of the relief/support block.
    pub relief_y: i16,
    /// Material id of the relief/support block.
    pub relief_material: u16,
    /// Internal thin-overlay Y coordinate; use [`Self::overlay_y`] for the optional view.
    overlay_y: i16,
    /// Material id of a thin overlay, or an internal sentinel when absent.
    overlay_material: u16,
    /// Water depth above the underwater support block.
    pub water_depth: u8,
    /// Material id of visible water, or an internal sentinel when absent.
    water_material: u16,
    /// Internal underwater-support Y coordinate; use [`Self::underwater_y`] for the optional view.
    underwater_y: i16,
    /// Material id of the underwater support block, or an internal sentinel when absent.
    underwater_material: u16,
    /// Biome context used by the 2D palette.
    pub biome: Option<TerrainColumnBiome>,
    /// Storage family that produced the visible surface.
    pub source: TerrainSampleSource,
}

impl SurfaceMapColumn {
    /// Returns the optional overlay Y coordinate.
    #[must_use]
    pub const fn overlay_y(self) -> Option<i16> {
        if self.overlay_y == NO_HEIGHT {
            None
        } else {
            Some(self.overlay_y)
        }
    }

    /// Returns the optional overlay material id.
    #[must_use]
    pub const fn overlay_material(self) -> Option<u16> {
        if self.overlay_material == NO_MATERIAL {
            None
        } else {
            Some(self.overlay_material)
        }
    }

    /// Returns the optional visible-water material id.
    #[must_use]
    pub const fn water_material(self) -> Option<u16> {
        if self.water_material == NO_MATERIAL {
            None
        } else {
            Some(self.water_material)
        }
    }

    /// Returns the optional underwater-support Y coordinate.
    #[must_use]
    pub const fn underwater_y(self) -> Option<i16> {
        if self.underwater_y == NO_HEIGHT {
            None
        } else {
            Some(self.underwater_y)
        }
    }

    /// Returns the optional underwater-support material id.
    #[must_use]
    pub const fn underwater_material(self) -> Option<u16> {
        if self.underwater_material == NO_MATERIAL {
            None
        } else {
            Some(self.underwater_material)
        }
    }
}

/// Compact exact 16x16 surface plane for one requested chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceMapChunk {
    /// Chunk position represented by this plane.
    pub pos: ChunkPos,
    /// Deduplicated render materials referenced by the 256 columns.
    pub materials: Vec<SurfaceMapMaterial>,
    /// Columns in `z * 16 + x` order. Missing columns stay `None`.
    columns: Box<[Option<SurfaceMapColumn>; SURFACE_COLUMN_COUNT]>,
}

impl SurfaceMapChunk {
    /// Returns one local 2D map column.
    #[must_use]
    pub fn column(&self, local_x: u8, local_z: u8) -> Option<&SurfaceMapColumn> {
        if local_x >= 16 || local_z >= 16 {
            return None;
        }
        self.columns[usize::from(local_z) * 16 + usize::from(local_x)].as_ref()
    }

    /// Returns the fixed 256-column plane in `z * 16 + x` order.
    #[must_use]
    pub fn columns(&self) -> &[Option<SurfaceMapColumn>; SURFACE_COLUMN_COUNT] {
        &self.columns
    }

    /// Resolves a compact material id.
    #[must_use]
    pub fn material(&self, id: u16) -> Option<&SurfaceMapMaterial> {
        self.materials.get(usize::from(id))
    }
}

/// Controls an exact compact 2D surface-map batch query.
///
/// There is deliberately no public `HintThenVerify`/`Full` correctness switch. A call to
/// [`BedrockWorld::query_surface_map_many_blocking`] always means an exact persisted-world surface.
/// Internal implementations may use hints only when they can prove the same result and must fall
/// back to exact/full reads otherwise.
#[derive(Debug, Clone)]
pub struct SurfaceMapQueryOptions {
    /// Threading policy for independent chunks.
    pub threading: WorldThreadingOptions,
    /// Bounded chunk/decode pipeline settings.
    pub pipeline: WorldPipelineOptions,
    /// Chunk ordering policy.
    pub priority: ChunkLoadPriority,
    /// Backend cache policy for exact storage reads.
    pub storage_cache_policy: StorageCachePolicy,
}

impl Default for SurfaceMapQueryOptions {
    fn default() -> Self {
        Self {
            threading: WorldThreadingOptions::Auto,
            pipeline: WorldPipelineOptions::default(),
            priority: ChunkLoadPriority::RowMajor,
            storage_cache_policy: StorageCachePolicy::Use,
        }
    }
}

/// Diagnostics returned by an exact compact 2D surface-map batch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceMapBatchStats {
    /// Underlying exact storage/decode statistics.
    pub load: ChunkLoadStats,
    /// Number of compact chunks returned.
    pub chunks: usize,
    /// Number of populated surface columns returned.
    pub columns: usize,
    /// Number of unique per-chunk materials retained after compaction.
    pub materials: usize,
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Loads compact exact 2D map data for explicit chunk positions.
    ///
    /// The public result never exposes full `ChunkData`/`SubChunk`/`BlockState`. The current first
    /// implementation intentionally uses the exact/full surface loader internally while the direct
    /// projection path is being moved below `ChunkData`; therefore correctness is already final even
    /// before the allocation-removal stage is complete.
    pub fn query_surface_map_many_blocking(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
        options: SurfaceMapQueryOptions,
    ) -> Result<(Vec<SurfaceMapChunk>, SurfaceMapBatchStats)> {
        let mut load_options = ChunkLoadOptions::for_data_request(
            ChunkDataRequest::new()
                .surface_columns(ExactSurfaceSubchunkPolicy::Full)
                .biome(BiomeDataRequirement::SurfaceColumns),
        );
        load_options.subchunk_decode = SubChunkDecodeMode::SurfaceColumns;
        load_options.threading = options.threading;
        load_options.pipeline = options.pipeline;
        load_options.priority = options.priority;
        load_options.storage_cache_policy = options.storage_cache_policy;

        let (chunks, load) = self.query_chunk_data_with_stats_blocking(positions, load_options)?;
        let mut compact = Vec::with_capacity(chunks.len());
        let mut column_count = 0usize;
        let mut material_count = 0usize;
        for chunk in chunks {
            let mapped = compact_surface_chunk(&chunk)?;
            column_count = column_count.saturating_add(
                mapped.columns.iter().filter(|column| column.is_some()).count(),
            );
            material_count = material_count.saturating_add(mapped.materials.len());
            compact.push(mapped);
        }
        let stats = SurfaceMapBatchStats {
            load,
            chunks: compact.len(),
            columns: column_count,
            materials: material_count,
        };
        Ok((compact, stats))
    }
}

fn compact_surface_chunk(chunk: &super::ChunkData) -> Result<SurfaceMapChunk> {
    let mut materials = Vec::<SurfaceMapMaterial>::with_capacity(32);
    let mut columns = Box::new(std::array::from_fn(|_| None));
    let Some(samples) = chunk.column_samples.as_ref() else {
        return Ok(SurfaceMapChunk {
            pos: chunk.pos,
            materials,
            columns,
        });
    };

    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            let Some(sample) = samples.get(local_x, local_z) else {
                continue;
            };
            let surface_material = intern_material(&mut materials, &sample.surface_block_state)?;
            let relief_material = intern_material(&mut materials, &sample.relief_block_state)?;
            let (overlay_y, overlay_material) = sample.overlay.as_ref().map_or(
                Ok::<(i16, u16), BedrockWorldError>((NO_HEIGHT, NO_MATERIAL)),
                |overlay| {
                    Ok((
                        overlay.y,
                        intern_material(&mut materials, &overlay.block_state)?,
                    ))
                },
            )?;
            let (water_depth, water_material, underwater_y, underwater_material) =
                sample.water.as_ref().map_or(
                    Ok::<(u8, u16, i16, u16), BedrockWorldError>((
                        0,
                        NO_MATERIAL,
                        NO_HEIGHT,
                        NO_MATERIAL,
                    )),
                    |water| {
                        Ok((
                            water.depth,
                            intern_material(&mut materials, &water.block_state)?,
                            water.underwater_y.unwrap_or(NO_HEIGHT),
                            water
                                .underwater_block_state
                                .as_ref()
                                .map(|state| intern_material(&mut materials, state))
                                .transpose()?
                                .unwrap_or(NO_MATERIAL),
                        ))
                    },
                )?;
            columns[usize::from(local_z) * 16 + usize::from(local_x)] = Some(SurfaceMapColumn {
                surface_y: sample.surface_y,
                surface_material,
                relief_y: sample.relief_y,
                relief_material,
                overlay_y,
                overlay_material,
                water_depth,
                water_material,
                underwater_y,
                underwater_material,
                biome: sample.biome,
                source: sample.source,
            });
        }
    }

    Ok(SurfaceMapChunk {
        pos: chunk.pos,
        materials,
        columns,
    })
}

fn intern_material(materials: &mut Vec<SurfaceMapMaterial>, state: &BlockState) -> Result<u16> {
    if let Some(index) = materials.iter().position(|material| material.matches_state(state)) {
        return u16::try_from(index).map_err(|_| {
            BedrockWorldError::Validation("surface material table exceeds u16".to_string())
        });
    }
    let index = u16::try_from(materials.len()).map_err(|_| {
        BedrockWorldError::Validation("surface material table exceeds u16".to_string())
    })?;
    materials.push(SurfaceMapMaterial::from_state(state));
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_optional_fields_use_internal_sentinels() {
        let column = SurfaceMapColumn {
            surface_y: 64,
            surface_material: 0,
            relief_y: 63,
            relief_material: 1,
            overlay_y: NO_HEIGHT,
            overlay_material: NO_MATERIAL,
            water_depth: 0,
            water_material: NO_MATERIAL,
            underwater_y: NO_HEIGHT,
            underwater_material: NO_MATERIAL,
            biome: None,
            source: TerrainSampleSource::Subchunk,
        };
        assert_eq!(column.overlay_y(), None);
        assert_eq!(column.overlay_material(), None);
        assert_eq!(column.water_material(), None);
        assert_eq!(column.underwater_y(), None);
        assert_eq!(column.underwater_material(), None);
    }
}
