//! Minecraft Bedrock `.mcstructure` files and world placement helpers.
//!
//! Structure files are uncompressed little-endian Bedrock NBT. The block index
//! arrays use the structure file order documented by Bedrock tooling: X outer,
//! then Y, then Z.

use crate::parsed::encode_consecutive_roots;
use crate::{
    BedrockWorld, BedrockWorldError, Biome2d, Biome3d, BlockPalette, BlockState, ChunkKey,
    ChunkPos, ChunkRecord, ChunkRecordTag, ChunkVersion, NbtReader, NbtTag, NbtWriter, Result,
    SubChunkFormat, WorldStorageHandle, WriteGuard, block_storage_index,
};
use bytes::Bytes;
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet, HashMap, hash_map::Entry};
use std::path::Path;

const STRUCTURE_FORMAT_VERSION: i32 = 1;
const STRUCTURE_LAYER_COUNT: usize = 2;
const DEFAULT_BLOCK_VERSION: i32 = 18_002_711;
const AIR_BLOCK_NAME: &str = "minecraft:air";
const BLOCKS_PER_SUBCHUNK: usize = 4096;
const MAX_STRUCTURE_BLOCKS: i64 = 134_217_728;
const WORLD_WRITE_CHUNK_BATCH_SIZE: usize = 16;

/// 3D size of a Bedrock structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McStructureSize {
    /// Size in the X direction.
    pub x: i32,
    /// Size in the Y direction.
    pub y: i32,
    /// Size in the Z direction.
    pub z: i32,
}

impl McStructureSize {
    /// Creates a validated structure size.
    pub fn new(x: i32, y: i32, z: i32) -> Result<Self> {
        if x <= 0 || y <= 0 || z <= 0 {
            return Err(BedrockWorldError::Validation(format!(
                "structure size must be positive, got {x}x{y}x{z}"
            )));
        }
        let block_count = i64::from(x)
            .checked_mul(i64::from(y))
            .and_then(|value| value.checked_mul(i64::from(z)))
            .ok_or_else(|| {
                BedrockWorldError::Validation("structure size overflowed".to_string())
            })?;
        if block_count > MAX_STRUCTURE_BLOCKS {
            return Err(BedrockWorldError::Validation(format!(
                "structure contains too many blocks: {block_count}; limit is {MAX_STRUCTURE_BLOCKS}"
            )));
        }
        Ok(Self { x, y, z })
    }

    /// Total number of block positions.
    pub fn block_count(self) -> Result<usize> {
        let count = i64::from(self.x)
            .checked_mul(i64::from(self.y))
            .and_then(|value| value.checked_mul(i64::from(self.z)))
            .ok_or_else(|| {
                BedrockWorldError::Validation("structure size overflowed".to_string())
            })?;
        usize::try_from(count).map_err(|_| {
            BedrockWorldError::Validation("structure block count does not fit usize".to_string())
        })
    }

    /// Index in `.mcstructure` block index order.
    pub fn index(self, x: i32, y: i32, z: i32) -> Result<usize> {
        if x < 0 || y < 0 || z < 0 || x >= self.x || y >= self.y || z >= self.z {
            return Err(BedrockWorldError::Validation(format!(
                "structure coordinate out of bounds: ({x}, {y}, {z}) in {}x{}x{}",
                self.x, self.y, self.z
            )));
        }
        let index = i64::from(x)
            .checked_mul(i64::from(self.y))
            .and_then(|value| value.checked_mul(i64::from(self.z)))
            .and_then(|value| value.checked_add(i64::from(y) * i64::from(self.z)))
            .and_then(|value| value.checked_add(i64::from(z)))
            .ok_or_else(|| {
                BedrockWorldError::Validation("structure index overflowed".to_string())
            })?;
        usize::try_from(index).map_err(|_| {
            BedrockWorldError::Validation("structure index does not fit usize".to_string())
        })
    }
}

/// A block palette entry in a `.mcstructure` file.
#[derive(Debug, Clone, PartialEq)]
pub struct McStructurePaletteEntry {
    /// Bedrock block identifier.
    pub name: String,
    /// Bedrock block states.
    pub states: BTreeMap<String, NbtTag>,
    /// Optional Bedrock block version.
    pub version: Option<i32>,
}

impl McStructurePaletteEntry {
    /// Creates a structure palette entry from a decoded chunk block state.
    pub fn from_block_state(state: &BlockState) -> Self {
        Self {
            name: state.name.clone(),
            states: state.states.clone(),
            version: state.version,
        }
    }

    /// Air palette entry.
    pub fn air() -> Self {
        Self {
            name: AIR_BLOCK_NAME.to_string(),
            states: BTreeMap::new(),
            version: Some(DEFAULT_BLOCK_VERSION),
        }
    }

    fn key(&self) -> String {
        let states = self
            .states
            .iter()
            .map(|(key, value)| format!("{key}={value:?}"))
            .collect::<Vec<_>>()
            .join(",");
        format!("{}|{}|{:?}", self.name, states, self.version)
    }

    fn to_nbt(&self) -> NbtTag {
        NbtTag::Compound(IndexMap::from([
            ("name".to_string(), NbtTag::String(self.name.clone())),
            (
                "states".to_string(),
                NbtTag::Compound(
                    self.states
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                ),
            ),
            (
                "version".to_string(),
                NbtTag::Int(self.version.unwrap_or(DEFAULT_BLOCK_VERSION)),
            ),
        ]))
    }

    fn is_air(&self) -> bool {
        self.name == AIR_BLOCK_NAME
    }
}

/// A single block sampled from a structure.
#[derive(Debug, Clone, PartialEq)]
pub struct McStructureBlock {
    /// X coordinate relative to the structure origin.
    pub x: i32,
    /// Y coordinate relative to the structure origin.
    pub y: i32,
    /// Z coordinate relative to the structure origin.
    pub z: i32,
    /// Primary-layer palette index, or `-1`.
    pub primary: i32,
    /// Secondary-layer palette index, or `-1`.
    pub secondary: i32,
}

/// Rotation applied while placing a structure into a world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum McStructureRotation {
    /// Keep the structure orientation unchanged.
    None,
    /// Rotate clockwise around the Y axis.
    Clockwise90,
    /// Rotate 180 degrees around the Y axis.
    Rotate180,
    /// Rotate counter-clockwise around the Y axis.
    CounterClockwise90,
}

impl McStructureRotation {
    #[must_use]
    /// Rotates chunk-relative offsets.
    pub const fn rotate_chunk_delta(self, delta_x: i32, delta_z: i32) -> (i32, i32) {
        match self {
            Self::None => (delta_x, delta_z),
            Self::Clockwise90 => (-delta_z, delta_x),
            Self::Rotate180 => (-delta_x, -delta_z),
            Self::CounterClockwise90 => (delta_z, -delta_x),
        }
    }

    const fn rotate_local_xz(self, local_x: u8, local_z: u8) -> (u8, u8) {
        match self {
            Self::None => (local_x, local_z),
            Self::Clockwise90 => (15 - local_z, local_x),
            Self::Rotate180 => (15 - local_x, 15 - local_z),
            Self::CounterClockwise90 => (local_z, 15 - local_x),
        }
    }
}

/// Chunk anchor and height used to place a structure in a world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McStructurePlacement {
    /// Chunk used as the source origin when the structure was copied/imported.
    pub source_anchor: ChunkPos,
    /// Chunk that receives the source anchor after placement.
    pub target_anchor: ChunkPos,
    /// World Y coordinate used as structure relative Y zero.
    pub origin_y: i32,
    /// Rotation applied around the Y axis.
    pub rotation: McStructureRotation,
    /// Mirror source X offsets before rotation.
    pub mirror_x: bool,
    /// Mirror source Z offsets before rotation.
    pub mirror_z: bool,
}

/// Progress phase emitted by structure world writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McStructureWritePhase {
    /// The write is preparing and grouping block placements.
    Prepare,
    /// The write is merging structure data into chunk records.
    WriteChunks,
}

/// Progress emitted by structure world writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McStructureWriteProgress {
    /// Current phase.
    pub phase: McStructureWritePhase,
    /// Completed units in this phase.
    pub completed: usize,
    /// Total units in this phase.
    pub total: usize,
}

/// Result of writing a structure into a world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McStructureWriteResult {
    /// Chunks whose terrain records were changed.
    pub affected_chunks: BTreeSet<ChunkPos>,
    /// Number of block positions considered during placement.
    pub placed_blocks: usize,
}

