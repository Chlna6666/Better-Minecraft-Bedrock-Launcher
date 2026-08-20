//! Typed block editing for modern Bedrock paletted chunks.
//!
//! Writes are grouped by chunk and committed through [`crate::WorldTransaction`]. The editor
//! preserves unrelated raw records, the secondary block layer, biome payloads, and unknown metadata.
//! SubChunk representations not supported by this editor are rejected without invoking world upgrade
//! or downgrade logic.

use crate::nbt::NbtTag;
use crate::parsed::encode_consecutive_roots;
use crate::{
    BedrockWorld, BedrockWorldError, Biome2d, Biome3d, BlockPalette, BlockPos, BlockState, Chunk,
    ChunkCapabilities, ChunkKey, ChunkPos, ChunkRecord, ChunkRecordTag, ChunkVersion,
    CompatibilityLevel, Dimension, Result, SubChunkFormat, WorldStorageHandle, WriteGuard,
    block_storage_index,
};
use bytes::Bytes;
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet};

const BLOCKS_PER_SUBCHUNK: usize = 4096;
const DEFAULT_NEW_SUBCHUNK_VERSION: u8 = 9;
const DEFAULT_COMMIT_BATCH_CHUNKS: usize = 16;

/// Block storage layer targeted by one edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlockStorageLayer {
    /// Primary terrain layer.
    Primary,
    /// Secondary layer used by water/overlay-style block storage.
    Secondary,
}

/// Optional block-entity action associated with a block edit.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockEntityEdit {
    /// Leave any existing block entity untouched.
    Preserve,
    /// Remove an existing block entity at this position.
    Remove,
    /// Replace/create the block entity at this position. `x/y/z` are normalized automatically.
    Replace(NbtTag),
}

/// One absolute block-state edit.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEdit {
    /// Dimension containing the block.
    pub dimension: Dimension,
    /// Absolute block position.
    pub position: BlockPos,
    /// Storage layer to modify.
    pub layer: BlockStorageLayer,
    /// Replacement block state.
    pub state: BlockState,
    /// Optional block-entity mutation.
    pub block_entity: BlockEntityEdit,
}

impl BlockEdit {
    /// Creates a primary-layer block edit that preserves block-entity data.
    #[must_use]
    pub fn new(dimension: Dimension, position: BlockPos, state: BlockState) -> Self {
        Self {
            dimension,
            position,
            layer: BlockStorageLayer::Primary,
            state,
            block_entity: BlockEntityEdit::Preserve,
        }
    }
}

/// Settings for typed block writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEditOptions {
    /// Maximum chunks staged into one atomic storage batch.
    pub commit_batch_chunks: usize,
    /// Delete a subchunk record when both supported layers become entirely air.
    pub compact_empty_subchunks: bool,
}

impl Default for BlockEditOptions {
    fn default() -> Self {
        Self {
            commit_batch_chunks: DEFAULT_COMMIT_BATCH_CHUNKS,
            compact_empty_subchunks: true,
        }
    }
}

/// Result of a typed block-edit operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockEditResult {
    /// Number of edits applied.
    pub edited_blocks: usize,
    /// Chunks changed by the operation.
    pub affected_chunks: BTreeSet<ChunkPos>,
    /// Number of atomic storage commits used.
    pub commits: usize,
}

#[derive(Debug, Clone)]
struct EditableSubchunk {
    y: i8,
    version: u8,
    primary: Vec<BlockState>,
    secondary: Vec<BlockState>,
}

impl EditableSubchunk {
    fn new_air(y: i8, version: u8, state_version: i32) -> Self {
        let air = air_state(state_version);
        Self {
            y,
            version,
            primary: vec![air.clone(); BLOCKS_PER_SUBCHUNK],
            secondary: vec![air; BLOCKS_PER_SUBCHUNK],
        }
    }

