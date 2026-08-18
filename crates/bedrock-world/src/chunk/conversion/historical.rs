//! Canonical decoding for historical numeric Bedrock terrain.
//!
//! `LegacyTerrain` and pre-paletted SubChunk versions store numeric block IDs plus four-bit metadata.
//! The decoder resolves each distinct numeric `(id, data)` reference at most once per subchunk and
//! builds a compact palette plus 4096 `u16` indices. This avoids cloning heap-owning `BlockState`
//! values for every block while still producing the same canonical random-access representation used
//! by modern paletted chunks.

use crate::biome::LegacyBiomeSample;
use crate::block::{BlockPalette, BlockState, block_storage_index};
use crate::chunk::legacy::{LegacySubChunk, LegacyTerrain};
use crate::error::{BedrockWorldError, Result};
use crate::integrity::SubChunkCodecKind;
use bytes::Bytes;

const BLOCKS_PER_SUBCHUNK: usize = 16 * 16 * 16;
const LEGACY_REFERENCE_COUNT: usize = 256 * 16;
const LEGACY_TERRAIN_SUBCHUNKS: usize = 128 / 16;
const UNRESOLVED_PALETTE_INDEX: u16 = u16::MAX;

/// Numeric block reference stored by pre-paletted Bedrock terrain formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LegacyBlockReference {
    /// Historical numeric block id.
    pub id: u8,
    /// Four-bit historical block data/metadata value.
    pub data: u8,
}

impl LegacyBlockReference {
    fn slot(self) -> Option<usize> {
        (self.data < 16).then_some(usize::from(self.id) * 16 + usize::from(self.data))
    }
}

/// Physical historical storage family that supplied a numeric block reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LegacyBlockSource {
    /// Old 16×128×16 `LegacyTerrain` record.
    LegacyTerrain,
    /// Pre-paletted 16×16×16 subchunk record.
    LegacySubChunk {
        /// Historical subchunk payload version (`0` or `2..=7`).
        version: u8,
    },
}

/// Resolves version-dependent historical numeric blocks into canonical semantic block states.
pub trait LegacyBlockResolver: Send + Sync {
    /// Resolves one numeric block reference.
    ///
    /// Returning `None` means the mapping is unknown for this resolver's source game version. The
    /// migration codec stops rather than inventing a modern block state.
    fn resolve(
        &self,
        source: LegacyBlockSource,
        block: LegacyBlockReference,
    ) -> Option<BlockState>;
}

/// Allocation-bounded exact `(id,data) -> BlockState` resolver for generated/static version tables.
///
/// The 4096 possible legacy references are indexed through an 8 KiB `u16` slot table. Semantic state
/// objects are stored only for populated mappings, avoiding one tree node/allocation per entry.
#[derive(Debug, Clone)]
pub struct LegacyBlockMapping {
    slots: Box<[u16; LEGACY_REFERENCE_COUNT]>,
    states: Vec<BlockState>,
}

impl Default for LegacyBlockMapping {
    fn default() -> Self {
        Self::new()
    }
}

impl LegacyBlockMapping {
    /// Creates an empty exact legacy block mapping.
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: Box::new([UNRESOLVED_PALETTE_INDEX; LEGACY_REFERENCE_COUNT]),
            states: Vec::new(),
        }
    }

    /// Inserts one exact numeric block/data mapping.
    pub fn insert(
        &mut self,
        block: LegacyBlockReference,
        state: BlockState,
    ) -> Result<Option<BlockState>> {
        let slot = block.slot().ok_or_else(|| {
            BedrockWorldError::Validation(format!(
                "legacy block metadata must fit four bits, got {} for id {}",
                block.data, block.id
            ))
        })?;
        let current = self.slots[slot];
        if current != UNRESOLVED_PALETTE_INDEX {
            return Ok(Some(std::mem::replace(
                &mut self.states[usize::from(current)],
                state,
            )));
        }
        let index = u16::try_from(self.states.len()).map_err(|_| {
            BedrockWorldError::Validation("legacy block mapping exceeds u16".to_string())
        })?;
        self.states.push(state);
        self.slots[slot] = index;
        Ok(None)
    }

    /// Returns the number of exact mappings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// Returns whether no mappings are configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

