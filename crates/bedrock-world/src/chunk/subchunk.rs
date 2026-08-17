//! Versioned Minecraft Bedrock SubChunk payloads and palette decoding.

use super::legacy::LegacySubChunk;
use super::palette::{BlockPalette, BlockState, BlockStatePaletteEntry, PackedPaletteIndices};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, parse_root_nbt_with_consumed};
use crate::surface::is_air_block_name;
use bytes::Bytes;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_SUBCHUNK_PALETTE_LEN: usize = 4096;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
/// Controls whether subchunk parsing keeps full indices or counts only.
pub enum SubChunkDecodeMode {
    /// Decode palette counts without retaining all block indices.
    CountsOnly,
    /// Retain packed palette indices for exact surface-column sampling.
    SurfaceColumns,
    /// Retain packed palette words while allowing full 3D random access.
    PackedIndices,
    #[default]
    /// Decode and retain full block index arrays.
    FullIndices,
}

#[derive(Debug, Clone, PartialEq)]
/// Decoded subchunk payload family.
pub enum SubChunkFormat {
    /// Legacy pre-paletted subchunk payload.
    LegacySubChunk(LegacySubChunk),
    /// Old LevelDB-era terrain record.
    LegacyTerrain,
    /// Old fixed-array v1 subchunk payload.
    FixedArrayV1,
    /// Modern paletted subchunk payload.
    Paletted {
        /// Bedrock format or payload version.
        version: u8,
        /// Block storages decoded from the record.
        storages: Vec<BlockPalette>,
    },
    /// Raw bytes preserved because the payload was not decoded.
    Raw {
        /// Bedrock format or payload version.
        version: Option<u8>,
        /// Raw payload bytes preserved for unsupported formats.
        bytes: Bytes,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// Decoded subchunk at a vertical subchunk index.
pub struct SubChunk {
    /// Vertical subchunk index encoded by the storage key.
    pub y: i8,
    /// Decoded payload family for this value.
    pub format: SubChunkFormat,
}

impl SubChunk {
    #[must_use]
    /// Returns the primary block state at local subchunk coordinates.
    pub fn block_state_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<&BlockState> {
        match &self.format {
            SubChunkFormat::Paletted { storages, .. } => storages
                .first()
                .and_then(|storage| storage.block_state_at(local_x, local_y, local_z)),
            _ => None,
        }
    }

    #[must_use]
    /// Returns the first visible block state at local subchunk coordinates.
    pub fn visible_block_state_at(
        &self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
    ) -> Option<&BlockState> {
        self.visible_block_states_at(local_x, local_y, local_z)
            .next()
    }

    #[must_use]
    /// Iterates visible block states at local subchunk coordinates from top storage to bottom.
    pub fn visible_block_states_at(
        &self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
    ) -> VisibleBlockStatesAt<'_> {
        let storages = match &self.format {
            SubChunkFormat::Paletted { storages, .. } => Some(storages.iter().rev()),
            _ => None,
        };
        VisibleBlockStatesAt {
            storages,
            local_x,
            local_y,
            local_z,
        }
    }

    pub(crate) fn visible_block_surface_states_at(
        &self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
    ) -> VisibleBlockSurfaceStatesAt<'_> {
        let storages = match &self.format {
            SubChunkFormat::Paletted { storages, .. } => Some(storages.iter().enumerate().rev()),
            _ => None,
        };
        VisibleBlockSurfaceStatesAt {
            storages,
            local_x,
            local_y,
            local_z,
        }
    }

    #[must_use]
    /// Returns a legacy block id at local subchunk coordinates when this is a legacy payload.
    pub fn legacy_block_id_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u8> {
        match &self.format {
            SubChunkFormat::LegacySubChunk(subchunk) => {
                subchunk.block_id_at(local_x, local_y, local_z)
            }
            _ => None,
        }
    }

    #[must_use]
    /// Returns legacy block data at local subchunk coordinates when this is a legacy payload.
    pub fn legacy_block_data_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u8> {
        match &self.format {
            SubChunkFormat::LegacySubChunk(subchunk) => {
                subchunk.block_data_at(local_x, local_y, local_z)
            }
            _ => None,
        }
    }
}

/// Iterator over visible block states at a local coordinate.
pub struct VisibleBlockStatesAt<'chunk> {
    storages: Option<std::iter::Rev<std::slice::Iter<'chunk, BlockPalette>>>,
    local_x: u8,
    local_y: u8,
    local_z: u8,
}