/// In-memory representation of a Bedrock `.mcstructure` file.
#[derive(Debug, Clone, PartialEq)]
pub struct McStructureFile {
    /// Structure dimensions.
    pub size: McStructureSize,
    /// World origin saved in the file.
    pub world_origin: [i32; 3],
    /// Shared block palette.
    pub palette: Vec<McStructurePaletteEntry>,
    /// Primary block layer indices in structure order.
    pub primary_indices: Vec<i32>,
    /// Secondary block layer indices in structure order.
    pub secondary_indices: Vec<i32>,
    /// Entity NBT entries.
    pub entities: Vec<NbtTag>,
    /// Raw `block_position_data` compound.
    pub block_position_data: IndexMap<String, NbtTag>,
}

impl McStructureFile {
    /// Creates an empty air structure with the requested size.
    pub fn new_air(size: McStructureSize, world_origin: [i32; 3]) -> Result<Self> {
        let block_count = size.block_count()?;
        Ok(Self {
            size,
            world_origin,
            palette: vec![McStructurePaletteEntry::air()],
            primary_indices: vec![0; block_count],
            secondary_indices: vec![-1; block_count],
            entities: Vec::new(),
            block_position_data: IndexMap::new(),
        })
    }

    /// Reads a structure from uncompressed little-endian NBT bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let root = NbtReader::new(bytes).parse_root()?;
        Self::from_nbt(root)
    }

    /// Serializes this structure as uncompressed little-endian NBT bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        NbtWriter::write_root(&self.to_nbt()?)
    }

    /// Reads a `.mcstructure` file from disk.
    pub fn read_from_path(path: &Path) -> Result<Self> {
        read_mcstructure_file(path)
    }

    /// Writes this structure to disk as a `.mcstructure` file.
    pub fn write_to_path(&self, path: &Path) -> Result<()> {
        write_mcstructure_file(path, self)
    }

    /// Parses a structure from its root NBT tag.
    pub fn from_nbt(root: NbtTag) -> Result<Self> {
        let NbtTag::Compound(root) = root else {
            return Err(BedrockWorldError::Nbt(
                "mcstructure root must be a compound".to_string(),
            ));
        };
        let format_version = compound_i32(&root, "format_version")?;
        if format_version != STRUCTURE_FORMAT_VERSION {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "unsupported mcstructure format_version {format_version}"
            )));
        }

        let size_values = compound_i32_list(&root, "size", 3)?;
        let size = McStructureSize::new(size_values[0], size_values[1], size_values[2])?;
        let world_origin_values = compound_i32_list(&root, "structure_world_origin", 3)?;
        let structure = compound_child(&root, "structure")?;
        let block_indices = compound_list(structure, "block_indices")?;
        if block_indices.len() != STRUCTURE_LAYER_COUNT {
            return Err(BedrockWorldError::Validation(format!(
                "block_indices must contain 2 layers, got {}",
                block_indices.len()
            )));
        }
        let primary_indices = nbt_i32_list(&block_indices[0])?;
        let secondary_indices = nbt_i32_list(&block_indices[1])?;
        let block_count = size.block_count()?;
        if primary_indices.len() != block_count || secondary_indices.len() != block_count {
            return Err(BedrockWorldError::Validation(format!(
                "block_indices length must match size product {block_count}, got {}/{}",
                primary_indices.len(),
                secondary_indices.len()
            )));
        }

        let palette_root = compound_child(structure, "palette")?;
        let default_palette = compound_child(palette_root, "default")?;
        let palette = compound_list(default_palette, "block_palette")?
            .iter()
            .map(palette_entry_from_nbt)
            .collect::<Result<Vec<_>>>()?;
        let block_position_data = match default_palette.get("block_position_data") {
            Some(NbtTag::Compound(values)) => values.clone(),
            Some(_) => {
                return Err(BedrockWorldError::Nbt(
                    "block_position_data must be a compound".to_string(),
                ));
            }
            None => IndexMap::new(),
        };
        let entities = match structure.get("entities") {
            Some(NbtTag::List(values)) => values.clone(),
            Some(_) => {
                return Err(BedrockWorldError::Nbt(
                    "structure.entities must be a list".to_string(),
                ));
            }
            None => Vec::new(),
        };

        Ok(Self {
            size,
            world_origin: [
                world_origin_values[0],
                world_origin_values[1],
                world_origin_values[2],
            ],
            palette,
            primary_indices,
            secondary_indices,
            entities,
            block_position_data,
        })
    }

    /// Converts this structure to a root NBT tag.
    pub fn to_nbt(&self) -> Result<NbtTag> {
        let block_count = self.size.block_count()?;
        if self.primary_indices.len() != block_count || self.secondary_indices.len() != block_count
        {
            return Err(BedrockWorldError::Validation(format!(
                "mcstructure index arrays must contain {block_count} values"
            )));
        }

        let default_palette = NbtTag::Compound(IndexMap::from([
            (
                "block_palette".to_string(),
                NbtTag::List(
                    self.palette
                        .iter()
                        .map(McStructurePaletteEntry::to_nbt)
                        .collect(),
                ),
            ),
            (
                "block_position_data".to_string(),
                NbtTag::Compound(self.block_position_data.clone()),
            ),
        ]));
        let structure = NbtTag::Compound(IndexMap::from([
            (
                "block_indices".to_string(),
                NbtTag::List(vec![
                    NbtTag::IntArray(self.primary_indices.clone()),
                    NbtTag::IntArray(self.secondary_indices.clone()),
                ]),
            ),
            ("entities".to_string(), NbtTag::List(self.entities.clone())),
            (
                "palette".to_string(),
                NbtTag::Compound(IndexMap::from([("default".to_string(), default_palette)])),
            ),
        ]));
        Ok(NbtTag::Compound(IndexMap::from([
            (
                "format_version".to_string(),
                NbtTag::Int(STRUCTURE_FORMAT_VERSION),
            ),
            (
                "size".to_string(),
                NbtTag::List(vec![
                    NbtTag::Int(self.size.x),
                    NbtTag::Int(self.size.y),
                    NbtTag::Int(self.size.z),
                ]),
            ),
            ("structure".to_string(), structure),
            (
                "structure_world_origin".to_string(),
                NbtTag::List(self.world_origin.iter().copied().map(NbtTag::Int).collect()),
            ),
        ])))
    }

    /// Returns the primary-layer palette index at a relative block position.
    pub fn primary_index_at(&self, x: i32, y: i32, z: i32) -> Result<i32> {
        let index = self.size.index(x, y, z)?;
        self.primary_indices.get(index).copied().ok_or_else(|| {
            BedrockWorldError::CorruptWorld("primary index array is truncated".to_string())
        })
    }

    /// Iterates every block position in structure order.
    pub fn blocks(&self) -> Result<Vec<McStructureBlock>> {
        let mut blocks = Vec::with_capacity(self.size.block_count()?);
        for x in 0..self.size.x {
            for y in 0..self.size.y {
                for z in 0..self.size.z {
                    let index = self.size.index(x, y, z)?;
                    blocks.push(McStructureBlock {
                        x,
                        y,
                        z,
                        primary: self.primary_indices[index],
                        secondary: self.secondary_indices[index],
                    });
                }
            }
        }
        Ok(blocks)
    }

    /// Returns the palette entry for a structure-layer index.
    pub fn palette_entry_or_air(&self, index: i32) -> McStructurePaletteEntry {
        if index < 0 {
            return McStructurePaletteEntry::air();
        }
        let Ok(index) = usize::try_from(index) else {
            return McStructurePaletteEntry::air();
        };
        self.palette
            .get(index)
            .cloned()
            .unwrap_or_else(McStructurePaletteEntry::air)
    }

    #[must_use]
    /// Returns whether a structure-layer index resolves to air.
    pub fn index_is_air(&self, index: i32) -> bool {
        self.palette_entry_or_air(index).is_air()
    }

    /// Computes all chunks touched by this structure placement.
    pub fn target_chunks(&self, placement: McStructurePlacement) -> Result<BTreeSet<ChunkPos>> {
        let source_origin_x = checked_mul_16(placement.source_anchor.x, "structure source x")?;
        let source_origin_z = checked_mul_16(placement.source_anchor.z, "structure source z")?;
        let source_max_x = checked_add_i32(
            source_origin_x,
            self.size.x.saturating_sub(1),
            "structure source x",
        )?;
        let source_max_z = checked_add_i32(
            source_origin_z,
            self.size.z.saturating_sub(1),
            "structure source z",
        )?;
        let min_source_chunk_x = source_origin_x.div_euclid(16);
        let max_source_chunk_x = source_max_x.div_euclid(16);
        let min_source_chunk_z = source_origin_z.div_euclid(16);
        let max_source_chunk_z = source_max_z.div_euclid(16);
        let mut chunks = BTreeSet::new();
        for source_chunk_x in min_source_chunk_x..=max_source_chunk_x {
            for source_chunk_z in min_source_chunk_z..=max_source_chunk_z {
                let source_chunk_x = if placement.mirror_x {
                    min_source_chunk_x
                        .saturating_add(max_source_chunk_x)
                        .saturating_sub(source_chunk_x)
                } else {
                    source_chunk_x
                };
                let source_chunk_z = if placement.mirror_z {
                    min_source_chunk_z
                        .saturating_add(max_source_chunk_z)
                        .saturating_sub(source_chunk_z)
                } else {
                    source_chunk_z
                };
                let delta_x = source_chunk_x.saturating_sub(placement.source_anchor.x);
                let delta_z = source_chunk_z.saturating_sub(placement.source_anchor.z);
                let (target_delta_x, target_delta_z) =
                    placement.rotation.rotate_chunk_delta(delta_x, delta_z);
                chunks.insert(ChunkPos {
                    x: placement.target_anchor.x.saturating_add(target_delta_x),
                    z: placement.target_anchor.z.saturating_add(target_delta_z),
                    dimension: placement.target_anchor.dimension,
                });
            }
        }
        Ok(chunks)
    }

    /// Merges this structure's block layers into a world.
    ///
    /// Block palette data, secondary block layers, and block-position NBT data
    /// are written. Entity placement is intentionally not performed because
    /// Bedrock actor storage requires stable `UniqueID` and digest updates.
    pub fn write_to_world_blocking<S>(
        &self,
        world: &BedrockWorld<S>,
        placement: McStructurePlacement,
        guard: &WriteGuard,
        mut progress: impl FnMut(McStructureWriteProgress),
    ) -> Result<McStructureWriteResult>
    where
        S: WorldStorageHandle,
    {
        guard.validate(world)?;
        let mut placements: BTreeMap<ChunkPos, BTreeMap<i8, Vec<StructureBlockPlacement>>> =
            BTreeMap::new();
        let mut block_entities: BTreeMap<ChunkPos, Vec<NbtTag>> = BTreeMap::new();
        let blocks = self.blocks()?;
        progress(McStructureWriteProgress {
            phase: McStructureWritePhase::Prepare,
            completed: 0,
            total: blocks.len(),
        });

        for block in blocks {
            let block_placement = self.structure_block_placement(&block, placement)?;
            if let Some(block_entity) = self.block_entity_for_block(&block, &block_placement)? {
                block_entities
                    .entry(block_placement.chunk)
                    .or_default()
                    .push(block_entity);
            }
            placements
                .entry(block_placement.chunk)
                .or_default()
                .entry(block_placement.subchunk_y)
                .or_default()
                .push(block_placement);
        }

        if placements.is_empty() {
            return Err(BedrockWorldError::Validation(
                "structure contains no blocks to place".to_string(),
            ));
        }

        let total_chunks = placements.len();
        progress(McStructureWriteProgress {
            phase: McStructureWritePhase::WriteChunks,
            completed: 0,
            total: total_chunks,
        });

        let affected_chunks =
            Self::write_placed_chunks_blocking(world, placements, block_entities, &mut progress)?;

        Ok(McStructureWriteResult {
            affected_chunks,
            placed_blocks: self.size.block_count()?,
        })
    }

    fn write_placed_chunks_blocking<S>(
        world: &BedrockWorld<S>,
        placements: BTreeMap<ChunkPos, BTreeMap<i8, Vec<StructureBlockPlacement>>>,
        mut block_entities: BTreeMap<ChunkPos, Vec<NbtTag>>,
        progress: &mut impl FnMut(McStructureWriteProgress),
    ) -> Result<BTreeSet<ChunkPos>>
    where
        S: WorldStorageHandle,
    {
        let total_chunks = placements.len();
        let mut affected_chunks = BTreeSet::new();
        let mut transaction = Some(world.transaction());
        for (index, (chunk, subchunks)) in placements.into_iter().enumerate() {
            let active_transaction = transaction.as_mut().ok_or_else(|| {
                BedrockWorldError::ConcurrentWrite(
                    "structure write transaction is unavailable".to_string(),
                )
            })?;
            let existing_chunk = world.get_chunk_blocking(chunk)?;
            let mut updated_subchunks = BTreeMap::new();
            let mut touched_columns = [false; 256];
            for (subchunk_y, subchunk_placements) in subchunks {
                let mut subchunk = EntrySubchunkBuilder::from_chunk(&existing_chunk, subchunk_y)?;
                for placement in subchunk_placements {
                    let storage_index = block_storage_index(
                        placement.local_x,
                        placement.local_y,
                        placement.local_z,
                    );
                    subchunk.primary[storage_index] = placement.primary;
                    subchunk.secondary[storage_index] = placement.secondary;
                    touched_columns
                        [usize::from(placement.local_z) * 16 + usize::from(placement.local_x)] =
                        true;
                }
                updated_subchunks.insert(subchunk_y, subchunk);
            }

            let (version, mut height_map) = chunk_height_map(&existing_chunk.records)?;
            update_height_map_from_subchunks(
                chunk,
                version,
                &existing_chunk,
                &updated_subchunks,
                &touched_columns,
                &mut height_map,
            )?;
            let (height_map_tag, height_map_bytes) =
                encode_chunk_height_map(&existing_chunk.records, height_map)?;
            active_transaction
                .put_raw_record(&ChunkKey::new(chunk, height_map_tag), height_map_bytes);

            for (subchunk_y, subchunk) in updated_subchunks {
                let bytes = encode_entry_subchunk(subchunk)?;
                active_transaction
                    .put_raw_record(&ChunkKey::subchunk(chunk, subchunk_y), Bytes::from(bytes));
            }

            active_transaction.put_raw_record(
                &ChunkKey::new(chunk, ChunkRecordTag::FinalizedState),
                Bytes::from(2_i32.to_le_bytes().to_vec()),
            );
            if let Some(structure_entities) = block_entities.remove(&chunk) {
                let mut roots = world
                    .block_entities_in_chunk_blocking(chunk)?
                    .into_iter()
                    .filter_map(|record| match record.entity.position {
                        Some([x, y, z])
                            if structure_entities
                                .iter()
                                .any(|tag| compound_position_matches(tag, x, y, z)) =>
                        {
                            None
                        }
                        _ => Some(record.entity.nbt),
                    })
                    .collect::<Vec<_>>();
                roots.extend(structure_entities);
                if !roots.is_empty() {
                    let value = encode_consecutive_roots(&roots)?;
                    active_transaction
                        .put_raw_record(&ChunkKey::new(chunk, ChunkRecordTag::BlockEntity), value);
                }
            }
            affected_chunks.insert(chunk);
            let completed = index + 1;
            if completed.is_multiple_of(WORLD_WRITE_CHUNK_BATCH_SIZE) || completed == total_chunks {
                let transaction_to_commit = transaction.take().ok_or_else(|| {
                    BedrockWorldError::ConcurrentWrite(
                        "structure write transaction is unavailable".to_string(),
                    )
                })?;
                transaction_to_commit.commit()?;
                progress(McStructureWriteProgress {
                    phase: McStructureWritePhase::WriteChunks,
                    completed,
                    total: total_chunks,
                });
                if completed != total_chunks {
                    transaction = Some(world.transaction());
                }
            }
        }

        Ok(affected_chunks)
    }

    fn structure_block_placement(
        &self,
        block: &McStructureBlock,
        placement: McStructurePlacement,
    ) -> Result<StructureBlockPlacement> {
        let source_origin_x = checked_mul_16(placement.source_anchor.x, "structure source x")?;
        let source_origin_z = checked_mul_16(placement.source_anchor.z, "structure source z")?;
        let relative_x = mirrored_structure_offset(self.size.x, block.x, placement.mirror_x, "x")?;
        let relative_z = mirrored_structure_offset(self.size.z, block.z, placement.mirror_z, "z")?;
        let source_world_x = checked_add_i32(source_origin_x, relative_x, "structure source x")?;
        let source_world_z = checked_add_i32(source_origin_z, relative_z, "structure source z")?;
        let source_chunk_x = source_world_x.div_euclid(16);
        let source_chunk_z = source_world_z.div_euclid(16);
        let local_x = u8::try_from(source_world_x.rem_euclid(16)).map_err(|_| {
            BedrockWorldError::Validation(format!(
                "structure source x has invalid local value: {source_world_x}"
            ))
        })?;
        let local_z = u8::try_from(source_world_z.rem_euclid(16)).map_err(|_| {
            BedrockWorldError::Validation(format!(
                "structure source z has invalid local value: {source_world_z}"
            ))
        })?;

        let delta_x = source_chunk_x.saturating_sub(placement.source_anchor.x);
        let delta_z = source_chunk_z.saturating_sub(placement.source_anchor.z);
        let (target_delta_x, target_delta_z) =
            placement.rotation.rotate_chunk_delta(delta_x, delta_z);
        let target_chunk = ChunkPos {
            x: placement.target_anchor.x.saturating_add(target_delta_x),
            z: placement.target_anchor.z.saturating_add(target_delta_z),
            dimension: placement.target_anchor.dimension,
        };
        let (target_local_x, target_local_z) = placement.rotation.rotate_local_xz(local_x, local_z);
        let target_world_y = checked_add_i32(placement.origin_y, block.y, "structure target y")?;
        let subchunk_y = i8::try_from(target_world_y.div_euclid(16)).map_err(|_| {
            BedrockWorldError::Validation(format!(
                "structure target y cannot be represented as subchunk: {target_world_y}"
            ))
        })?;
        let local_y = u8::try_from(target_world_y.rem_euclid(16)).map_err(|_| {
            BedrockWorldError::Validation(format!(
                "structure target y has invalid local value: {target_world_y}"
            ))
        })?;

        Ok(StructureBlockPlacement {
            chunk: target_chunk,
            subchunk_y,
            local_x: target_local_x,
            local_y,
            local_z: target_local_z,
            primary: transform_palette_entry(self.palette_entry_or_air(block.primary), placement),
            secondary: transform_palette_entry(
                self.palette_entry_or_air(block.secondary),
                placement,
            ),
        })
    }

    fn block_entity_for_block(
        &self,
        block: &McStructureBlock,
        placement: &StructureBlockPlacement,
    ) -> Result<Option<NbtTag>> {
        let index = self.size.index(block.x, block.y, block.z)?;
        let Some(NbtTag::Compound(position_data)) =
            self.block_position_data.get(&index.to_string())
        else {
            return Ok(None);
        };
        let Some(NbtTag::Compound(block_entity_data)) = position_data.get("block_entity_data")
        else {
            return Ok(None);
        };
        let mut entity = block_entity_data.clone();
        let world_x = placement
            .chunk
            .x
            .checked_mul(16)
            .and_then(|value| value.checked_add(i32::from(placement.local_x)))
            .ok_or_else(|| {
                BedrockWorldError::Validation("block entity x overflowed".to_string())
            })?;
        let world_y = i32::from(placement.subchunk_y)
            .checked_mul(16)
            .and_then(|value| value.checked_add(i32::from(placement.local_y)))
            .ok_or_else(|| {
                BedrockWorldError::Validation("block entity y overflowed".to_string())
            })?;
        let world_z = placement
            .chunk
            .z
            .checked_mul(16)
            .and_then(|value| value.checked_add(i32::from(placement.local_z)))
            .ok_or_else(|| {
                BedrockWorldError::Validation("block entity z overflowed".to_string())
            })?;
        entity.insert("x".to_string(), NbtTag::Int(world_x));
        entity.insert("y".to_string(), NbtTag::Int(world_y));
        entity.insert("z".to_string(), NbtTag::Int(world_z));
        Ok(Some(NbtTag::Compound(entity)))
    }

    /// Builds a structure by sampling decoded blocks from a world.
    pub fn from_world_region_blocking(
        world: &BedrockWorld,
        dimension: crate::Dimension,
        min_x: i32,
        min_y: i32,
        min_z: i32,
        size: McStructureSize,
    ) -> Result<Self> {
        let mut structure = Self::new_air(size, [min_x, min_y, min_z])?;
        let mut palette_indices = HashMap::new();
        palette_indices.insert(structure.palette[0].key(), 0_i32);
        let mut chunk_cache = HashMap::new();

        for x in 0..size.x {
            let world_x = min_x + x;
            let chunk_x = world_x.div_euclid(16);
            let local_x = u8::try_from(world_x.rem_euclid(16)).map_err(|_| {
                BedrockWorldError::Validation(format!("invalid local x for block {world_x}"))
            })?;
            for z in 0..size.z {
                let world_z = min_z + z;
                let chunk_z = world_z.div_euclid(16);
                let local_z = u8::try_from(world_z.rem_euclid(16)).map_err(|_| {
                    BedrockWorldError::Validation(format!("invalid local z for block {world_z}"))
                })?;
                let chunk_pos = ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                    dimension,
                };
                if let Entry::Vacant(entry) = chunk_cache.entry(chunk_pos) {
                    let chunk = world.get_chunk_blocking(chunk_pos)?;
                    entry.insert(chunk);
                }
                let chunk = chunk_cache.get(&chunk_pos).ok_or_else(|| {
                    BedrockWorldError::CorruptWorld("chunk cache insert failed".to_string())
                })?;
                for y in 0..size.y {
                    let world_y = min_y + y;
                    let state = match chunk.get_block(
                        local_x,
                        i16::try_from(world_y).map_err(|_| {
                            BedrockWorldError::Validation(format!(
                                "block y={world_y} cannot be represented as i16"
                            ))
                        })?,
                        local_z,
                    ) {
                        Ok(state) => state,
                        Err(error)
                            if matches!(
                                error.kind(),
                                crate::BedrockWorldErrorKind::UnsupportedChunkFormat
                            ) =>
                        {
                            BlockState {
                                name: AIR_BLOCK_NAME.to_string(),
                                states: BTreeMap::new(),
                                version: Some(DEFAULT_BLOCK_VERSION),
                            }
                        }
                        Err(error) => return Err(error),
                    };
                    let entry = McStructurePaletteEntry::from_block_state(&state);
                    let key = entry.key();
                    let palette_index = if let Some(existing) = palette_indices.get(&key) {
                        *existing
                    } else {
                        let new_index = i32::try_from(structure.palette.len()).map_err(|_| {
                            BedrockWorldError::Validation(
                                "mcstructure palette length exceeds i32".to_string(),
                            )
                        })?;
                        structure.palette.push(entry);
                        palette_indices.insert(key, new_index);
                        new_index
                    };
                    let block_index = size.index(x, y, z)?;
                    structure.primary_indices[block_index] = palette_index;
                }
            }
        }

        Ok(structure)
    }
}

