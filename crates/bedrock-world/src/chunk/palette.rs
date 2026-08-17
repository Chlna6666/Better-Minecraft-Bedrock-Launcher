//! Bedrock block states, palettes and packed block-storage access.

use crate::nbt::NbtTag;
use bytes::Bytes;
use std::borrow::Cow;
use std::collections::BTreeMap;

#[must_use]
/// Returns the Bedrock X-major storage index for local 16x16x16 coordinates.
pub fn block_storage_index(local_x: u8, local_y: u8, local_z: u8) -> usize {
    usize::from(local_x) * 256 + usize::from(local_z) * 16 + usize::from(local_y)
}

#[derive(Debug, Clone, PartialEq)]
/// Block state decoded from a Bedrock palette entry.
pub struct BlockState {
    /// Named Bedrock value or identifier.
    pub name: String,
    /// Palette block states in storage order.
    pub states: BTreeMap<String, NbtTag>,
    /// Bedrock format or payload version.
    pub version: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
/// Block palette and optional unpacked indices for a subchunk storage.
pub struct BlockPalette {
    /// Palette block states in storage order.
    pub states: Vec<BlockState>,
    /// Optional unpacked palette indices in Bedrock storage order.
    pub indices: Option<Vec<u16>>,
    /// Packed palette indices retained when an unpacked 4096-entry array is unnecessary.
    pub(crate) packed_indices: Option<PackedPaletteIndices>,
    /// Per-palette-entry usage counts when the decode request needs them.
    pub counts: Option<Vec<u16>>,
}

#[derive(Debug, Clone, PartialEq)]
/// Packed Bedrock palette indices retained for random block lookup.
pub(crate) struct PackedPaletteIndices {
    /// Packed little-endian 32-bit palette words.
    pub(crate) bytes: Bytes,
    /// Number of bits used by one palette index.
    pub(crate) bits_per_block: u8,
    /// Number of entries in the associated block palette.
    pub(crate) palette_len: usize,
}

impl PackedPaletteIndices {
    pub(crate) fn get(&self, index: usize) -> Option<u16> {
        if index >= 4096 {
            return None;
        }
        if self.bits_per_block == 0 {
            return Some(0);
        }
        let values_per_word = usize::from(32 / self.bits_per_block);
        let word_index = index / values_per_word;
        let item_index = index % values_per_word;
        let byte_offset = word_index.checked_mul(4)?;
        let word_bytes: [u8; 4] = self
            .bytes
            .get(byte_offset..byte_offset + 4)?
            .try_into()
            .ok()?;
        let word = u32::from_le_bytes(word_bytes);
        let mask = (1_u32 << self.bits_per_block) - 1;
        let value = ((word >> (item_index * usize::from(self.bits_per_block))) & mask) as u16;
        (usize::from(value) < self.palette_len).then_some(value)
    }

    pub(crate) fn decode_all(&self) -> Option<Vec<u16>> {
        if self.bits_per_block == 0 {
            return Some(vec![0; 4096]);
        }
        let values_per_word = usize::from(32 / self.bits_per_block);
        let mask = (1_u32 << self.bits_per_block) - 1;
        let mut values = Vec::with_capacity(4096);
        for word_bytes in self.bytes.chunks_exact(4) {
            let word = u32::from_le_bytes(word_bytes.try_into().ok()?);
            for item_index in 0..values_per_word {
                if values.len() == 4096 {
                    return Some(values);
                }
                let value =
                    ((word >> (item_index * usize::from(self.bits_per_block))) & mask) as u16;
                if usize::from(value) >= self.palette_len {
                    return None;
                }
                values.push(value);
            }
        }
        (values.len() == 4096).then_some(values)
    }
}

impl BlockPalette {
    #[must_use]
    /// Creates a palette backed by already unpacked block indices.
    pub fn with_unpacked_indices(
        states: Vec<BlockState>,
        indices: Vec<u16>,
        counts: Option<Vec<u16>>,
    ) -> Self {
        Self {
            states,
            indices: Some(indices),
            packed_indices: None,
            counts,
        }
    }

    #[must_use]
    /// Returns the decoded palette index at local subchunk coordinates.
    pub fn palette_index_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<u16> {
        if local_x >= 16 || local_y >= 16 || local_z >= 16 {
            return None;
        }
        let index = block_storage_index(local_x, local_y, local_z);
        self.indices
            .as_ref()
            .and_then(|indices| indices.get(index).copied())
            .or_else(|| self.packed_indices.as_ref()?.get(index))
    }

    #[must_use]
    /// Returns the block state at local subchunk coordinates.
    pub fn block_state_at(&self, local_x: u8, local_y: u8, local_z: u8) -> Option<&BlockState> {
        let palette_index = usize::from(self.palette_index_at(local_x, local_y, local_z)?);
        self.states.get(palette_index)
    }

    pub(crate) fn block_state_with_palette_index_at(
        &self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
    ) -> Option<BlockStatePaletteEntry<'_>> {
        let palette_index = usize::from(self.palette_index_at(local_x, local_y, local_z)?);
        let state = self.states.get(palette_index)?;
        Some(BlockStatePaletteEntry {
            state,
            storage_index: 0,
        })
    }

    pub(crate) fn surface_indices(&self) -> Option<Cow<'_, [u16]>> {
        if let Some(indices) = &self.indices {
            return (indices.len() >= 4096).then(|| Cow::Borrowed(&indices[..4096]));
        }
        self.packed_indices.as_ref()?.decode_all().map(Cow::Owned)
    }
}

#[derive(Debug, Clone, Copy)]
/// Visible block-state entry paired with its subchunk storage-layer index.
pub(crate) struct BlockStatePaletteEntry<'chunk> {
    /// Visible block state.
    pub(crate) state: &'chunk BlockState,
    /// Storage layer containing the block state.
    pub(crate) storage_index: usize,
}