impl<'chunk> Iterator for VisibleBlockStatesAt<'chunk> {
    type Item = &'chunk BlockState;

    fn next(&mut self) -> Option<Self::Item> {
        let storages = self.storages.as_mut()?;
        for storage in storages {
            let Some(entry) =
                storage.block_state_with_palette_index_at(self.local_x, self.local_y, self.local_z)
            else {
                continue;
            };
            if !is_air_block_name(&entry.state.name) {
                return Some(entry.state);
            }
        }
        None
    }
}

/// Iterator over visible block states including their storage-layer positions.
pub(crate) struct VisibleBlockSurfaceStatesAt<'chunk> {
    storages: Option<std::iter::Rev<std::iter::Enumerate<std::slice::Iter<'chunk, BlockPalette>>>>,
    local_x: u8,
    local_y: u8,
    local_z: u8,
}

impl<'chunk> Iterator for VisibleBlockSurfaceStatesAt<'chunk> {
    type Item = BlockStatePaletteEntry<'chunk>;

    fn next(&mut self) -> Option<Self::Item> {
        let storages = self.storages.as_mut()?;
        for (storage_index, storage) in storages {
            let Some(mut entry) =
                storage.block_state_with_palette_index_at(self.local_x, self.local_y, self.local_z)
            else {
                continue;
            };
            if !is_air_block_name(&entry.state.name) {
                entry.storage_index = storage_index;
                return Some(entry);
            }
        }
        None
    }
}

/// Parses a SubChunk using full palette indices.
pub fn parse_subchunk(y: i8, bytes: Bytes) -> Result<SubChunk> {
    parse_subchunk_with_mode(y, bytes, SubChunkDecodeMode::FullIndices)
}

/// Parses a SubChunk with the requested palette retention mode.
pub fn parse_subchunk_with_mode(y: i8, bytes: Bytes, mode: SubChunkDecodeMode) -> Result<SubChunk> {
    let version = bytes.first().copied();
    let format = match version {
        Some(0 | 2..=7) => LegacySubChunk::parse(bytes.clone()).map_or_else(
            |_| SubChunkFormat::Raw { version, bytes },
            SubChunkFormat::LegacySubChunk,
        ),
        Some(version @ 1) => parse_exact_palette_storages(&bytes, 1, 1, mode).map_or_else(
            |_| SubChunkFormat::Raw {
                version: Some(version),
                bytes,
            },
            |storages| SubChunkFormat::Paletted { version, storages },
        ),
        Some(version @ 8..=u8::MAX) => parse_paletted_subchunk(version, &bytes, mode)
            .unwrap_or_else(|_| SubChunkFormat::Raw {
                version: Some(version),
                bytes,
            }),
        _ => SubChunkFormat::Raw { version, bytes },
    };
    Ok(SubChunk { y, format })
}

fn parse_paletted_subchunk(
    version: u8,
    bytes: &[u8],
    mode: SubChunkDecodeMode,
) -> Result<SubChunkFormat> {
    let Some(storage_count) = bytes.get(1).copied() else {
        return Err(BedrockWorldError::UnsupportedChunkFormat(
            "paletted subchunk is missing storage count".to_string(),
        ));
    };
    let offsets: &[usize] = if version == 9 { &[3, 2] } else { &[2] };
    for offset in offsets {
        if let Ok(storages) = parse_exact_palette_storages(bytes, *offset, storage_count, mode) {
            return Ok(SubChunkFormat::Paletted { version, storages });
        }
    }
    Err(BedrockWorldError::UnsupportedChunkFormat(
        "unsupported paletted subchunk layout".to_string(),
    ))
}

fn parse_exact_palette_storages(
    bytes: &[u8],
    offset: usize,
    storage_count: u8,
    mode: SubChunkDecodeMode,
) -> Result<Vec<BlockPalette>> {
    let (storages, consumed) = parse_palette_storages(bytes, offset, storage_count, mode)?;
    if consumed != bytes.len() {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "palette storage ended at byte {consumed} but payload has {} bytes",
            bytes.len()
        )));
    }
    Ok(storages)
}