    fn from_chunk(chunk: &Chunk, y: i8, state_version: i32) -> Result<Self> {
        let Some(subchunk) = chunk.get_subchunk(y)? else {
            return Ok(Self::new_air(
                y,
                DEFAULT_NEW_SUBCHUNK_VERSION,
                state_version,
            ));
        };
        let SubChunkFormat::Paletted { version, storages } = subchunk.format else {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "chunk {},{} subchunk {y} is not a V8/V9 paletted SubChunk",
                chunk.pos.x, chunk.pos.z
            )));
        };
        if !matches!(version, 8 | 9) {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "typed block editing only supports SubChunk V8/V9, got V{version}"
            )));
        }
        if storages.len() > 2 && storages.iter().skip(2).any(palette_contains_non_air) {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "chunk {},{} subchunk {y} contains non-air data above two storage layers",
                chunk.pos.x, chunk.pos.z
            )));
        }

        let mut editable = Self::new_air(y, version, state_version);
        if let Some(primary) = storages.first() {
            fill_layer(primary, &mut editable.primary)?;
        }
        if let Some(secondary) = storages.get(1) {
            fill_layer(secondary, &mut editable.secondary)?;
        }
        Ok(editable)
    }

    fn is_empty(&self) -> bool {
        self.primary.iter().all(is_air) && self.secondary.iter().all(is_air)
    }

    fn has_block_at(&self, local_x: u8, local_y: u8, local_z: u8) -> bool {
        let index = block_storage_index(local_x, local_y, local_z);
        self.primary.get(index).is_some_and(|state| !is_air(state))
            || self
                .secondary
                .get(index)
                .is_some_and(|state| !is_air(state))
    }

    fn encode(&self) -> Result<Bytes> {
        let secondary_has_blocks = self.secondary.iter().any(|state| !is_air(state));
        let storage_count = if secondary_has_blocks { 2 } else { 1 };
        let mut bytes = match self.version {
            8 => vec![8, storage_count],
            9 => vec![9, storage_count, self.y.to_ne_bytes()[0]],
            version => {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "cannot encode SubChunk V{version} with the V8/V9 typed editor"
                )));
            }
        };
        bytes.extend_from_slice(&encode_layer(&self.primary)?);
        if secondary_has_blocks {
            bytes.extend_from_slice(&encode_layer(&self.secondary)?);
        }
        Ok(Bytes::from(bytes))
    }
}