impl LegacyBlockResolver for LegacyBlockMapping {
    fn resolve(
        &self,
        _source: LegacyBlockSource,
        block: LegacyBlockReference,
    ) -> Option<BlockState> {
        let slot = block.slot()?;
        let index = *self.slots.get(slot)?;
        (index != UNRESOLVED_PALETTE_INDEX)
            .then(|| self.states.get(usize::from(index)).cloned())
            .flatten()
    }
}

/// One canonical 16×16×16 block layer resolved from a historical numeric subchunk.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedHistoricalSubChunk {
    /// Vertical subchunk index.
    pub y: i8,
    /// Source historical codec family.
    pub source_codec: SubChunkCodecKind,
    /// Canonical compact block palette and exact block indices.
    pub palette: BlockPalette,
    /// Original source bytes retained for diagnostics/preservation.
    pub raw: Bytes,
}

impl ResolvedHistoricalSubChunk {
    /// Returns a canonical block at local subchunk coordinates.
    #[must_use]
    pub fn block_state_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<&BlockState> {
        self.palette.block_state_at(local_x, local_y, local_z)
    }
}

/// Canonical view resolved from one old `LegacyTerrain` record.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLegacyTerrain {
    /// Eight compact 16-block-high canonical subchunks covering Y=0..127.
    pub subchunks: Vec<ResolvedHistoricalSubChunk>,
    /// Original 256-byte heightmap in `z * 16 + x` order.
    pub heightmap: Vec<u8>,
    /// Original legacy biome samples in `z * 16 + x` order.
    pub biomes: Vec<LegacyBiomeSample>,
    /// Complete raw `LegacyTerrain` value retained for preservation/diagnostics.
    pub raw: Bytes,
}

/// Resolves a pre-paletted subchunk into a compact canonical palette.
pub fn resolve_legacy_subchunk(
    y: i8,
    subchunk: &LegacySubChunk,
    resolver: &dyn LegacyBlockResolver,
) -> Result<ResolvedHistoricalSubChunk> {
    let source = LegacyBlockSource::LegacySubChunk {
        version: subchunk.version(),
    };
    let palette = resolve_numeric_palette(source, resolver, |local_x, local_y, local_z| {
        Ok(LegacyBlockReference {
            id: subchunk.block_id_at(local_x, local_y, local_z).ok_or_else(|| {
                BedrockWorldError::CorruptWorld(
                    "legacy subchunk block id index is out of bounds".to_string(),
                )
            })?,
            data: subchunk
                .block_data_at(local_x, local_y, local_z)
                .ok_or_else(|| {
                    BedrockWorldError::CorruptWorld(
                        "legacy subchunk block data index is out of bounds".to_string(),
                    )
                })?,
        })
    })?;

    Ok(ResolvedHistoricalSubChunk {
        y,
        source_codec: SubChunkCodecKind::from_version(Some(subchunk.version())),
        palette,
        raw: subchunk.raw().clone(),
    })
}

/// Resolves one 16×128×16 `LegacyTerrain` record into eight compact canonical subchunks.
pub fn resolve_legacy_terrain(
    terrain: &LegacyTerrain,
    resolver: &dyn LegacyBlockResolver,
) -> Result<ResolvedLegacyTerrain> {
    let mut subchunks = Vec::with_capacity(LEGACY_TERRAIN_SUBCHUNKS);
    for subchunk_index in 0..LEGACY_TERRAIN_SUBCHUNKS {
        let y = i8::try_from(subchunk_index).map_err(|_| {
            BedrockWorldError::Validation("legacy terrain subchunk index overflowed".to_string())
        })?;
        let palette = resolve_numeric_palette(
            LegacyBlockSource::LegacyTerrain,
            resolver,
            |local_x, local_y, local_z| {
                let world_y = u8::try_from(subchunk_index * 16 + usize::from(local_y)).map_err(
                    |_| {
                        BedrockWorldError::Validation(
                            "legacy terrain Y coordinate overflowed".to_string(),
                        )
                    },
                )?;
                Ok(LegacyBlockReference {
                    id: terrain.block_id_at(local_x, world_y, local_z).ok_or_else(|| {
                        BedrockWorldError::CorruptWorld(
                            "LegacyTerrain block id index is out of bounds".to_string(),
                        )
                    })?,
                    data: terrain
                        .block_data_at(local_x, world_y, local_z)
                        .ok_or_else(|| {
                            BedrockWorldError::CorruptWorld(
                                "LegacyTerrain block data index is out of bounds".to_string(),
                            )
                        })?,
                })
            },
        )?;
        subchunks.push(ResolvedHistoricalSubChunk {
            y,
            source_codec: SubChunkCodecKind::UnknownLegacy(0xff),
            palette,
            raw: Bytes::new(),
        });
    }

    let mut biomes = Vec::with_capacity(256);
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            biomes.push(terrain.biome_sample_at(local_x, local_z).ok_or_else(|| {
                BedrockWorldError::CorruptWorld(
                    "LegacyTerrain biome sample index is out of bounds".to_string(),
                )
            })?);
        }
    }

    Ok(ResolvedLegacyTerrain {
        subchunks,
        heightmap: terrain.heightmap().to_vec(),
        biomes,
        raw: terrain.raw().clone(),
    })
}