/// Reads a `.mcstructure` file from disk.
pub fn read_mcstructure_file(path: &Path) -> Result<McStructureFile> {
    let bytes = std::fs::read(path)?;
    McStructureFile::from_bytes(&bytes)
}

/// Writes a `.mcstructure` file to disk.
pub fn write_mcstructure_file(path: &Path, structure: &McStructureFile) -> Result<()> {
    let bytes = structure.to_bytes()?;
    std::fs::write(path, bytes)?;
    Ok(())
}

struct StructureBlockPlacement {
    chunk: ChunkPos,
    subchunk_y: i8,
    local_x: u8,
    local_y: u8,
    local_z: u8,
    primary: McStructurePaletteEntry,
    secondary: McStructurePaletteEntry,
}

#[derive(Clone)]
struct EntrySubchunkBuilder {
    primary: Vec<McStructurePaletteEntry>,
    secondary: Vec<McStructurePaletteEntry>,
}

impl EntrySubchunkBuilder {
    fn new_air() -> Self {
        let air = McStructurePaletteEntry::air();
        Self {
            primary: vec![air.clone(); BLOCKS_PER_SUBCHUNK],
            secondary: vec![air; BLOCKS_PER_SUBCHUNK],
        }
    }

    fn from_chunk(chunk: &crate::Chunk, y: i8) -> Result<Self> {
        let Some(subchunk) = chunk.get_subchunk(y)? else {
            return Ok(Self::new_air());
        };
        let SubChunkFormat::Paletted { storages, .. } = &subchunk.format else {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "chunk {},{} subchunk {y} is not a mergeable paletted format",
                chunk.pos.x, chunk.pos.z
            )));
        };
        if storages.len() > 2 && storages.iter().skip(2).any(block_palette_contains_non_air) {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "chunk {},{} subchunk {y} contains non-air blocks above two layers",
                chunk.pos.x, chunk.pos.z
            )));
        }

        let mut builder = Self::new_air();
        if let Some(primary) = storages.first() {
            fill_entries_from_palette(primary, &mut builder.primary)?;
        }
        if let Some(secondary) = storages.get(1) {
            fill_entries_from_palette(secondary, &mut builder.secondary)?;
        }
        Ok(builder)
    }
}