/// Applies typed block edits while preserving unrelated chunk/world records.
pub fn apply_block_edits_blocking<S>(
    world: &BedrockWorld<S>,
    edits: &[BlockEdit],
    guard: &WriteGuard,
    options: BlockEditOptions,
) -> Result<BlockEditResult>
where
    S: WorldStorageHandle,
{
    guard.validate(world)?;
    if edits.is_empty() {
        return Ok(BlockEditResult {
            edited_blocks: 0,
            affected_chunks: BTreeSet::new(),
            commits: 0,
        });
    }
    if options.commit_batch_chunks == 0 {
        return Err(BedrockWorldError::Validation(
            "commit_batch_chunks must be greater than zero".to_string(),
        ));
    }

    let mut grouped = BTreeMap::<ChunkPos, Vec<&BlockEdit>>::new();
    let mut exact_targets = BTreeSet::<(Dimension, i32, i32, i32, BlockStorageLayer)>::new();
    for edit in edits {
        validate_writable_state(&edit.state)?;
        let target = (
            edit.dimension,
            edit.position.x,
            edit.position.y,
            edit.position.z,
            edit.layer,
        );
        if !exact_targets.insert(target) {
            return Err(BedrockWorldError::Validation(format!(
                "duplicate block edit target at {},{},{} layer {:?}",
                edit.position.x, edit.position.y, edit.position.z, edit.layer
            )));
        }
        grouped
            .entry(edit.position.to_chunk_pos(edit.dimension))
            .or_default()
            .push(edit);
    }

    let total_chunks = grouped.len();
    let mut affected_chunks = BTreeSet::new();
    let mut commits = 0usize;
    let mut transaction = Some(world.transaction());

    for (chunk_index, (chunk_pos, chunk_edits)) in grouped.into_iter().enumerate() {
        let existing = world.get_chunk_blocking(chunk_pos)?;
        if existing.records.is_empty() {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "typed block editing refuses to synthesize missing chunk {chunk_pos:?}; create/generate the chunk first"
            )));
        }
        validate_chunk_write_compatibility(chunk_pos, &existing.records)?;
        let fallback_state_version = chunk_edits
            .iter()
            .find_map(|edit| edit.state.version)
            .ok_or_else(|| {
                BedrockWorldError::Validation(
                    "typed block editing requires persisted BlockState version metadata"
                        .to_string(),
                )
            })?;

        let mut updated_subchunks = BTreeMap::<i8, EditableSubchunk>::new();
        let mut touched_columns = [false; 256];
        let mut block_entity_edits = Vec::new();

        for edit in chunk_edits {
            let (_, world_y, _) = edit.position.in_chunk_offset();
            let subchunk_y = i8::try_from(world_y.div_euclid(16)).map_err(|_| {
                BedrockWorldError::Validation(format!(
                    "block y={} cannot be represented as a subchunk index",
                    edit.position.y
                ))
            })?;
            let local_y = u8::try_from(world_y.rem_euclid(16))
                .map_err(|_| BedrockWorldError::Validation("invalid local block y".to_string()))?;
            let (local_x, _, local_z) = edit.position.in_chunk_offset();
            let subchunk = match updated_subchunks.entry(subchunk_y) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => entry.insert(
                    EditableSubchunk::from_chunk(&existing, subchunk_y, fallback_state_version)?,
                ),
            };
            let storage_index = block_storage_index(local_x, local_y, local_z);
            match edit.layer {
                BlockStorageLayer::Primary => subchunk.primary[storage_index] = edit.state.clone(),
                BlockStorageLayer::Secondary => {
                    subchunk.secondary[storage_index] = edit.state.clone();
                }
            }
            touched_columns[usize::from(local_z) * 16 + usize::from(local_x)] = true;
            if !matches!(edit.block_entity, BlockEntityEdit::Preserve) {
                block_entity_edits.push(edit);
            }
        }

        let (chunk_version, mut height_map) = chunk_height_map(&existing.records)?;
        update_height_map(
            &existing,
            chunk_version,
            &updated_subchunks,
            &touched_columns,
            fallback_state_version,
            &mut height_map,
        )?;
        let (height_tag, height_bytes) = encode_height_map(&existing.records, height_map)?;

        let active = transaction.as_mut().ok_or_else(|| {
            BedrockWorldError::ConcurrentWrite("block edit transaction is unavailable".to_string())
        })?;
        active.put_raw_record(&ChunkKey::new(chunk_pos, height_tag), height_bytes);
        for (subchunk_y, subchunk) in &updated_subchunks {
            let key = ChunkKey::subchunk(chunk_pos, *subchunk_y);
            if options.compact_empty_subchunks && subchunk.is_empty() {
                active.delete_raw_record(&key);
            } else {
                active.put_raw_record(&key, subchunk.encode()?);
            }
        }
        // FinalizedState is world-generation metadata. Ordinary block edits preserve the existing
        // record exactly instead of forcing a guessed generation state.
        apply_block_entity_edits(world, active, chunk_pos, &block_entity_edits)?;
        affected_chunks.insert(chunk_pos);

        let completed = chunk_index + 1;
        if completed.is_multiple_of(options.commit_batch_chunks) || completed == total_chunks {
            transaction
                .take()
                .ok_or_else(|| {
                    BedrockWorldError::ConcurrentWrite(
                        "block edit transaction is unavailable".to_string(),
                    )
                })?
                .commit()?;
            commits = commits.saturating_add(1);
            if completed != total_chunks {
                transaction = Some(world.transaction());
            }
        }
    }

    Ok(BlockEditResult {
        edited_blocks: edits.len(),
        affected_chunks,
        commits,
    })
}

