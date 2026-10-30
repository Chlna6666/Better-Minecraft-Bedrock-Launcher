//! Logical resident payload estimates for decoded subchunks.

use super::{BlockPalette, BlockState, NbtTag, SubChunk, SubChunkFormat};
use std::mem::size_of;

impl SubChunk {
    /// Estimates CPU storage including decoded palettes, indices and nested state values.
    ///
    /// Includes vector capacity and the visible lengths of shared byte slices. Allocator headers,
    /// map bookkeeping and shared backing bytes outside those slices are excluded. Shared payloads
    /// may be counted by multiple owners; this is a cache estimate, not a process RSS measurement.
    #[must_use]
    pub fn estimated_cpu_bytes(&self) -> usize {
        let heap = match &self.format {
            SubChunkFormat::Paletted { storages, .. } => {
                vector_bytes(storages).saturating_add(storages.iter().map(palette_heap_bytes).sum())
            }
            SubChunkFormat::LegacySubChunk(chunk) => chunk.raw().len(),
            SubChunkFormat::Raw { bytes, .. } => bytes.len(),
            SubChunkFormat::LegacyTerrain | SubChunkFormat::FixedArrayV1 => 0,
        };
        size_of::<Self>().saturating_add(heap)
    }
}

fn vector_bytes<T>(values: &Vec<T>) -> usize {
    values.capacity().saturating_mul(size_of::<T>())
}

fn palette_heap_bytes(palette: &BlockPalette) -> usize {
    let indices = palette.indices.as_ref().map_or(0, vector_bytes);
    let packed = palette
        .packed_indices
        .as_ref()
        .map_or(0, |indices| indices.bytes.len());
    let counts = palette.counts.as_ref().map_or(0, vector_bytes);
    vector_bytes(&palette.states)
        .saturating_add(indices)
        .saturating_add(packed)
        .saturating_add(counts)
        .saturating_add(palette.states.iter().map(state_heap_bytes).sum())
}

fn state_heap_bytes(state: &BlockState) -> usize {
    state
        .name
        .capacity()
        .saturating_add(
            state
                .states
                .len()
                .saturating_mul(size_of::<(String, NbtTag)>()),
        )
        .saturating_add(
            state
                .states
                .iter()
                .map(|(name, value)| name.capacity().saturating_add(tag_heap_bytes(value)))
                .sum(),
        )
}

fn tag_heap_bytes(tag: &NbtTag) -> usize {
    match tag {
        NbtTag::ByteArray(values) => vector_bytes(values),
        NbtTag::ByteString(values) => vector_bytes(values),
        NbtTag::String(value) => value.capacity(),
        NbtTag::IntArray(values) => vector_bytes(values),
        NbtTag::LongArray(values) => vector_bytes(values),
        NbtTag::ShortArray(values) => vector_bytes(values),
        NbtTag::List(values) => {
            vector_bytes(values).saturating_add(values.iter().map(tag_heap_bytes).sum())
        }
        NbtTag::Compound(values) => values
            .capacity()
            .saturating_mul(size_of::<(String, NbtTag)>())
            .saturating_add(
                values
                    .iter()
                    .map(|(name, value)| name.capacity().saturating_add(tag_heap_bytes(value)))
                    .sum(),
            ),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::SubChunkDecodeMode;
    use std::collections::BTreeMap;

    #[test]
    fn packed_indices_use_less_storage_with_identical_voxel_lookups() {
        let states = ["minecraft:air", "minecraft:stone"].map(|name| BlockState {
            name: name.into(),
            states: BTreeMap::new(),
            version: Some(1),
        });
        let source = SubChunk {
            y: 0,
            format: SubChunkFormat::Paletted {
                version: 9,
                storages: vec![BlockPalette::with_unpacked_indices(
                    states.into(),
                    (0..4096).map(|index| (index % 2) as u16).collect(),
                    None,
                )],
            },
        };
        let encoded = source.write().unwrap();
        let full = SubChunk::read(0, encoded.clone(), SubChunkDecodeMode::FullIndices).unwrap();
        let packed = SubChunk::read(0, encoded, SubChunkDecodeMode::PackedIndices).unwrap();
        assert!(packed.estimated_cpu_bytes() < full.estimated_cpu_bytes());
        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    assert_eq!(packed.block_state_at(x, y, z), full.block_state_at(x, y, z));
                }
            }
        }
    }

    #[test]
    fn nested_state_values_are_included_in_estimate() {
        let mut palette = BlockPalette::with_unpacked_indices(
            vec![BlockState {
                name: "minecraft:stone".into(),
                states: BTreeMap::new(),
                version: None,
            }],
            vec![0; 4096],
            None,
        );
        let before = palette_heap_bytes(&palette);
        palette.states[0].states.insert(
            "nested".into(),
            NbtTag::List(vec![NbtTag::ByteArray(vec![0; 1024])]),
        );
        assert!(palette_heap_bytes(&palette) >= before + 1024);
    }
}
