use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn push_collected_block(
    key: BlockKey,
    state: &BlockState,
    block_class: BlockClass,
    biome: Option<BiomeSample>,
    block_entity_state: Option<BlockEntityModelState>,
    properties: &BlockProperties,
    colors: &mut BlockColorCache,
    block_budget: &mut BlockBudget,
    connection_kinds: &mut FxHashMap<BlockKey, ConnectionKind>,
    occupied: &mut FxHashSet<BlockKey>,
    foliage: &mut FxHashSet<BlockKey>,
    glass: &mut FxHashSet<BlockKey>,
    water: &mut FxHashSet<BlockKey>,
    lava: &mut FxHashSet<BlockKey>,
    opaque_blocks: &mut Vec<BlockRecord>,
    glass_blocks: &mut Vec<BlockRecord>,
    detail_blocks: &mut Vec<DetailBlock>,
    glass_detail_blocks: &mut Vec<DetailBlock>,
    water_blocks: &mut Vec<FluidBlock>,
    lava_blocks: &mut Vec<FluidBlock>,
    min_x: &mut i32,
    max_x: &mut i32,
    min_y: &mut i16,
    max_y: &mut i16,
    min_z: &mut i32,
    max_z: &mut i32,
) -> Result<(), String> {
    if !block_budget.try_take() {
        return Err("3D 预览单区块方块记录超出预算".to_string());
    }
    include_block_bounds(key, min_x, max_x, min_y, max_y, min_z, max_z);
    // A banner's dye and pattern layers live on the block entity, so its faces name a
    // per-variant material that the texture stage composites from `banner_base` + `banner.tga`.
    // The composited sheet already carries the dye, so the vertex colour stays white.
    let banner_art = block_entity_state
        .as_ref()
        .and_then(|state| state.banner.clone())
        .filter(|_| is_banner_block(&state.name));
    let material = banner_art.as_ref().map_or_else(
        || properties.material.clone(),
        |art| MaterialName::from(art.material_name()),
    );
    let normalized_name = properties.normalized_name.clone();
    let stair_state = properties.stair_state;
    if let Some(kind) = properties.connection_kind {
        connection_kinds.insert(key, kind);
    }
    let shape = properties.shape(state, block_entity_state.as_ref());
    let neighbor_model = properties.neighbor_model;
    // Colors depend only on the immutable palette entry and the biome sample;
    // one resolution per (entry, biome) replaces the per-block palette probes.
    let entry_colors = colors.resolve(state, biome);
    match block_class {
        BlockClass::Opaque => {
            if let Some(shape) = shape {
                if !shape.is_empty() {
                    detail_blocks.push(DetailBlock {
                        key,
                        normalized_name: normalized_name.clone(),
                        neighbor_model,
                        stair_state,
                        shape,
                        color: if banner_art.is_some() {
                            [1.0, 1.0, 1.0, 1.0]
                        } else {
                            entry_colors.flat
                        },
                        material,
                    });
                    return Ok(());
                }
            }
            occupied.insert(key);
            // Cutout foliage (leaves) is a full cube for opaque-face culling but must not
            // hide transparent or fluid faces behind its texture gaps.
            if is_foliage_block(properties.normalized_name.trim_start_matches("minecraft:")) {
                foliage.insert(key);
            }
            opaque_blocks.push(BlockRecord::new(key, entry_colors.face, material));
        }
        BlockClass::TransparentGlass => {
            if let Some(shape) = shape {
                if !shape.is_empty() {
                    glass_detail_blocks.push(DetailBlock {
                        key,
                        normalized_name: normalized_name.clone(),
                        neighbor_model,
                        stair_state,
                        shape,
                        color: entry_colors.transparent,
                        material,
                    });
                    return Ok(());
                }
            }
            glass.insert(key);
            glass_blocks.push(BlockRecord::uniform(
                key,
                entry_colors.transparent,
                material,
            ));
        }
        BlockClass::Water => {
            water.insert(key);
            water_blocks.push(FluidBlock {
                key,
                color: entry_colors.water,
                material,
                height: properties.fluid_height,
            });
        }
        BlockClass::Lava => {
            lava.insert(key);
            lava_blocks.push(FluidBlock {
                key,
                color: entry_colors.lava,
                material,
                height: properties.fluid_height,
            });
        }
        BlockClass::DetailOpaque | BlockClass::DetailGlass => {
            let Some(shape) = shape else {
                return Ok(());
            };
            if shape.is_empty() {
                return Ok(());
            }
            let color = if block_class == BlockClass::DetailGlass {
                entry_colors.transparent
            } else {
                entry_colors.flat
            };
            let block = DetailBlock {
                key,
                normalized_name,
                neighbor_model,
                stair_state,
                shape,
                color,
                material,
            };
            if block_class == BlockClass::DetailGlass {
                glass_detail_blocks.push(block);
            } else {
                detail_blocks.push(block);
            }
        }
        BlockClass::Air | BlockClass::SkipTransparent => {}
    }
    Ok(())
}

pub(super) fn visible_renderable_block_state_at(
    subchunk: &bedrock_world::SubChunk,
    local_x: u8,
    local_y: u8,
    local_z: u8,
) -> Option<&BlockState> {
    subchunk
        .visible_block_states_at(local_x, local_y, local_z)
        .find(|state| block_class_is_renderable(block_class(&state.name)))
}