fn validate_chunk_write_compatibility(chunk: ChunkPos, records: &[ChunkRecord]) -> Result<()> {
    let capabilities = ChunkCapabilities::inspect(records);
    match capabilities.compatibility {
        CompatibilityLevel::Exact => Ok(()),
        CompatibilityLevel::UnsupportedFuture => {
            Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "chunk {chunk:?} contains a future/unknown persisted representation; raw data is preserved and this V8/V9 editor refuses the write"
            )))
        }
        CompatibilityLevel::Corrupt => Err(BedrockWorldError::CorruptWorld(format!(
            "chunk {chunk:?} is not safe to rewrite"
        ))),
        CompatibilityLevel::ReadCompatible => {
            Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "chunk {chunk:?} contains data that requires raw preservation; this V8/V9 editor cannot rewrite it"
            )))
        }
    }
}

/// Convenience wrapper for replacing one primary-layer block.
pub fn set_block_state_blocking<S>(
    world: &BedrockWorld<S>,
    dimension: Dimension,
    position: BlockPos,
    state: BlockState,
    guard: &WriteGuard,
) -> Result<BlockEditResult>
where
    S: WorldStorageHandle,
{
    apply_block_edits_blocking(
        world,
        &[BlockEdit::new(dimension, position, state)],
        guard,
        BlockEditOptions::default(),
    )
}

fn validate_writable_state(state: &BlockState) -> Result<()> {
    if state.name.trim().is_empty() || state.name == "<invalid>" || state.name == "<unknown>" {
        return Err(BedrockWorldError::Validation(
            "cannot write a block state with an invalid identifier".to_string(),
        ));
    }
    if state.version.is_none() {
        return Err(BedrockWorldError::Validation(format!(
            "block state {} has no persisted BlockState version",
            state.name
        )));
    }
    state.canonical_bytes()?;
    Ok(())
}

fn air_state(version: i32) -> BlockState {
    BlockState {
        name: "minecraft:air".to_string(),
        states: BTreeMap::new(),
        version: Some(version),
    }
}

fn is_air(state: &BlockState) -> bool {
    state.name == "minecraft:air" || state.name == "minecraft:cave_air"
}

fn palette_contains_non_air(palette: &BlockPalette) -> bool {
    let Some(indices) = &palette.indices else {
        return true;
    };
    indices.iter().any(|index| {
        palette
            .states
            .get(usize::from(*index))
            .is_some_and(|state| !is_air(state))
    })
}

fn fill_layer(palette: &BlockPalette, output: &mut [BlockState]) -> Result<()> {
    let indices = palette.indices.as_ref().ok_or_else(|| {
        BedrockWorldError::UnsupportedChunkFormat(
            "typed editing requires full subchunk palette indices".to_string(),
        )
    })?;
    if indices.len() != BLOCKS_PER_SUBCHUNK || output.len() != BLOCKS_PER_SUBCHUNK {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "subchunk index count is {}, expected {BLOCKS_PER_SUBCHUNK}",
            indices.len()
        )));
    }
    for (position, palette_index) in indices.iter().enumerate() {
        output[position] = palette
            .states
            .get(usize::from(*palette_index))
            .ok_or_else(|| {
                BedrockWorldError::CorruptWorld(format!(
                    "palette index {palette_index} exceeds palette length {}",
                    palette.states.len()
                ))
            })?
            .clone();
    }
    Ok(())
}