fn block_palette_contains_non_air(palette: &BlockPalette) -> bool {
    palette.counts.as_ref().is_some_and(|counts| {
        palette
            .states
            .iter()
            .zip(counts)
            .any(|(state, count)| *count > 0 && state.name != AIR_BLOCK_NAME)
    })
}

fn fill_entries_from_palette(
    palette: &BlockPalette,
    entries: &mut [McStructurePaletteEntry],
) -> Result<()> {
    if entries.len() != BLOCKS_PER_SUBCHUNK {
        return Err(BedrockWorldError::Validation(format!(
            "subchunk entry count invalid: {}",
            entries.len()
        )));
    }
    let Some(indices) = palette.indices.as_ref() else {
        return Err(BedrockWorldError::UnsupportedChunkFormat(
            "subchunk palette has no full indices".to_string(),
        ));
    };
    if indices.len() != BLOCKS_PER_SUBCHUNK {
        return Err(BedrockWorldError::Validation(format!(
            "subchunk palette index count invalid: {}",
            indices.len()
        )));
    }
    for (storage_index, palette_index) in indices.iter().enumerate() {
        let state = palette
            .states
            .get(usize::from(*palette_index))
            .ok_or_else(|| {
                BedrockWorldError::CorruptWorld(format!(
                    "subchunk palette index {} out of range {}",
                    palette_index,
                    palette.states.len()
                ))
            })?;
        entries[storage_index] = McStructurePaletteEntry::from_block_state(state);
    }
    Ok(())
}