pub(super) fn filter_internal_block_records(blocks: &mut ChunkBlocks) -> usize {
    let opaque_before = blocks.opaque_blocks.len();
    let glass_before = blocks.glass_blocks.len();
    let water_before = blocks.water_blocks.len();
    let lava_before = blocks.lava_blocks.len();

    let occupied = &blocks.occupied;
    blocks
        .opaque_blocks
        .retain(|block| !opaque_block_is_fully_hidden(block.key, occupied));

    let glass = &blocks.glass;
    let foliage = &blocks.foliage;
    // Same-fluid neighbours include waterlogged cells: a fluid record inside
    // a waterlogged cell must not be culled as "hidden by the block" when the
    // neighbouring cell carries the same liquid layer.
    let water_cells: FxHashSet<BlockKey> = blocks
        .water
        .union(&blocks.waterlogged_water)
        .copied()
        .collect();
    let lava_cells: FxHashSet<BlockKey> = blocks
        .lava
        .union(&blocks.waterlogged_lava)
        .copied()
        .collect();
    let water = &water_cells;
    let lava = &lava_cells;
    blocks.glass_blocks.retain(|block| {
        !transparent_block_is_fully_hidden(block.key, occupied, foliage, glass, water, lava)
    });

    let partial_heights = &blocks.partial_fluid_heights;
    blocks.water_blocks.retain(|block| {
        !transparent_block_is_fully_hidden(block.key, occupied, foliage, water, glass, lava)
            || has_exposed_liquid_side_above_neighbor(block, water, partial_heights)
    });

    blocks.lava_blocks.retain(|block| {
        !transparent_block_is_fully_hidden(block.key, occupied, foliage, lava, glass, water)
            || has_exposed_liquid_side_above_neighbor(block, lava, partial_heights)
    });

    opaque_before
        .saturating_sub(blocks.opaque_blocks.len())
        .saturating_add(glass_before.saturating_sub(blocks.glass_blocks.len()))
        .saturating_add(water_before.saturating_sub(blocks.water_blocks.len()))
        .saturating_add(lava_before.saturating_sub(blocks.lava_blocks.len()))
}

pub(super) fn opaque_block_is_fully_hidden(
    block: BlockKey,
    occupied: &FxHashSet<BlockKey>,
) -> bool {
    FACE_DEFINITIONS
        .iter()
        .all(|face| occupied.contains(&block.neighbor(*face)))
}

pub(super) fn transparent_block_is_fully_hidden(
    block: BlockKey,
    occupied: &FxHashSet<BlockKey>,
    foliage: &FxHashSet<BlockKey>,
    same_class: &FxHashSet<BlockKey>,
    other_transparent_a: &FxHashSet<BlockKey>,
    other_transparent_b: &FxHashSet<BlockKey>,
) -> bool {
    FACE_DEFINITIONS.iter().all(|face| {
        let neighbor = block.neighbor(*face);
        same_class.contains(&neighbor)
            || (occupied.contains(&neighbor)
                // Cutout foliage never hides a transparent face: the water or
                // glass stays visible through the leaf texture gaps.
                && !foliage.contains(&neighbor)
                && !other_transparent_a.contains(&neighbor)
                && !other_transparent_b.contains(&neighbor))
    })
}

/// A fluid fully surrounded by other fluid blocks can still expose a
/// vertical upper strip beside a lower fluid level. This is the only
/// expensive path for pre-culling: the common exterior-face path returns
/// before examining level differences.
pub(super) fn has_exposed_liquid_side_above_neighbor(
    block: &FluidBlock,
    same_fluid: &FxHashSet<BlockKey>,
    partial_heights: &FxHashMap<BlockKey, f32>,
) -> bool {
    FACE_DEFINITIONS
        .into_iter()
        .filter(|face| face.normal[1] == 0)
        .any(|face| {
            let neighbour = block.key.neighbor(face);
            same_fluid.contains(&neighbour)
                && partial_heights
                    .get(&neighbour)
                    // A missing entry is the official source surface height
                    // (level/9 = 8/9), not a full column.
                    .copied()
                    .unwrap_or(SOURCE_FLUID_HEIGHT)
                    < block.height - 0.0001
        })
}
pub(super) fn include_block_bounds(
    block: BlockKey,
    min_x: &mut i32,
    max_x: &mut i32,
    min_y: &mut i16,
    max_y: &mut i16,
    min_z: &mut i32,
    max_z: &mut i32,
) {
    *min_x = (*min_x).min(block.x);
    *max_x = (*max_x).max(block.x);
    if let Ok(y) = i16::try_from(block.y) {
        *min_y = (*min_y).min(y);
        *max_y = (*max_y).max(y);
    }
    *min_z = (*min_z).min(block.z);
    *max_z = (*max_z).max(block.z);
}

pub(super) struct BlockBudget {
    max_records: usize,
    used_records: usize,
}

impl BlockBudget {
    const fn new(max_records: usize) -> Self {
        Self {
            max_records,
            used_records: 0,
        }
    }

    pub(super) const fn unbounded() -> Self {
        Self::new(usize::MAX)
    }

    const fn is_full(&self) -> bool {
        self.used_records >= self.max_records
    }

    pub(super) fn try_take(&mut self) -> bool {
        if self.is_full() {
            return false;
        }
        self.used_records = self.used_records.saturating_add(1);
        true
    }
}