fn encode_layer(states: &[BlockState]) -> Result<Vec<u8>> {
    if states.len() != BLOCKS_PER_SUBCHUNK {
        return Err(BedrockWorldError::Validation(format!(
            "subchunk layer has {} blocks, expected {BLOCKS_PER_SUBCHUNK}",
            states.len()
        )));
    }
    let mut palette = Vec::<BlockState>::new();
    let mut lookup = BTreeMap::<Vec<u8>, u16>::new();
    let mut indices = Vec::<u16>::with_capacity(BLOCKS_PER_SUBCHUNK);
    for state in states {
        validate_writable_state(state)?;
        let key = storage_identity_bytes(state)?;
        let index = if let Some(index) = lookup.get(&key) {
            *index
        } else {
            let index = u16::try_from(palette.len()).map_err(|_| {
                BedrockWorldError::Validation("subchunk palette exceeds u16".to_string())
            })?;
            palette.push(state.clone());
            lookup.insert(key, index);
            index
        };
        indices.push(index);
    }

    let bits = bits_per_palette_index(palette.len())?;
    let mut bytes = vec![bits << 1];
    if bits != 0 {
        for word in pack_palette_indices(&indices, bits)? {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(
            &i32::try_from(palette.len())
                .map_err(|_| BedrockWorldError::Validation("palette too large".to_string()))?
                .to_le_bytes(),
        );
    }
    for state in &palette {
        bytes.extend_from_slice(&crate::NbtWriter::write_root(&storage_state_nbt(state)?)?);
    }
    Ok(bytes)
}

fn storage_identity_bytes(state: &BlockState) -> Result<Vec<u8>> {
    let mut bytes = state.canonical_bytes()?;
    bytes.push(u8::from(state.version.is_some()));
    if let Some(version) = state.version {
        bytes.extend_from_slice(&version.to_le_bytes());
    }
    Ok(bytes)
}

fn storage_state_nbt(state: &BlockState) -> Result<NbtTag> {
    let version = state.version.ok_or_else(|| {
        BedrockWorldError::Validation(format!("block state {} has no version", state.name))
    })?;
    Ok(NbtTag::Compound(IndexMap::from([
        ("name".to_string(), NbtTag::String(state.name.clone())),
        (
            "states".to_string(),
            NbtTag::Compound(
                state
                    .states
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            ),
        ),
        ("version".to_string(), NbtTag::Int(version)),
    ])))
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
    let mut words = vec![0_u32; indices.len().div_ceil(values_per_word)];
    for (index, value) in indices.iter().copied().enumerate() {
        if u32::from(value) > mask {
            return Err(BedrockWorldError::Validation(format!(
                "palette index {value} does not fit {bits} bits"
            )));
        }
        let word = &mut words[index / values_per_word];
        *word |= u32::from(value) << ((index % values_per_word) * usize::from(bits));
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
    Err(BedrockWorldError::UnsupportedChunkFormat(
        "typed block editing requires an existing Data3D/Data2D heightmap record".to_string(),
    ))
}

fn encode_height_map(
    records: &[ChunkRecord],
    height_map: Vec<i16>,
) -> Result<(ChunkRecordTag, Bytes)> {
    for record in records {
        match record.key.tag {
            ChunkRecordTag::Data3D => {
                let biome = Biome3d::parse(&record.value)?;
                return Ok((
                    ChunkRecordTag::Data3D,
                    Bytes::from(Biome3d::new(height_map, biome.storages)?.encode()?),
                ));
            }
            ChunkRecordTag::Data2D | ChunkRecordTag::Data2DLegacy => {
                let biome = Biome2d::parse(&record.value)?;
                return Ok((
                    record.key.tag,
                    Bytes::from(Biome2d::new(height_map, biome.biomes)?.encode()?),
                ));
            }
            _ => {}
        }
    }
    Err(BedrockWorldError::UnsupportedChunkFormat(
        "cannot encode a heightmap for a chunk without Data3D/Data2D".to_string(),
    ))
}

fn update_height_map(
    existing: &Chunk,
    version: ChunkVersion,
    updated: &BTreeMap<i8, EditableSubchunk>,
    touched: &[bool; 256],
    state_version: i32,
    height_map: &mut [i16],
) -> Result<()> {
    if height_map.len() != 256 {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "heightmap has {} values instead of 256",
            height_map.len()
        )));
    }
    let (min_y, max_y) = existing.pos.y_range(version);
    let mut unresolved = touched.iter().filter(|value| **value).count();
    let mut resolved = [false; 256];
    for subchunk_y in (min_y.div_euclid(16)..=max_y.div_euclid(16)).rev() {
        if unresolved == 0 {
            break;
        }
        let subchunk_y = i8::try_from(subchunk_y).map_err(|_| {
            BedrockWorldError::Validation("heightmap subchunk index overflowed".to_string())
        })?;
        let loaded;
        let subchunk = if let Some(subchunk) = updated.get(&subchunk_y) {
            subchunk
        } else {
            loaded = EditableSubchunk::from_chunk(existing, subchunk_y, state_version)?;
            &loaded
        };
        for local_z in 0..16_u8 {
            for local_x in 0..16_u8 {
                let column = usize::from(local_z) * 16 + usize::from(local_x);
                if !touched[column] || resolved[column] {
                    continue;
                }
                for local_y in (0..16_u8).rev() {
                    if subchunk.has_block_at(local_x, local_y, local_z) {
                        let surface_y = i32::from(subchunk_y) * 16 + i32::from(local_y);
                        height_map[column] = i16::try_from(surface_y - min_y).map_err(|_| {
                            BedrockWorldError::Validation(
                                "surface height does not fit Bedrock heightmap".to_string(),
                            )
                        })?;
                        resolved[column] = true;
                        unresolved = unresolved.saturating_sub(1);
                        break;
                    }
                }
            }
        }
    }
    for (column, is_touched) in touched.iter().enumerate() {
        if *is_touched && !resolved[column] {
            height_map[column] = 0;
        }
    }
    Ok(())
}