fn encode_entry_subchunk(subchunk: EntrySubchunkBuilder) -> Result<Vec<u8>> {
    let secondary_has_blocks = subchunk.secondary.iter().any(|entry| !entry.is_air());
    let mut bytes = vec![8, if secondary_has_blocks { 2 } else { 1 }];
    bytes.extend_from_slice(&encode_palette_storage_entries(&subchunk.primary)?);
    if secondary_has_blocks {
        bytes.extend_from_slice(&encode_palette_storage_entries(&subchunk.secondary)?);
    }
    Ok(bytes)
}

fn encode_palette_storage_entries(entries: &[McStructurePaletteEntry]) -> Result<Vec<u8>> {
    if entries.len() != BLOCKS_PER_SUBCHUNK {
        return Err(BedrockWorldError::Validation(format!(
            "subchunk palette entry count invalid: {}",
            entries.len()
        )));
    }
    let mut palette = vec![McStructurePaletteEntry::air()];
    let mut palette_lookup = HashMap::from([(palette[0].key(), 0_u16)]);
    let mut local_indices = Vec::with_capacity(BLOCKS_PER_SUBCHUNK);
    for entry in entries {
        let key = entry.key();
        let local_index = if let Some(existing) = palette_lookup.get(&key) {
            *existing
        } else {
            let new_index = u16::try_from(palette.len()).map_err(|_| {
                BedrockWorldError::Validation("subchunk palette is too large".to_string())
            })?;
            palette.push(entry.clone());
            palette_lookup.insert(key, new_index);
            new_index
        };
        local_indices.push(local_index);
    }

    let bits = bits_per_palette_index(palette.len())?;
    let mut bytes = Vec::new();
    bytes.push(bits << 1);
    if bits > 0 {
        for word in pack_palette_indices(&local_indices, bits)? {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(
            &i32::try_from(palette.len())
                .map_err(|_| {
                    BedrockWorldError::Validation(
                        "subchunk palette length does not fit i32".to_string(),
                    )
                })?
                .to_le_bytes(),
        );
    }
    for entry in &palette {
        bytes.extend_from_slice(&NbtWriter::write_root(&entry.to_nbt())?);
    }
    Ok(bytes)
}

fn bits_per_palette_index(palette_len: usize) -> Result<u8> {
    match palette_len {
        0 | 1 => Ok(0),
        2 => Ok(1),
        3..=4 => Ok(2),
        5..=8 => Ok(3),
        9..=16 => Ok(4),
        17..=32 => Ok(5),
        33..=64 => Ok(6),
        65..=256 => Ok(8),
        257..=4096 => Ok(16),
        _ => Err(BedrockWorldError::Validation(format!(
            "subchunk palette length exceeds 4096: {palette_len}"
        ))),
    }
}

fn pack_palette_indices(indices: &[u16], bits: u8) -> Result<Vec<u32>> {
    if bits == 0 {
        return Ok(Vec::new());
    }
    let values_per_word = usize::from(32 / bits);
    let mask = (1_u32 << bits) - 1;
    let word_count = indices.len().div_ceil(values_per_word);
    let mut words = vec![0_u32; word_count];
    for (index, value) in indices.iter().enumerate() {
        let value = u32::from(*value);
        if value > mask {
            return Err(BedrockWorldError::Validation(format!(
                "palette index {value} does not fit {bits} bits"
            )));
        }
        let word_index = index / values_per_word;
        let shift = (index % values_per_word) * usize::from(bits);
        let Some(word) = words.get_mut(word_index) else {
            return Err(BedrockWorldError::Validation(
                "packed palette word index out of bounds".to_string(),
            ));
        };
        *word |= value << shift;
    }
    Ok(words)
}

fn chunk_height_map(records: &[ChunkRecord]) -> Result<(ChunkVersion, Vec<i16>)> {
    for record in records {
        match record.key.tag {
            ChunkRecordTag::Data3D => {
                return Biome3d::parse(&record.value)
                    .map(|biome| (ChunkVersion::New, biome.height_map));
            }
            ChunkRecordTag::Data2D | ChunkRecordTag::Data2DLegacy => {
                return Biome2d::parse(&record.value)
                    .map(|biome| (ChunkVersion::Old, biome.height_map));
            }
            _ => {}
        }
    }
    Ok((ChunkVersion::New, vec![0; 256]))
}

fn encode_chunk_height_map(
    records: &[ChunkRecord],
    height_map: Vec<i16>,
) -> Result<(ChunkRecordTag, Bytes)> {
    for record in records {
        match record.key.tag {
            ChunkRecordTag::Data3D => {
                let biome = Biome3d::parse(&record.value)?;
                let bytes = Biome3d::new(height_map, biome.storages)?.encode()?;
                return Ok((ChunkRecordTag::Data3D, Bytes::from(bytes)));
            }
            ChunkRecordTag::Data2D | ChunkRecordTag::Data2DLegacy => {
                let biome = Biome2d::parse(&record.value)?;
                let bytes = Biome2d::new(height_map, biome.biomes)?.encode()?;
                return Ok((record.key.tag, Bytes::from(bytes)));
            }
            _ => {}
        }
    }
    let bytes = Biome3d::new(height_map, Vec::new())?.encode()?;
    Ok((ChunkRecordTag::Data3D, Bytes::from(bytes)))
}

fn update_height_map_from_subchunks(
    chunk: ChunkPos,
    version: ChunkVersion,
    existing_chunk: &crate::Chunk,
    updated_subchunks: &BTreeMap<i8, EntrySubchunkBuilder>,
    touched_columns: &[bool; 256],
    height_map: &mut [i16],
) -> Result<()> {
    let (min_y, max_y) = chunk.y_range(version);
    let min_subchunk = min_y.div_euclid(16);
    let max_subchunk = max_y.div_euclid(16);
    let mut unresolved = touched_columns.iter().filter(|touched| **touched).count();
    let mut resolved = [false; 256];

    for subchunk_y in (min_subchunk..=max_subchunk).rev() {
        if unresolved == 0 {
            break;
        }
        let subchunk_y = i8::try_from(subchunk_y).map_err(|_| {
            BedrockWorldError::Validation(format!(
                "chunk {},{} has an invalid subchunk index {subchunk_y}",
                chunk.x, chunk.z
            ))
        })?;
        let existing;
        let subchunk = if let Some(updated) = updated_subchunks.get(&subchunk_y) {
            updated
        } else {
            existing = EntrySubchunkBuilder::from_chunk(existing_chunk, subchunk_y)?;
            &existing
        };
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                let column_index = usize::from(local_z) * 16 + usize::from(local_x);
                if !touched_columns[column_index] || resolved[column_index] {
                    continue;
                }
                for local_y in (0..16_u8).rev() {
                    let storage_index = block_storage_index(local_x, local_y, local_z);
                    if !subchunk.primary[storage_index].is_air()
                        || !subchunk.secondary[storage_index].is_air()
                    {
                        let surface_y = i32::from(subchunk_y) * 16 + i32::from(local_y);
                        height_map[column_index] =
                            i16::try_from(surface_y - min_y).map_err(|_| {
                                BedrockWorldError::Validation(format!(
                                    "chunk {},{} surface height {surface_y} is out of range",
                                    chunk.x, chunk.z
                                ))
                            })?;
                        resolved[column_index] = true;
                        unresolved = unresolved.saturating_sub(1);
                        break;
                    }
                }
            }
        }
    }

    for (column_index, touched) in touched_columns.iter().enumerate() {
        if *touched && !resolved[column_index] {
            height_map[column_index] = 0;
        }
    }
    Ok(())
}

fn checked_mul_16(value: i32, label: &str) -> Result<i32> {
    value.checked_mul(16).ok_or_else(|| {
        BedrockWorldError::Validation(format!(
            "{label} coordinate overflowed while multiplying by 16"
        ))
    })
}

