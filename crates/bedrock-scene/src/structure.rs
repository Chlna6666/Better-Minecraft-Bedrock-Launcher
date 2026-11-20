//! Scene construction from decoded Bedrock structures.

use super::*;

/// Builds a structure mesh at its chunk-aligned X/Z anchor and world Y origin.
///
/// # Errors
/// Returns an error for invalid bounds, structure indices, or mesh geometry.
pub fn build(
    structure: &bedrock_world::structure::McStructureFile,
    anchor_chunk: ChunkPos,
    origin_y: i32,
    render_palette: &RenderPalette,
) -> Result<SceneMesh, String> {
    let max_block_x = structure.size.x.saturating_sub(1);
    let max_block_z = structure.size.z.saturating_sub(1);
    let bounds = SlimeChunkBounds {
        dimension: anchor_chunk.dimension,
        min_chunk_x: anchor_chunk.x,
        max_chunk_x: anchor_chunk.x.saturating_add(max_block_x.div_euclid(16)),
        min_chunk_z: anchor_chunk.z,
        max_chunk_z: anchor_chunk.z.saturating_add(max_block_z.div_euclid(16)),
    };
    bounds.validate().map_err(|error| error.to_string())?;

    let palette = structure
        .palette
        .iter()
        .map(|entry| BlockState {
            name: entry.name.clone(),
            states: entry.states.clone(),
            version: entry.version,
        })
        .collect::<Vec<_>>();
    let blocks = structure
        .blocks()
        .map_err(|error| format!("结构方块索引无效：{error}"))?;
    let mut chunk_builders = FxHashMap::<ChunkKey, StructureChunkBuilder>::default();
    let origin_x = anchor_chunk.x.saturating_mul(16);
    let origin_z = anchor_chunk.z.saturating_mul(16);

    for block in blocks {
        let key = BlockKey {
            x: origin_x.saturating_add(block.x),
            y: origin_y.saturating_add(block.y),
            z: origin_z.saturating_add(block.z),
        };
        let chunk_key = ChunkKey::from_block(key);
        let builder = chunk_builders.entry(chunk_key).or_default();
        push_structure_block(
            key,
            structure_palette_state(&palette, block.primary),
            structure_palette_state(&palette, block.secondary),
            render_palette,
            builder,
        )?;
    }

    let mut builder = SceneBuilder::new(bounds, 0).with_neutral_colors();
    for chunk_position in chunk_positions(bounds) {
        let chunk_key = ChunkKey::from_pos(chunk_position);
        let mut blocks = chunk_builders
            .remove(&chunk_key)
            .map_or_else(ChunkBlocks::default, |chunk_builder| chunk_builder.blocks);
        blocks.internally_culled_blocks = filter_internal_block_records(&mut blocks);
        builder.insert_prepared(PreparedChunk {
            chunk_key,
            subchunk_count: structure_subchunk_count(&blocks),
            internally_culled_blocks: blocks.internally_culled_blocks,
            blocks: Some(blocks),
        });
    }

    builder.build_selection()
}
pub(super) struct StructureChunkBuilder {
    pub(super) blocks: ChunkBlocks,
    block_budget: BlockBudget,
}

impl Default for StructureChunkBuilder {
    fn default() -> Self {
        Self {
            blocks: ChunkBlocks::default(),
            block_budget: BlockBudget::unbounded(),
        }
    }
}

pub(super) fn structure_palette_state(palette: &[BlockState], index: i32) -> Option<&BlockState> {
    let index = usize::try_from(index).ok()?;
    palette.get(index)
}

