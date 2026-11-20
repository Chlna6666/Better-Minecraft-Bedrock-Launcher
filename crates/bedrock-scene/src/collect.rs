use super::*;

/// Collects renderable block records from the selected subchunks.
///
/// # Errors
/// Returns an error if a block cannot be added to the scene.
pub fn collect(
    chunk: &ChunkData,
    block_models: Option<&BlockModelRepository>,
    biome_blend: Option<&BiomeBlendField>,
    include_subchunk: impl Fn(i8) -> bool,
) -> Result<ChunkBlocks, String> {
    let palette = RenderPalette::builtin_shared();
    let mut color_cache = BlockColorCache::new(&palette);
    // Parse each palette once; most generated air subchunks do not require
    // any cell iteration, biome lookup, geometry expansion or hash allocation.
    let subchunk_palettes = chunk
        .subchunks
        .iter()
        .filter(|&(&y, _)| include_subchunk(y))
        .map(|(&y, subchunk)| (y, subchunk, SubchunkPalette::classify(subchunk)))
        .collect::<Vec<_>>();
    let initial_block_capacity = subchunk_palettes
        .iter()
        .filter(|(_, _, classification)| classification.renderable)
        .count()
        .saturating_mul(16 * 16 * 16);
    let mut occupied =
        FxHashSet::<BlockKey>::with_capacity_and_hasher(initial_block_capacity, Default::default());
    let mut foliage = FxHashSet::<BlockKey>::default();
    let mut waterlogged_water = FxHashSet::<BlockKey>::default();
    let mut waterlogged_lava = FxHashSet::<BlockKey>::default();
    let mut glass = FxHashSet::<BlockKey>::with_capacity_and_hasher(
        initial_block_capacity / 8,
        Default::default(),
    );
    let mut water = FxHashSet::<BlockKey>::with_capacity_and_hasher(
        initial_block_capacity / 8,
        Default::default(),
    );
    let mut lava = FxHashSet::<BlockKey>::with_capacity_and_hasher(
        initial_block_capacity / 16,
        Default::default(),
    );
    let mut connection_kinds = FxHashMap::<BlockKey, ConnectionKind>::default();
    let block_entity_states = block_entity_model_states(chunk);
    let mut properties = BlockPropertiesCache::new(block_models);
    let mut opaque_blocks = Vec::<BlockRecord>::with_capacity(initial_block_capacity);
    let mut glass_blocks = Vec::<BlockRecord>::with_capacity(initial_block_capacity / 8);
    let mut detail_blocks = Vec::<DetailBlock>::with_capacity(initial_block_capacity / 8);
    let mut glass_detail_blocks = Vec::<DetailBlock>::with_capacity(initial_block_capacity / 16);
    let mut water_blocks = Vec::<FluidBlock>::with_capacity(initial_block_capacity / 8);
    let mut lava_blocks = Vec::<FluidBlock>::with_capacity(initial_block_capacity / 16);
    let mut block_budget = BlockBudget::unbounded();
    let mut min_y = i16::MAX;
    let mut max_y = i16::MIN;
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_z = i32::MAX;
    let mut max_z = i32::MIN;

    for (subchunk_y, subchunk, classification) in &subchunk_palettes {
        if !classification.renderable {
            continue;
        }
        let base_y = i32::from(*subchunk_y) * 16;
        for local_y in 0u8..16 {
            let y = base_y + i32::from(local_y);
            for local_z in 0u8..16 {
                for local_x in 0u8..16 {
                    let key = BlockKey {
                        x: chunk.pos.x.saturating_mul(16) + i32::from(local_x),
                        y,
                        z: chunk.pos.z.saturating_mul(16) + i32::from(local_z),
                    };
                    // Air cells need no biome lookup. Cache the result for waterlogged
                    // blocks, whose primary and fluid layers share this coordinate.
                    let mut biome = None;
                    let (primary_state, primary_class) = match &subchunk.format {
                        bedrock_world::SubChunkFormat::Paletted { storages, .. } => {
                            let storage = storages.first();
                            let index = storage
                                .and_then(|storage| storage.index_at(local_x, local_y, local_z))
                                .map(usize::from);
                            (
                                storage
                                    .and_then(|storage| index.and_then(|i| storage.states.get(i))),
                                index.and_then(|i| classification.primary.get(i).copied()),
                            )
                        }
                        _ => {
                            let state = subchunk.block_state_at(local_x, local_y, local_z);
                            (state, state.map(|state| block_class(&state.name)))
                        }
                    };
                    if let (Some(state), Some(block_class)) = (primary_state, primary_class) {
                        if block_class_is_renderable(block_class) {
                            push_collected_block(
                                key,
                                state,
                                block_class,
                                *biome.get_or_insert_with(|| {
                                    block_biome_sample(chunk, local_x, local_z, y, biome_blend)
                                }),
                                block_entity_states.get(&key).cloned(),
                                properties.resolve(state, block_class),
                                &mut color_cache,
                                &mut block_budget,
                                &mut connection_kinds,
                                &mut occupied,
                                &mut foliage,
                                &mut glass,
                                &mut water,
                                &mut lava,
                                &mut opaque_blocks,
                                &mut glass_blocks,
                                &mut detail_blocks,
                                &mut glass_detail_blocks,
                                &mut water_blocks,
                                &mut lava_blocks,
                                &mut min_x,
                                &mut max_x,
                                &mut min_y,
                                &mut max_y,
                                &mut min_z,
                                &mut max_z,
                            )?;
                        }
                    }

                    let fluid_state = classification.fluid_at(subchunk, local_x, local_y, local_z);
                    if let Some((state, block_class)) = fluid_state {
                        if primary_class != Some(block_class) {
                            // Official waterlogging (minecraft:liquid_detection): the
                            // contained liquid renders as plain full-square quads —
                            // "a full square, no matter what the collision shape of the
                            // block they share space with is" (use_liquid_clipping is
                            // false for 1.26.0+ blocks). The co-located block simply
                            // draws on top of / through the water layer.
                            // Record the cell as a same-fluid neighbour for face
                            // culling: adjacent waterlogged cells form one water
                            // body, so the walls between them must not render.
                            match block_class {
                                BlockClass::Water => {
                                    waterlogged_water.insert(key);
                                }
                                BlockClass::Lava => {
                                    waterlogged_lava.insert(key);
                                }
                                _ => {}
                            }
                            push_collected_block(
                                key,
                                state,
                                block_class,
                                *biome.get_or_insert_with(|| {
                                    block_biome_sample(chunk, local_x, local_z, y, biome_blend)
                                }),
                                block_entity_states.get(&key).cloned(),
                                properties.resolve(state, block_class),
                                &mut color_cache,
                                &mut block_budget,
                                &mut connection_kinds,
                                &mut occupied,
                                &mut foliage,
                                &mut glass,
                                &mut water,
                                &mut lava,
                                &mut opaque_blocks,
                                &mut glass_blocks,
                                &mut detail_blocks,
                                &mut glass_detail_blocks,
                                &mut water_blocks,
                                &mut lava_blocks,
                                &mut min_x,
                                &mut max_x,
                                &mut min_y,
                                &mut max_y,
                                &mut min_z,
                                &mut max_z,
                            )?;
                        }
                    }

                    if primary_class
                        .is_none_or(|block_class| !block_class_is_renderable(block_class))
                        && fluid_state.is_none()
                    {
                        if let Some(state) =
                            visible_renderable_block_state_at(subchunk, local_x, local_y, local_z)
                        {
                            let block_class = block_class(&state.name);
                            push_collected_block(
                                key,
                                state,
                                block_class,
                                *biome.get_or_insert_with(|| {
                                    block_biome_sample(chunk, local_x, local_z, y, biome_blend)
                                }),
                                block_entity_states.get(&key).cloned(),
                                properties.resolve(state, block_class),
                                &mut color_cache,
                                &mut block_budget,
                                &mut connection_kinds,
                                &mut occupied,
                                &mut foliage,
                                &mut glass,
                                &mut water,
                                &mut lava,
                                &mut opaque_blocks,
                                &mut glass_blocks,
                                &mut detail_blocks,
                                &mut glass_detail_blocks,
                                &mut water_blocks,
                                &mut lava_blocks,
                                &mut min_x,
                                &mut max_x,
                                &mut min_y,
                                &mut max_y,
                                &mut min_z,
                                &mut max_z,
                            )?;
                        }
                    }
                }
            }
        }
    }

    let mut blocks = ChunkBlocks {
        ao_exclusions: FxHashSet::default(),
        occupied,
        foliage,
        glass,
        water,
        lava,
        waterlogged_water,
        waterlogged_lava,
        partial_fluid_heights: FxHashMap::default(),
        connection_kinds,
        stair_states: FxHashMap::default(),
        opaque_blocks,
        glass_blocks,
        detail_blocks,
        glass_detail_blocks,
        water_blocks,
        lava_blocks,
        min_y,
        max_y,
        min_x,
        max_x,
        min_z,
        max_z,
        internally_culled_blocks: 0,
    };
    blocks.rebuild_neighbor_indexes();
    blocks.internally_culled_blocks = filter_internal_block_records(&mut blocks);
    blocks.compact_storage();
    Ok(blocks)
}