fn checked_add_i32(left: i32, right: i32, label: &str) -> Result<i32> {
    left.checked_add(right).ok_or_else(|| {
        BedrockWorldError::Validation(format!("{label} coordinate addition overflowed"))
    })
}

fn mirrored_structure_offset(size: i32, offset: i32, mirror: bool, axis: &str) -> Result<i32> {
    if !mirror {
        return Ok(offset);
    }
    let max_offset = size.checked_sub(1).ok_or_else(|| {
        BedrockWorldError::Validation(format!("structure {axis} size must be positive"))
    })?;
    max_offset.checked_sub(offset).ok_or_else(|| {
        BedrockWorldError::Validation(format!(
            "structure {axis} offset is outside mirrored bounds"
        ))
    })
}

fn transform_palette_entry(
    mut entry: McStructurePaletteEntry,
    placement: McStructurePlacement,
) -> McStructurePaletteEntry {
    if entry.states.is_empty() || !placement_changes_horizontal_state(placement) {
        return entry;
    }

    let is_trapdoor = is_trapdoor_block_name(&entry.name);
    transform_direction_string_state(&mut entry.states, "minecraft:cardinal_direction", placement);
    transform_direction_string_state(&mut entry.states, "cardinal_direction", placement);
    transform_direction_string_state(&mut entry.states, "facing", placement);
    transform_direction_string_state(&mut entry.states, "facing_direction", placement);
    transform_direction_string_state(&mut entry.states, "minecraft:block_face", placement);
    transform_direction_string_state(&mut entry.states, "block_face", placement);
    transform_direction_string_state(&mut entry.states, "torch_facing_direction", placement);
    transform_direction_string_state(&mut entry.states, "vine_direction", placement);

    transform_facing_direction_state(&mut entry.states, "facing_direction", placement);
    transform_facing_direction_state(&mut entry.states, "minecraft:facing_direction", placement);
    if is_trapdoor {
        transform_trapdoor_direction_state(&mut entry.states, "direction", placement);
        transform_trapdoor_direction_state(&mut entry.states, "minecraft:direction", placement);
    } else {
        transform_cardinal_direction_state(&mut entry.states, "direction", placement);
        transform_cardinal_direction_state(&mut entry.states, "minecraft:direction", placement);
    }
    transform_cardinal_direction_state(&mut entry.states, "weirdo_direction", placement);
    transform_cardinal_direction_state(&mut entry.states, "minecraft:weirdo_direction", placement);
    transform_sixteen_way_direction_state(&mut entry.states, "ground_sign_direction", placement);
    transform_sixteen_way_direction_state(
        &mut entry.states,
        "minecraft:ground_sign_direction",
        placement,
    );

    transform_directional_state_group(
        &mut entry.states,
        |direction| direction.state_key().to_string(),
        placement,
    );
    transform_directional_state_group(
        &mut entry.states,
        |direction| format!("minecraft:{}", direction.state_key()),
        placement,
    );
    transform_directional_state_group(
        &mut entry.states,
        |direction| format!("{}_bit", direction.state_key()),
        placement,
    );
    transform_directional_state_group(
        &mut entry.states,
        |direction| format!("connected_{}", direction.state_key()),
        placement,
    );
    transform_directional_state_group(
        &mut entry.states,
        |direction| format!("{}_connection_bit", direction.state_key()),
        placement,
    );
    transform_directional_state_group(
        &mut entry.states,
        |direction| format!("{}_wall_bit", direction.state_key()),
        placement,
    );
    transform_directional_state_group(
        &mut entry.states,
        |direction| format!("{}_connection_type", direction.state_key()),
        placement,
    );
    transform_directional_state_group(
        &mut entry.states,
        |direction| format!("wall_connection_type_{}", direction.state_key()),
        placement,
    );

    transform_axis_state(&mut entry.states, "axis", placement);
    transform_axis_state(&mut entry.states, "minecraft:axis", placement);
    transform_axis_state(&mut entry.states, "pillar_axis", placement);
    transform_axis_state(&mut entry.states, "minecraft:pillar_axis", placement);
    transform_axis_state(&mut entry.states, "portal_axis", placement);
    transform_axis_state(&mut entry.states, "minecraft:portal_axis", placement);
    transform_left_right_shape_state(&mut entry.states, "shape", placement);
    transform_left_right_shape_state(&mut entry.states, "minecraft:shape", placement);

    entry
}

fn is_trapdoor_block_name(name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    name == "trapdoor" || name.ends_with("_trapdoor")
}

const fn placement_changes_horizontal_state(placement: McStructurePlacement) -> bool {
    placement.mirror_x
        || placement.mirror_z
        || !matches!(placement.rotation, McStructureRotation::None)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HorizontalDirection {
    North,
    South,
    East,
    West,
}

impl HorizontalDirection {
    const ALL: [Self; 4] = [Self::North, Self::South, Self::East, Self::West];

    const fn state_key(self) -> &'static str {
        match self {
            Self::North => "north",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
        }
    }

    const fn xz(self) -> (i32, i32) {
        match self {
            Self::North => (0, -1),
            Self::South => (0, 1),
            Self::East => (1, 0),
            Self::West => (-1, 0),
        }
    }

    const fn from_xz(x: i32, z: i32) -> Option<Self> {
        match (x, z) {
            (0, -1) => Some(Self::North),
            (0, 1) => Some(Self::South),
            (1, 0) => Some(Self::East),
            (-1, 0) => Some(Self::West),
            _ => None,
        }
    }

    fn transform(self, placement: McStructurePlacement) -> Self {
        let (mut x, mut z) = self.xz();
        if placement.mirror_x {
            x = -x;
        }
        if placement.mirror_z {
            z = -z;
        }
        let (x, z) = placement.rotation.rotate_chunk_delta(x, z);
        Self::from_xz(x, z).unwrap_or(self)
    }

    const fn as_str(self) -> &'static str {
        self.state_key()
    }

    fn from_string(value: &str) -> Option<Self> {
        match value {
            "north" => Some(Self::North),
            "south" => Some(Self::South),
            "east" => Some(Self::East),
            "west" => Some(Self::West),
            _ => None,
        }
    }

    fn from_cardinal_int(value: i32) -> Option<Self> {
        match value.rem_euclid(4) {
            0 => Some(Self::South),
            1 => Some(Self::West),
            2 => Some(Self::North),
            3 => Some(Self::East),
            _ => None,
        }
    }

    const fn to_cardinal_int(self) -> i32 {
        match self {
            Self::South => 0,
            Self::West => 1,
            Self::North => 2,
            Self::East => 3,
        }
    }

    const fn from_facing_int(value: i32) -> Option<Self> {
        match value {
            2 => Some(Self::North),
            3 => Some(Self::South),
            4 => Some(Self::West),
            5 => Some(Self::East),
            _ => None,
        }
    }

    const fn to_facing_int(self) -> i32 {
        match self {
            Self::North => 2,
            Self::South => 3,
            Self::West => 4,
            Self::East => 5,
        }
    }

    const fn from_trapdoor_direction_int(value: i32) -> Option<Self> {
        match value.rem_euclid(4) {
            0 => Some(Self::West),
            1 => Some(Self::East),
            2 => Some(Self::North),
            3 => Some(Self::South),
            _ => None,
        }
    }

    const fn to_trapdoor_direction_int(self) -> i32 {
        match self {
            Self::West => 0,
            Self::East => 1,
            Self::North => 2,
            Self::South => 3,
        }
    }
}

fn transform_direction_string_state(
    states: &mut BTreeMap<String, NbtTag>,
    key: &str,
    placement: McStructurePlacement,
) {
    let Some(NbtTag::String(value)) = states.get_mut(key) else {
        return;
    };
    let Some(direction) = HorizontalDirection::from_string(value) else {
        return;
    };
    *value = direction.transform(placement).as_str().to_string();
}

fn transform_facing_direction_state(
    states: &mut BTreeMap<String, NbtTag>,
    key: &str,
    placement: McStructurePlacement,
) {
    let Some(value) = states.get_mut(key) else {
        return;
    };
    let Some(direction) = nbt_i32_value(value).and_then(HorizontalDirection::from_facing_int)
    else {
        return;
    };
    set_nbt_i32_value(value, direction.transform(placement).to_facing_int());
}

fn transform_cardinal_direction_state(
    states: &mut BTreeMap<String, NbtTag>,
    key: &str,
    placement: McStructurePlacement,
) {
    let Some(value) = states.get_mut(key) else {
        return;
    };
    let Some(direction) = nbt_i32_value(value).and_then(HorizontalDirection::from_cardinal_int)
    else {
        return;
    };
    set_nbt_i32_value(value, direction.transform(placement).to_cardinal_int());
}