fn parse_palette_storages(
    bytes: &[u8],
    mut offset: usize,
    storage_count: u8,
    mode: SubChunkDecodeMode,
) -> Result<(Vec<BlockPalette>, usize)> {
    let mut storages = Vec::with_capacity(usize::from(storage_count));
    for _ in 0..storage_count {
        let header = *bytes.get(offset).ok_or_else(|| {
            BedrockWorldError::UnsupportedChunkFormat(
                "palette storage header is missing".to_string(),
            )
        })?;
        offset += 1;

        let bits_per_block = header >> 1;
        if !matches!(bits_per_block, 0 | 1 | 2 | 3 | 4 | 5 | 6 | 8 | 16) {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "unsupported bits-per-block value: {bits_per_block}"
            )));
        }

        let word_count = packed_word_count(bits_per_block);
        let words_byte_len = word_count.checked_mul(4).ok_or_else(|| {
            BedrockWorldError::UnsupportedChunkFormat("palette word count overflowed".to_string())
        })?;
        let words_bytes = bytes.get(offset..offset + words_byte_len).ok_or_else(|| {
            BedrockWorldError::UnsupportedChunkFormat(
                "palette block indices are truncated".to_string(),
            )
        })?;
        offset += words_byte_len;

        let palette_len = if bits_per_block == 0 {
            1
        } else {
            let palette_len = read_i32_at(bytes, offset)?;
            offset += 4;
            if palette_len < 0 {
                return Err(BedrockWorldError::UnsupportedChunkFormat(
                    "palette length cannot be negative".to_string(),
                ));
            }
            let palette_len = usize::try_from(palette_len).map_err(|_| {
                BedrockWorldError::UnsupportedChunkFormat("palette length overflowed".to_string())
            })?;
            if palette_len > MAX_SUBCHUNK_PALETTE_LEN {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "palette length {palette_len} exceeds maximum {MAX_SUBCHUNK_PALETTE_LEN}"
                )));
            }
            palette_len
        };

        let mut states = Vec::with_capacity(palette_len);
        for _ in 0..palette_len {
            let (tag, consumed) = parse_root_nbt_with_consumed(&bytes[offset..])?;
            offset += consumed;
            states.push(block_state_from_nbt(tag));
        }

        let mut counts = matches!(
            mode,
            SubChunkDecodeMode::CountsOnly | SubChunkDecodeMode::FullIndices
        )
        .then(|| vec![0_u16; palette_len]);
        let (indices, packed_indices) = match mode {
            SubChunkDecodeMode::FullIndices => {
                let indices = unpack_palette_indices(words_bytes, bits_per_block, palette_len)?;
                for index in &indices {
                    if let Some(count) = counts
                        .as_mut()
                        .and_then(|counts| counts.get_mut(usize::from(*index)))
                    {
                        *count = count.saturating_add(1);
                    }
                }
                (Some(indices), None)
            }
            SubChunkDecodeMode::CountsOnly => {
                count_packed_palette_indices(
                    words_bytes,
                    bits_per_block,
                    palette_len,
                    counts.as_deref_mut().ok_or_else(|| {
                        BedrockWorldError::Validation(
                            "counts-only decode did not allocate palette counts".to_string(),
                        )
                    })?,
                )?;
                (None, None)
            }
            SubChunkDecodeMode::SurfaceColumns | SubChunkDecodeMode::PackedIndices => (
                None,
                Some(PackedPaletteIndices {
                    bytes: Bytes::copy_from_slice(words_bytes),
                    bits_per_block,
                    palette_len,
                }),
            ),
        };
        storages.push(BlockPalette {
            states,
            indices,
            packed_indices,
            counts,
        });
    }
    Ok((storages, offset))
}

fn count_packed_palette_indices(
    words_bytes: &[u8],
    bits_per_block: u8,
    palette_len: usize,
    counts: &mut [u16],
) -> Result<()> {
    if bits_per_block == 0 {
        if let Some(count) = counts.first_mut() {
            *count = 4096;
        }
        return Ok(());
    }
    let values_per_word = usize::from(32 / bits_per_block);
    let mask = (1_u32 << bits_per_block) - 1;
    let mut decoded = 0usize;
    for word_bytes in words_bytes.chunks_exact(4) {
        let word = u32::from_le_bytes(
            word_bytes
                .try_into()
                .map_err(|_| BedrockWorldError::CorruptWorld("bad palette word".to_string()))?,
        );
        for item_index in 0..values_per_word {
            if decoded == 4096 {
                return Ok(());
            }
            let value = ((word >> (item_index * usize::from(bits_per_block))) & mask) as u16;
            if usize::from(value) >= palette_len {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "palette index {value} exceeds palette length {palette_len}"
                )));
            }
            if let Some(count) = counts.get_mut(usize::from(value)) {
                *count = count.saturating_add(1);
            }
            decoded = decoded.saturating_add(1);
        }
    }
    if decoded == 4096 {
        Ok(())
    } else {
        Err(BedrockWorldError::UnsupportedChunkFormat(
            "palette block indices are truncated".to_string(),
        ))
    }
}

