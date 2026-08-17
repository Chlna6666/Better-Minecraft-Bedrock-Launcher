//! Canonical migration decoding for historical numeric Bedrock terrain.
//!
//! `LegacyTerrain` and pre-paletted subchunk versions store numeric block IDs plus four-bit data
//! values. Those numbers are version-dependent game semantics, so the codec does not guess their
//! modern meaning. A caller supplies a [`LegacyBlockResolver`] backed by an authoritative/versioned
//! mapping and receives normal [`crate::BlockState`] values suitable for later palette validation.

use crate::{
    BedrockWorldError, BlockState, LegacyBiomeSample, LegacySubChunk, LegacyTerrain, Result,
    SubChunkCodecKind, block_storage_index,
};
use bytes::Bytes;
use std::collections::BTreeMap;

const BLOCKS_PER_SUBCHUNK: usize = 16 * 16 * 16;
const LEGACY_TERRAIN_SUBCHUNKS: usize = 128 / 16;

/// Numeric block reference stored by pre-paletted Bedrock terrain formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LegacyBlockReference {
    /// Historical numeric block id.
    pub id: u8,
    /// Four-bit historical block data/metadata value.
    pub data: u8,
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
    /// migration codec will stop rather than inventing a modern block state.
    fn resolve(
        &self,
        source: LegacyBlockSource,
        block: LegacyBlockReference,
    ) -> Option<BlockState>;
}

/// Simple exact `(id,data) → BlockState` resolver for generated/static version mapping tables.
#[derive(Debug, Clone, Default)]
pub struct LegacyBlockMapping {
    entries: BTreeMap<LegacyBlockReference, BlockState>,
}

impl LegacyBlockMapping {
    /// Creates an empty exact legacy block mapping.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Inserts one exact numeric block/data mapping.
    pub fn insert(&mut self, block: LegacyBlockReference, state: BlockState) -> Option<BlockState> {
        self.entries.insert(block, state)
    }

    /// Returns the number of exact mappings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no mappings are configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl LegacyBlockResolver for LegacyBlockMapping {
    fn resolve(
        &self,
        _source: LegacyBlockSource,
        block: LegacyBlockReference,
    ) -> Option<BlockState> {
        self.entries.get(&block).cloned()
    }
}

/// One canonical 16×16×16 block layer resolved from a historical numeric subchunk.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedHistoricalSubChunk {
    /// Vertical subchunk index.
    pub y: i8,
    /// Source historical codec family.
    pub source_codec: SubChunkCodecKind,
    /// Primary block layer in Bedrock X-major storage order.
    pub blocks: Vec<BlockState>,
    /// Original source bytes retained for diagnostics/preservation.
    pub raw: Bytes,
}

impl ResolvedHistoricalSubChunk {
    /// Returns a canonical block at local subchunk coordinates.
    #[must_use]
    pub fn block_state_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<&BlockState> {
        self.blocks
            .get(block_storage_index(local_x, local_y, local_z))
    }
}

/// Canonical view resolved from one old `LegacyTerrain` record.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLegacyTerrain {
    /// Eight 16-block-high canonical subchunks covering Y=0..127.
    pub subchunks: Vec<ResolvedHistoricalSubChunk>,
    /// Original 256-byte heightmap in `z * 16 + x` order.
    pub heightmap: Vec<u8>,
    /// Original legacy biome samples in `z * 16 + x` order.
    pub biomes: Vec<LegacyBiomeSample>,
    /// Complete raw `LegacyTerrain` value retained for preservation/diagnostics.
    pub raw: Bytes,
}