pub(super) fn structure_subchunk_count(blocks: &ChunkBlocks) -> usize {
    let mut subchunks = FxHashSet::default();
    for block in &blocks.opaque_blocks {
        subchunks.insert(block.key.y.div_euclid(16));
    }
    for block in &blocks.glass_blocks {
        subchunks.insert(block.key.y.div_euclid(16));
    }
    for block in &blocks.detail_blocks {
        subchunks.insert(block.key.y.div_euclid(16));
    }
    for block in &blocks.glass_detail_blocks {
        subchunks.insert(block.key.y.div_euclid(16));
    }
    for block in &blocks.water_blocks {
        subchunks.insert(block.key.y.div_euclid(16));
    }
    for block in &blocks.lava_blocks {
        subchunks.insert(block.key.y.div_euclid(16));
    }
    subchunks.len()
}

pub(super) fn push_structure_block(
    key: BlockKey,
    primary: Option<&BlockState>,
    secondary: Option<&BlockState>,
    render_palette: &RenderPalette,
    builder: &mut StructureChunkBuilder,
) -> Result<(), String> {
    let primary_class = primary.map(|state| block_class(&state.name));
    if let Some((state, block_class)) = primary.zip(primary_class) {
        if block_class_is_renderable(block_class) {
            push_structure_state(key, state, block_class, render_palette, builder)?;
        }
    }

    let secondary_class = secondary.map(|state| block_class(&state.name));
    let mut pushed_secondary = false;
    if let Some((state, block_class)) = secondary.zip(secondary_class) {
        if matches!(block_class, BlockClass::Water | BlockClass::Lava)
            && primary_class != Some(block_class)
        {
            // Official waterlogging: the contained liquid renders as plain
            // full-square quads regardless of the co-located block's shape.
            // Record the cell for same-fluid face culling, like the world
            // collection path does.
            match block_class {
                BlockClass::Water => {
                    builder.blocks.waterlogged_water.insert(key);
                }
                BlockClass::Lava => {
                    builder.blocks.waterlogged_lava.insert(key);
                }
                _ => {}
            }
            push_structure_state(key, state, block_class, render_palette, builder)?;
            pushed_secondary = true;
        }
    }

    if primary_class.is_none_or(|block_class| !block_class_is_renderable(block_class))
        && !pushed_secondary
    {
        if let Some((state, block_class)) = secondary.zip(secondary_class) {
            if block_class_is_renderable(block_class) {
                push_structure_state(key, state, block_class, render_palette, builder)?;
            }
        }
    }
    Ok(())
}

pub(super) fn push_structure_state(
    key: BlockKey,
    state: &BlockState,
    block_class: BlockClass,
    render_palette: &RenderPalette,
    builder: &mut StructureChunkBuilder,
) -> Result<(), String> {
    push_collected_block(
        key,
        state,
        block_class,
        None,
        None,
        &BlockProperties::new(state, block_class, None),
        &mut BlockColorCache::new_neutral(render_palette),
        &mut builder.block_budget,
        &mut builder.blocks.connection_kinds,
        &mut builder.blocks.occupied,
        &mut builder.blocks.foliage,
        &mut builder.blocks.glass,
        &mut builder.blocks.water,
        &mut builder.blocks.lava,
        &mut builder.blocks.opaque_blocks,
        &mut builder.blocks.glass_blocks,
        &mut builder.blocks.detail_blocks,
        &mut builder.blocks.glass_detail_blocks,
        &mut builder.blocks.water_blocks,
        &mut builder.blocks.lava_blocks,
        &mut builder.blocks.min_x,
        &mut builder.blocks.max_x,
        &mut builder.blocks.min_y,
        &mut builder.blocks.max_y,
        &mut builder.blocks.min_z,
        &mut builder.blocks.max_z,
    )
}

pub(super) fn chunk_positions(bounds: SlimeChunkBounds) -> Vec<ChunkPos> {
    (bounds.min_chunk_z..=bounds.max_chunk_z)
        .flat_map(|z| {
            (bounds.min_chunk_x..=bounds.max_chunk_x).map(move |x| ChunkPos {
                x,
                z,
                dimension: bounds.dimension,
            })
        })
        .collect()
}