fn transform_trapdoor_direction_state(
    states: &mut BTreeMap<String, NbtTag>,
    key: &str,
    placement: McStructurePlacement,
) {
    let Some(value) = states.get_mut(key) else {
        return;
    };
    let Some(direction) =
        nbt_i32_value(value).and_then(HorizontalDirection::from_trapdoor_direction_int)
    else {
        return;
    };
    set_nbt_i32_value(
        value,
        direction.transform(placement).to_trapdoor_direction_int(),
    );
}

fn transform_sixteen_way_direction_state(
    states: &mut BTreeMap<String, NbtTag>,
    key: &str,
    placement: McStructurePlacement,
) {
    let Some(value) = states.get_mut(key) else {
        return;
    };
    let Some(raw_step) = nbt_i32_value(value) else {
        return;
    };
    let mut step = raw_step.rem_euclid(16);
    if placement.mirror_x {
        step = (16 - step).rem_euclid(16);
    }
    if placement.mirror_z {
        step = (8 - step).rem_euclid(16);
    }
    step = match placement.rotation {
        McStructureRotation::None => step,
        McStructureRotation::Clockwise90 => step.saturating_add(4).rem_euclid(16),
        McStructureRotation::Rotate180 => step.saturating_add(8).rem_euclid(16),
        McStructureRotation::CounterClockwise90 => step.saturating_add(12).rem_euclid(16),
    };
    set_nbt_i32_value(value, step);
}

fn transform_directional_state_group(
    states: &mut BTreeMap<String, NbtTag>,
    key_for: impl Fn(HorizontalDirection) -> String,
    placement: McStructurePlacement,
) {
    let mut values = Vec::new();
    for direction in HorizontalDirection::ALL {
        let key = key_for(direction);
        if let Some(value) = states.remove(&key) {
            values.push((direction, value));
        }
    }
    if values.is_empty() {
        return;
    }
    for (direction, value) in values {
        states.insert(key_for(direction.transform(placement)), value);
    }
}

fn transform_axis_state(
    states: &mut BTreeMap<String, NbtTag>,
    key: &str,
    placement: McStructurePlacement,
) {
    if !matches!(
        placement.rotation,
        McStructureRotation::Clockwise90 | McStructureRotation::CounterClockwise90
    ) {
        return;
    }
    let Some(NbtTag::String(value)) = states.get_mut(key) else {
        return;
    };
    match value.as_str() {
        "x" => *value = "z".to_string(),
        "z" => *value = "x".to_string(),
        _ => {}
    }
}

fn transform_left_right_shape_state(
    states: &mut BTreeMap<String, NbtTag>,
    key: &str,
    placement: McStructurePlacement,
) {
    if placement.mirror_x == placement.mirror_z {
        return;
    }
    let Some(NbtTag::String(value)) = states.get_mut(key) else {
        return;
    };
    match value.as_str() {
        "inner_left" => *value = "inner_right".to_string(),
        "inner_right" => *value = "inner_left".to_string(),
        "outer_left" => *value = "outer_right".to_string(),
        "outer_right" => *value = "outer_left".to_string(),
        _ => {}
    }
}

fn set_nbt_i32_value(tag: &mut NbtTag, value: i32) {
    match tag {
        NbtTag::Byte(current) => match i8::try_from(value) {
            Ok(value) => *current = value,
            Err(_) => *tag = NbtTag::Int(value),
        },
        NbtTag::Short(current) => match i16::try_from(value) {
            Ok(value) => *current = value,
            Err(_) => *tag = NbtTag::Int(value),
        },
        NbtTag::Int(current) => *current = value,
        NbtTag::Long(current) => *current = i64::from(value),
        _ => {}
    }
}

fn compound_position_matches(tag: &NbtTag, x: i32, y: i32, z: i32) -> bool {
    let NbtTag::Compound(root) = tag else {
        return false;
    };
    root.get("x").and_then(nbt_i32_value) == Some(x)
        && root.get("y").and_then(nbt_i32_value) == Some(y)
        && root.get("z").and_then(nbt_i32_value) == Some(z)
}

fn nbt_i32_value(tag: &NbtTag) -> Option<i32> {
    match tag {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn palette_entry_from_nbt(tag: &NbtTag) -> Result<McStructurePaletteEntry> {
    let NbtTag::Compound(root) = tag else {
        return Err(BedrockWorldError::Nbt(
            "block palette entry must be a compound".to_string(),
        ));
    };
    let name = compound_string(root, "name")?.to_string();
    let states = match root.get("states") {
        Some(NbtTag::Compound(values)) => values
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        Some(_) => {
            return Err(BedrockWorldError::Nbt(
                "block palette states must be a compound".to_string(),
            ));
        }
        None => BTreeMap::new(),
    };
    let version = match root.get("version") {
        Some(value) => Some(nbt_i32(value)?),
        None => None,
    };
    Ok(McStructurePaletteEntry {
        name,
        states,
        version,
    })
}

fn compound_child<'a>(
    root: &'a IndexMap<String, NbtTag>,
    key: &str,
) -> Result<&'a IndexMap<String, NbtTag>> {
    match root.get(key) {
        Some(NbtTag::Compound(value)) => Ok(value),
        Some(_) => Err(BedrockWorldError::Nbt(format!("{key} must be a compound"))),
        None => Err(BedrockWorldError::Nbt(format!("{key} is missing"))),
    }
}

fn compound_list<'a>(root: &'a IndexMap<String, NbtTag>, key: &str) -> Result<&'a [NbtTag]> {
    match root.get(key) {
        Some(NbtTag::List(values)) => Ok(values),
        Some(_) => Err(BedrockWorldError::Nbt(format!("{key} must be a list"))),
        None => Err(BedrockWorldError::Nbt(format!("{key} is missing"))),
    }
}

fn compound_i32(root: &IndexMap<String, NbtTag>, key: &str) -> Result<i32> {
    let value = root
        .get(key)
        .ok_or_else(|| BedrockWorldError::Nbt(format!("{key} is missing")))?;
    nbt_i32(value)
}

fn compound_i32_list(
    root: &IndexMap<String, NbtTag>,
    key: &str,
    expected_len: usize,
) -> Result<Vec<i32>> {
    let values = compound_list(root, key)?;
    if values.len() != expected_len {
        return Err(BedrockWorldError::Validation(format!(
            "{key} must contain {expected_len} integers, got {}",
            values.len()
        )));
    }
    values.iter().map(nbt_i32).collect()
}

fn compound_string<'a>(root: &'a IndexMap<String, NbtTag>, key: &str) -> Result<&'a str> {
    match root.get(key) {
        Some(NbtTag::String(value)) => Ok(value),
        Some(_) => Err(BedrockWorldError::Nbt(format!("{key} must be a string"))),
        None => Err(BedrockWorldError::Nbt(format!("{key} is missing"))),
    }
}

fn nbt_i32(tag: &NbtTag) -> Result<i32> {
    match tag {
        NbtTag::Byte(value) => Ok(i32::from(*value)),
        NbtTag::Short(value) => Ok(i32::from(*value)),
        NbtTag::Int(value) => Ok(*value),
        NbtTag::Long(value) => i32::try_from(*value).map_err(|_| {
            BedrockWorldError::Validation(format!("integer value {value} does not fit i32"))
        }),
        _ => Err(BedrockWorldError::Nbt(
            "expected integer NBT tag".to_string(),
        )),
    }
}