fn apply_block_entity_edits<S>(
    world: &BedrockWorld<S>,
    transaction: &mut crate::WorldTransaction<'_, S>,
    chunk: ChunkPos,
    edits: &[&BlockEdit],
) -> Result<()>
where
    S: WorldStorageHandle,
{
    if edits.is_empty() {
        return Ok(());
    }
    let mut roots = world
        .block_entities_in_chunk_blocking(chunk)?
        .into_iter()
        .map(|record| record.entity.nbt)
        .collect::<Vec<_>>();

    for edit in edits {
        roots.retain(|root| !nbt_position_matches(root, edit.position));
        if let BlockEntityEdit::Replace(root) = &edit.block_entity {
            roots.push(normalize_block_entity(root.clone(), edit.position)?);
        }
    }

    let key = ChunkKey::new(chunk, ChunkRecordTag::BlockEntity);
    if roots.is_empty() {
        transaction.delete_raw_record(&key);
    } else {
        transaction.put_raw_record(&key, encode_consecutive_roots(&roots)?);
    }
    Ok(())
}

fn normalize_block_entity(root: NbtTag, position: BlockPos) -> Result<NbtTag> {
    let NbtTag::Compound(mut compound) = root else {
        return Err(BedrockWorldError::Validation(
            "block entity replacement must be an NBT compound".to_string(),
        ));
    };
    compound.insert("x".to_string(), NbtTag::Int(position.x));
    compound.insert("y".to_string(), NbtTag::Int(position.y));
    compound.insert("z".to_string(), NbtTag::Int(position.z));
    Ok(NbtTag::Compound(compound))
}

fn nbt_position_matches(root: &NbtTag, position: BlockPos) -> bool {
    let NbtTag::Compound(compound) = root else {
        return false;
    };
    let read = |name: &str| match compound.get(name) {
        Some(NbtTag::Int(value)) => Some(*value),
        Some(NbtTag::Short(value)) => Some(i32::from(*value)),
        _ => None,
    };
    read("x") == Some(position.x) && read("y") == Some(position.y) && read("z") == Some(position.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_encoding_roundtrips_through_subchunk_parser() {
        let stone = BlockState {
            name: "minecraft:stone".to_string(),
            states: BTreeMap::new(),
            version: Some(18_168_865),
        };
        let mut editable = EditableSubchunk::new_air(0, 9, 18_168_865);
        editable.primary[block_storage_index(1, 2, 3)] = stone.clone();
        let encoded = editable.encode().expect("encode");
        let parsed = crate::chunk::parse_subchunk_with_mode(
            0,
            encoded,
            crate::SubChunkDecodeMode::FullIndices,
        )
        .expect("parse");
        assert_eq!(parsed.block_state_at(1, 2, 3), Some(&stone));
    }
}
