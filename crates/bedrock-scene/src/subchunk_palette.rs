use super::*;

/// Cache the palette classification for one 16^3 subchunk. PackedIndices
/// provides a direct O(1) palette index lookup, so there is no reason to
/// normalize block names or check whether a water storage exists 4,096 times.
/// A palette with no renderable states is guaranteed not to emit any geometry.
pub(super) struct SubchunkPalette {
    pub(super) primary: Vec<BlockClass>,
    pub(super) renderable: bool,
    pub(super) has_fluid: bool,
    /// Only storages with water/lava entries, ordered top layer first.
    /// Dense palette indices are retained without per-block state-name checks.
    pub(super) fluid_layers: Vec<(usize, Vec<Option<BlockClass>>)>,
}

impl SubchunkPalette {
    pub(super) fn classify(subchunk: &bedrock_world::SubChunk) -> Self {
        let bedrock_world::SubChunkFormat::Paletted { storages, .. } = &subchunk.format else {
            // Unknown/legacy subchunk formats keep the old conservative path.
            return Self {
                primary: Vec::new(),
                renderable: true,
                has_fluid: true,
                fluid_layers: Vec::new(),
            };
        };
        let mut primary = Vec::new();
        let mut renderable = false;
        let mut fluid_layers = Vec::new();
        for (index, storage) in storages.iter().enumerate() {
            let mut classes = Vec::with_capacity(storage.states.len());
            let mut contains_fluid = false;
            for state in &storage.states {
                let class = block_class(&state.name);
                if index == 0 {
                    primary.push(class);
                }
                renderable |= block_class_is_renderable(class);
                let fluid = matches!(class, BlockClass::Water | BlockClass::Lava).then_some(class);
                contains_fluid |= fluid.is_some();
                classes.push(fluid);
            }
            if contains_fluid {
                fluid_layers.push((index, classes));
            }
        }
        // Bedrock's visible_block_states_at scans storages in reverse; match
        // that order when several distinct fluid layers overlap.
        fluid_layers.reverse();
        Self {
            primary,
            renderable,
            has_fluid: !fluid_layers.is_empty(),
            fluid_layers,
        }
    }

    pub(super) fn fluid_at<'a>(
        &self,
        subchunk: &'a bedrock_world::SubChunk,
        x: u8,
        y: u8,
        z: u8,
    ) -> Option<(&'a BlockState, BlockClass)> {
        if !self.has_fluid {
            return None;
        }
        if let bedrock_world::SubChunkFormat::Paletted { storages, .. } = &subchunk.format {
            for (layer_index, classes) in &self.fluid_layers {
                let Some(storage) = storages.get(*layer_index) else {
                    continue;
                };
                let Some(palette_index) = storage.index_at(x, y, z) else {
                    continue;
                };
                let palette_index = usize::from(palette_index);
                let Some(Some(class)) = classes.get(palette_index) else {
                    continue;
                };
                let Some(state) = storage.states.get(palette_index) else {
                    continue;
                };
                return Some((state, *class));
            }
            return None;
        }
        subchunk.visible_block_states_at(x, y, z).find_map(|state| {
            let class = block_class(&state.name);
            matches!(class, BlockClass::Water | BlockClass::Lava).then_some((state, class))
        })
    }
}

#[cfg(test)]
mod palette_collect_tests {
    use super::*;

    fn section(layers: &[&[&str]]) -> bedrock_world::SubChunk {
        bedrock_world::SubChunk {
            y: 0,
            format: bedrock_world::SubChunkFormat::Paletted {
                version: 9,
                storages: layers
                    .iter()
                    .map(|layer| {
                        ::bedrock_world::block::BlockPalette::with_unpacked_indices(
                            layer
                                .iter()
                                .map(|name| BlockState {
                                    name: (*name).to_owned(),
                                    states: BTreeMap::new(),
                                    version: None,
                                })
                                .collect(),
                            vec![0; 4096],
                            None,
                        )
                    })
                    .collect(),
            },
        }
    }

    #[test]
    fn multiple_fluid_storages_use_top_layer_priority() {
        let data = section(&[&["minecraft:water"], &["minecraft:lava"]]);
        let classified = SubchunkPalette::classify(&data);
        assert_eq!(
            classified
                .fluid_layers
                .iter()
                .map(|(i, _)| *i)
                .collect::<Vec<_>>(),
            [1, 0]
        );
        assert_eq!(
            classified.fluid_at(&data, 2, 3, 4).map(|(_, class)| class),
            Some(BlockClass::Lava)
        );
    }

    #[test]
    fn skips_all_air_subchunks_without_losing_waterlogged_overlay() {
        let air = SubchunkPalette::classify(&section(&[&["minecraft:air", "minecraft:cave_air"]]));
        assert!(!air.renderable);
        assert!(!air.has_fluid);

        let waterlogged =
            SubchunkPalette::classify(&section(&[&["minecraft:oak_stairs"], &["minecraft:water"]]));
        assert!(waterlogged.renderable);
        assert!(waterlogged.has_fluid);
        assert_eq!(waterlogged.primary, [BlockClass::DetailOpaque]);
        assert_eq!(waterlogged.fluid_layers.len(), 1);
        assert_eq!(waterlogged.fluid_layers[0].0, 1);
        let waterlogged_data = section(&[&["minecraft:oak_stairs"], &["minecraft:water"]]);
        assert_eq!(
            waterlogged
                .fluid_at(&waterlogged_data, 3, 4, 5)
                .map(|(_, class)| class),
            Some(BlockClass::Water)
        );

        let stone =
            SubchunkPalette::classify(&section(&[&["minecraft:stone", "minecraft:deepslate"]]));
        assert_eq!(stone.primary, [BlockClass::Opaque; 2]);
        assert!(!stone.has_fluid);
        assert!(stone.fluid_layers.is_empty());
    }
}