pub(crate) fn packed_word_count(bits_per_block: u8) -> usize {
    if bits_per_block == 0 {
        return 0;
    }
    let values_per_word = usize::from(32 / bits_per_block);
    4096_usize.div_ceil(values_per_word)
}

fn unpack_palette_indices(
    words_bytes: &[u8],
    bits_per_block: u8,
    palette_len: usize,
) -> Result<Vec<u16>> {
    if bits_per_block == 0 {
        return Ok(vec![0; 4096]);
    }
    let values_per_word = usize::from(32 / bits_per_block);
    let mask = (1_u32 << bits_per_block) - 1;
    let mut indices = Vec::with_capacity(4096);
    for word_bytes in words_bytes.chunks_exact(4) {
        let word = u32::from_le_bytes(
            word_bytes
                .try_into()
                .map_err(|_| BedrockWorldError::CorruptWorld("bad palette word".to_string()))?,
        );
        for item_index in 0..values_per_word {
            if indices.len() == 4096 {
                break;
            }
            let value = ((word >> (item_index * usize::from(bits_per_block))) & mask) as u16;
            if palette_len > 0 && usize::from(value) >= palette_len {
                return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "palette index {value} exceeds palette length {palette_len}"
                )));
            }
            indices.push(value);
        }
    }
    if indices.len() != 4096 {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "palette produced {} block indices instead of 4096",
            indices.len()
        )));
    }
    Ok(indices)
}

fn block_state_from_nbt(tag: NbtTag) -> BlockState {
    let NbtTag::Compound(root) = tag else {
        return BlockState {
            name: "<invalid>".to_string(),
            states: BTreeMap::new(),
            version: None,
        };
    };
    block_state_from_nbt_root(root)
}

fn block_state_from_nbt_root(root: IndexMap<String, NbtTag>) -> BlockState {
    let mut name = None;
    let mut fallback_name = None;
    let mut states_tag = None;
    let mut fallback_states_tag = None;
    let mut saw_states_tag = false;
    let mut version = None;
    let mut fallback_version = None;
    for (key, value) in root {
        match (key.as_str(), value) {
            ("name", NbtTag::String(value)) => name = Some(value),
            ("Name", NbtTag::String(value)) => fallback_name = Some(value),
            ("states", value) => {
                saw_states_tag = true;
                states_tag = Some(value);
            }
            ("States", value) => fallback_states_tag = Some(value),
            ("version", value) => version = int_from_tag(value),
            ("Version", value) => fallback_version = int_from_tag(value),
            _ => {}
        }
    }
    let name = name
        .or(fallback_name)
        .unwrap_or_else(|| "<unknown>".to_string());
    let states = match if saw_states_tag {
        states_tag
    } else {
        fallback_states_tag
    } {
        Some(NbtTag::Compound(values)) => values.into_iter().collect(),
        _ => BTreeMap::new(),
    };
    let version = version.or(fallback_version);
    BlockState {
        name,
        states,
        version,
    }
}

fn int_from_tag(tag: NbtTag) -> Option<i32> {
    match tag {
        NbtTag::Byte(value) => Some(i32::from(value)),
        NbtTag::Short(value) => Some(i32::from(value)),
        NbtTag::Int(value) => Some(value),
        _ => None,
    }
}

fn read_i32_at(bytes: &[u8], offset: usize) -> Result<i32> {
    let slice: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| {
            BedrockWorldError::UnsupportedChunkFormat("i32 field is truncated".to_string())
        })?
        .try_into()
        .map_err(|_| BedrockWorldError::UnsupportedChunkFormat("bad i32 field".to_string()))?;
    Ok(i32::from_le_bytes(slice))
}