fn resolve_numeric_palette<F>(
    source: LegacyBlockSource,
    resolver: &dyn LegacyBlockResolver,
    mut block_at: F,
) -> Result<BlockPalette>
where
    F: FnMut(u8, u8, u8) -> Result<LegacyBlockReference>,
{
    let mut numeric_slots = [UNRESOLVED_PALETTE_INDEX; LEGACY_REFERENCE_COUNT];
    let mut states = Vec::<BlockState>::new();
    let mut counts = Vec::<u16>::new();
    let mut indices = vec![0_u16; BLOCKS_PER_SUBCHUNK];

    for local_x in 0..16_u8 {
        for local_z in 0..16_u8 {
            for local_y in 0..16_u8 {
                let block = block_at(local_x, local_y, local_z)?;
                let numeric_slot = block.slot().ok_or_else(|| {
                    BedrockWorldError::CorruptWorld(format!(
                        "historical metadata exceeds four bits: id={}, data={}",
                        block.id, block.data
                    ))
                })?;
                let palette_index = if numeric_slots[numeric_slot] == UNRESOLVED_PALETTE_INDEX {
                    let state = resolve_required(resolver, source, block)?;
                    let index = u16::try_from(states.len()).map_err(|_| {
                        BedrockWorldError::Validation(
                            "historical subchunk palette exceeds u16".to_string(),
                        )
                    })?;
                    states.push(state);
                    counts.push(0);
                    numeric_slots[numeric_slot] = index;
                    index
                } else {
                    numeric_slots[numeric_slot]
                };
                let block_index = block_storage_index(local_x, local_y, local_z);
                indices[block_index] = palette_index;
                counts[usize::from(palette_index)] =
                    counts[usize::from(palette_index)].saturating_add(1);
            }
        }
    }

    Ok(BlockPalette::with_unpacked_indices(
        states,
        indices,
        Some(counts),
    ))
}

fn resolve_required(
    resolver: &dyn LegacyBlockResolver,
    source: LegacyBlockSource,
    block: LegacyBlockReference,
) -> Result<BlockState> {
    let state = resolver.resolve(source, block).ok_or_else(|| {
        BedrockWorldError::UnsupportedChunkFormat(format!(
            "legacy numeric block mapping is unresolved: source={source:?}, id={}, data={}; provide a version-appropriate LegacyBlockResolver",
            block.id, block.data
        ))
    })?;
    if state.name.trim().is_empty() || state.version.is_none() {
        return Err(BedrockWorldError::Validation(format!(
            "legacy block resolver returned an incomplete canonical BlockState for id={}, data={}",
            block.id, block.data
        )));
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::NbtTag;
    use std::collections::BTreeMap;

    fn test_state(name: &str) -> BlockState {
        BlockState {
            name: name.to_string(),
            states: BTreeMap::from([("test".to_string(), NbtTag::Byte(1))]),
            version: Some(18_168_865),
        }
    }

    #[test]
    fn mapping_uses_fixed_numeric_slots() {
        let mut mapping = LegacyBlockMapping::new();
        mapping
            .insert(
                LegacyBlockReference { id: 1, data: 0 },
                test_state("minecraft:stone"),
            )
            .unwrap();
        assert!(mapping
            .resolve(
                LegacyBlockSource::LegacyTerrain,
                LegacyBlockReference { id: 1, data: 1 }
            )
            .is_none());
        assert_eq!(mapping.len(), 1);
    }

    #[test]
    fn mapping_rejects_metadata_outside_nibble() {
        let mut mapping = LegacyBlockMapping::new();
        assert!(mapping
            .insert(
                LegacyBlockReference { id: 1, data: 16 },
                test_state("minecraft:stone"),
            )
            .is_err());
    }
}