fn nbt_i32_list(tag: &NbtTag) -> Result<Vec<i32>> {
    match tag {
        NbtTag::List(values) => values.iter().map(nbt_i32).collect(),
        NbtTag::IntArray(values) => Ok(values.clone()),
        _ => Err(BedrockWorldError::Nbt(
            "expected integer list NBT tag".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dimension, MemoryStorage, OpenOptions};
    use std::sync::Arc;

    #[test]
    fn mcstructure_roundtrip_preserves_core_fields() {
        let size = McStructureSize::new(2, 3, 4).expect("valid size");
        let mut structure = McStructureFile::new_air(size, [10, 64, -3]).expect("air structure");
        structure.palette.push(McStructurePaletteEntry {
            name: "minecraft:stone".to_string(),
            states: BTreeMap::new(),
            version: Some(1),
        });
        let index = size.index(1, 2, 3).expect("index");
        structure.primary_indices[index] = 1;

        let bytes = structure.to_bytes().expect("serialize");
        let parsed = McStructureFile::from_bytes(&bytes).expect("parse");

        assert_eq!(parsed.size, size);
        assert_eq!(parsed.world_origin, [10, 64, -3]);
        assert_eq!(parsed.palette.len(), 2);
        assert_eq!(parsed.primary_index_at(1, 2, 3).expect("primary"), 1);
        assert_eq!(parsed.primary_index_at(0, 0, 0).expect("air"), 0);
    }

    #[test]
    fn structure_write_progress_reports_only_committed_chunk_batches() {
        let size = McStructureSize::new(17 * 16, 1, 1).expect("valid structure size");
        let structure = McStructureFile::new_air(size, [0, 0, 0]).expect("air structure");
        let world = BedrockWorld::from_storage(
            "memory",
            Arc::new(MemoryStorage::new()),
            OpenOptions {
                read_only: false,
                ..OpenOptions::default()
            },
        );
        let target_anchor = ChunkPos {
            x: 100,
            z: -20,
            dimension: Dimension::Overworld,
        };
        let guard = WriteGuard::confirmed("memory", "structure progress test");
        let mut committed = Vec::new();

        structure
            .write_to_world_blocking(
                &world,
                McStructurePlacement {
                    source_anchor: ChunkPos {
                        x: 0,
                        z: 0,
                        dimension: Dimension::Overworld,
                    },
                    target_anchor,
                    origin_y: 0,
                    rotation: McStructureRotation::None,
                    mirror_x: false,
                    mirror_z: false,
                },
                &guard,
                |progress| {
                    if progress.phase != McStructureWritePhase::WriteChunks {
                        return;
                    }
                    if progress.completed > 0 {
                        let last_chunk = ChunkPos {
                            x: target_anchor.x
                                + i32::try_from(progress.completed - 1).expect("chunk offset"),
                            ..target_anchor
                        };
                        assert!(
                            world
                                .get_chunk_blocking(last_chunk)
                                .expect("read committed structure chunk")
                                .records
                                .iter()
                                .any(|record| record.key.tag == ChunkRecordTag::Data3D)
                        );
                    }
                    committed.push(progress.completed);
                },
            )
            .expect("write structure");

        assert_eq!(committed, [0, 16, 17]);
    }

    #[test]
    fn mcstructure_index_order_is_x_then_y_then_z() {
        let size = McStructureSize::new(2, 3, 4).expect("valid size");

        assert_eq!(size.index(0, 0, 0).expect("index"), 0);
        assert_eq!(size.index(0, 0, 3).expect("index"), 3);
        assert_eq!(size.index(0, 1, 0).expect("index"), 4);
        assert_eq!(size.index(1, 0, 0).expect("index"), 12);
        assert_eq!(size.index(1, 2, 3).expect("index"), 23);
    }

    #[test]
    fn mcstructure_target_chunks_support_mirror_transform() {
        let size = McStructureSize::new(33, 4, 49).expect("valid size");
        let structure = McStructureFile::new_air(size, [0, 64, 0]).expect("air structure");
        let placement = McStructurePlacement {
            source_anchor: ChunkPos {
                x: 4,
                z: 10,
                dimension: Dimension::Overworld,
            },
            target_anchor: ChunkPos {
                x: -20,
                z: 30,
                dimension: Dimension::End,
            },
            origin_y: 64,
            rotation: McStructureRotation::None,
            mirror_x: true,
            mirror_z: true,
        };

        assert_eq!(
            structure.target_chunks(placement).expect("target chunks"),
            BTreeSet::from([
                ChunkPos {
                    x: -20,
                    z: 30,
                    dimension: Dimension::End,
                },
                ChunkPos {
                    x: -20,
                    z: 31,
                    dimension: Dimension::End,
                },
                ChunkPos {
                    x: -20,
                    z: 32,
                    dimension: Dimension::End,
                },
                ChunkPos {
                    x: -20,
                    z: 33,
                    dimension: Dimension::End,
                },
                ChunkPos {
                    x: -19,
                    z: 30,
                    dimension: Dimension::End,
                },
                ChunkPos {
                    x: -19,
                    z: 31,
                    dimension: Dimension::End,
                },
                ChunkPos {
                    x: -19,
                    z: 32,
                    dimension: Dimension::End,
                },
                ChunkPos {
                    x: -19,
                    z: 33,
                    dimension: Dimension::End,
                },
                ChunkPos {
                    x: -18,
                    z: 30,
                    dimension: Dimension::End,
                },
                ChunkPos {
                    x: -18,
                    z: 31,
                    dimension: Dimension::End,
                },
                ChunkPos {
                    x: -18,
                    z: 32,
                    dimension: Dimension::End,
                },
                ChunkPos {
                    x: -18,
                    z: 33,
                    dimension: Dimension::End,
                },
            ])
        );
    }

    #[test]
    fn mcstructure_block_placement_supports_mirror_local_coordinates() {
        let size = McStructureSize::new(4, 2, 4).expect("valid size");
        let structure = McStructureFile::new_air(size, [0, 64, 0]).expect("air structure");
        let placement = McStructurePlacement {
            source_anchor: ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            target_anchor: ChunkPos {
                x: 8,
                z: -3,
                dimension: Dimension::End,
            },
            origin_y: 70,
            rotation: McStructureRotation::None,
            mirror_x: true,
            mirror_z: true,
        };

        let block_placement = structure
            .structure_block_placement(
                &McStructureBlock {
                    x: 1,
                    y: 1,
                    z: 2,
                    primary: 0,
                    secondary: -1,
                },
                placement,
            )
            .expect("block placement");

        assert_eq!(block_placement.chunk, placement.target_anchor);
        assert_eq!(block_placement.local_x, 2);
        assert_eq!(block_placement.local_y, 7);
        assert_eq!(block_placement.local_z, 1);
    }

    #[test]
    fn mcstructure_block_placement_transforms_horizontal_block_states() {
        let size = McStructureSize::new(1, 1, 1).expect("valid size");
        let mut structure = McStructureFile::new_air(size, [0, 64, 0]).expect("air structure");
        structure.palette.push(McStructurePaletteEntry {
            name: "minecraft:oak_trapdoor".to_string(),
            states: BTreeMap::from([("facing_direction".to_string(), NbtTag::Byte(4))]),
            version: Some(1),
        });
        structure.primary_indices[0] = 1;
        let placement = McStructurePlacement {
            source_anchor: ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            target_anchor: ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            origin_y: 64,
            rotation: McStructureRotation::None,
            mirror_x: true,
            mirror_z: false,
        };

        let block_placement = structure
            .structure_block_placement(
                &McStructureBlock {
                    x: 0,
                    y: 0,
                    z: 0,
                    primary: 1,
                    secondary: -1,
                },
                placement,
            )
            .expect("block placement");

        assert_eq!(
            block_placement.primary.states.get("facing_direction"),
            Some(&NbtTag::Byte(5))
        );
    }

    #[test]
    fn mcstructure_block_placement_transforms_trapdoor_direction_state() {
        let size = McStructureSize::new(1, 1, 1).expect("valid size");
        let mut structure = McStructureFile::new_air(size, [0, 64, 0]).expect("air structure");
        structure.palette.push(McStructurePaletteEntry {
            name: "minecraft:oak_trapdoor".to_string(),
            states: BTreeMap::from([("direction".to_string(), NbtTag::Byte(0))]),
            version: Some(1),
        });
        structure.primary_indices[0] = 1;
        let placement = McStructurePlacement {
            source_anchor: ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            target_anchor: ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            origin_y: 64,
            rotation: McStructureRotation::None,
            mirror_x: true,
            mirror_z: false,
        };

        let block_placement = structure
            .structure_block_placement(
                &McStructureBlock {
                    x: 0,
                    y: 0,
                    z: 0,
                    primary: 1,
                    secondary: -1,
                },
                placement,
            )
            .expect("block placement");

        assert_eq!(
            block_placement.primary.states.get("direction"),
            Some(&NbtTag::Byte(1))
        );
    }

    #[test]
    fn mcstructure_block_placement_transforms_connection_state_keys() {
        let size = McStructureSize::new(1, 1, 1).expect("valid size");
        let mut structure = McStructureFile::new_air(size, [0, 64, 0]).expect("air structure");
        structure.palette.push(McStructurePaletteEntry {
            name: "minecraft:glass_pane".to_string(),
            states: BTreeMap::from([("north".to_string(), NbtTag::Byte(1))]),
            version: Some(1),
        });
        structure.primary_indices[0] = 1;
        let placement = McStructurePlacement {
            source_anchor: ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            target_anchor: ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            origin_y: 64,
            rotation: McStructureRotation::Clockwise90,
            mirror_x: false,
            mirror_z: false,
        };

        let block_placement = structure
            .structure_block_placement(
                &McStructureBlock {
                    x: 0,
                    y: 0,
                    z: 0,
                    primary: 1,
                    secondary: -1,
                },
                placement,
            )
            .expect("block placement");

        assert_eq!(block_placement.primary.states.get("north"), None);
        assert_eq!(
            block_placement.primary.states.get("east"),
            Some(&NbtTag::Byte(1))
        );
    }
}