/// Resolves a pre-paletted subchunk into canonical block states.
pub fn resolve_legacy_subchunk(
    y: i8,
    subchunk: &LegacySubChunk,
    resolver: &dyn LegacyBlockResolver,
) -> Result<ResolvedHistoricalSubChunk> {
    let source = LegacyBlockSource::LegacySubChunk {
        version: subchunk.version(),
    };
    let mut blocks = Vec::with_capacity(BLOCKS_PER_SUBCHUNK);
    blocks.resize_with(BLOCKS_PER_SUBCHUNK, || invalid_placeholder_state());

    for local_x in 0..16_u8 {
        for local_z in 0..16_u8 {
            for local_y in 0..16_u8 {
                let block = LegacyBlockReference {
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
                };
                let state = resolve_required(resolver, source, block)?;
                blocks[block_storage_index(local_x, local_y, local_z)] = state;
            }
        }
    }

    Ok(ResolvedHistoricalSubChunk {
        y,
        source_codec: SubChunkCodecKind::from_version(Some(subchunk.version())),
        blocks,
        raw: subchunk.raw().clone(),
    })
}

/// Resolves one 16×128×16 `LegacyTerrain` record into eight canonical subchunks.
pub fn resolve_legacy_terrain(
    terrain: &LegacyTerrain,
    resolver: &dyn LegacyBlockResolver,
) -> Result<ResolvedLegacyTerrain> {
    let mut subchunks = Vec::with_capacity(LEGACY_TERRAIN_SUBCHUNKS);
    for subchunk_index in 0..LEGACY_TERRAIN_SUBCHUNKS {
        let y = i8::try_from(subchunk_index).map_err(|_| {
            BedrockWorldError::Validation("legacy terrain subchunk index overflowed".to_string())
        })?;
        let mut blocks = Vec::with_capacity(BLOCKS_PER_SUBCHUNK);
        blocks.resize_with(BLOCKS_PER_SUBCHUNK, || invalid_placeholder_state());
        for local_x in 0..16_u8 {
            for local_z in 0..16_u8 {
                for local_y in 0..16_u8 {
                    let world_y = u8::try_from(subchunk_index * 16 + usize::from(local_y))
                        .map_err(|_| {
                            BedrockWorldError::Validation(
                                "legacy terrain Y coordinate overflowed".to_string(),
                            )
                        })?;
                    let block = LegacyBlockReference {
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
                    };
                    blocks[block_storage_index(local_x, local_y, local_z)] =
                        resolve_required(resolver, LegacyBlockSource::LegacyTerrain, block)?;
                }
            }
        }
        subchunks.push(ResolvedHistoricalSubChunk {
            y,
            source_codec: SubChunkCodecKind::UnknownLegacy(0xff),
            blocks,
            // One LegacyTerrain value backs all eight canonical slices. Keep the raw payload on the
            // outer result rather than cloning 83 KiB eight times.
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

fn invalid_placeholder_state() -> BlockState {
    BlockState {
        name: "<unresolved-legacy>".to_string(),
        states: BTreeMap::new(),
        version: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NbtTag;

    fn test_state(name: &str) -> BlockState {
        BlockState {
            name: name.to_string(),
            states: BTreeMap::from([("test".to_string(), NbtTag::Byte(1))]),
            version: Some(18_168_865),
        }
    }

    #[test]
    fn exact_mapping_never_falls_back_to_metadata_zero() {
        let mut mapping = LegacyBlockMapping::new();
        mapping.insert(
            LegacyBlockReference { id: 1, data: 0 },
            test_state("minecraft:stone"),
        );
        assert!(mapping
            .resolve(
                LegacyBlockSource::LegacyTerrain,
                LegacyBlockReference { id: 1, data: 1 }
            )
            .is_none());
    }

    #[test]
    fn resolver_rejects_incomplete_target_state() {
        struct Invalid;
        impl LegacyBlockResolver for Invalid {
            fn resolve(
                &self,
                _source: LegacyBlockSource,
                _block: LegacyBlockReference,
            ) -> Option<BlockState> {
                Some(invalid_placeholder_state())
            }
        }
        assert!(resolve_required(
            &Invalid,
            LegacyBlockSource::LegacyTerrain,
            LegacyBlockReference { id: 0, data: 0 }
        )
        .is_err());
    }
}
