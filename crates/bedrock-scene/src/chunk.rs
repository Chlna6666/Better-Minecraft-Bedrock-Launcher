//! Collected chunk data and compact neighbor boundaries.

use super::*;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct ChunkKey {
    pub(super) x: i32,
    pub(super) z: i32,
}

impl ChunkKey {
    pub(super) const fn from_pos(pos: ChunkPos) -> Self {
        Self { x: pos.x, z: pos.z }
    }

    pub(super) fn from_block(block: BlockKey) -> Self {
        Self {
            x: block.x.div_euclid(16),
            z: block.z.div_euclid(16),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct BlockKey {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) z: i32,
}

impl BlockKey {
    pub(super) const fn neighbor(self, face: FaceDefinition) -> Self {
        Self {
            x: self.x + face.normal[0],
            y: self.y + face.normal[1],
            z: self.z + face.normal[2],
        }
    }

    pub(super) const fn cardinal_neighbor(self, direction: CardinalDirection) -> Self {
        let [x, y, z] = direction.normal();
        Self {
            x: self.x + x,
            y: self.y + y,
            z: self.z + z,
        }
    }

    pub(super) const fn above(self) -> Self {
        Self {
            x: self.x,
            y: self.y + 1,
            z: self.z,
        }
    }

    pub(super) const fn below(self) -> Self {
        Self {
            x: self.x,
            y: self.y - 1,
            z: self.z,
        }
    }
}

/// Renderable cells and occlusion data collected from one Bedrock chunk.
pub struct ChunkBlocks {
    pub(super) ao_exclusions: FxHashSet<BlockKey>,
    pub(super) occupied: FxHashSet<BlockKey>,
    /// Opaque-classified cutout foliage (leaves). Full cubes for opaque-face
    /// culling, but transparent/fluid faces stay visible behind their gaps.
    pub(super) foliage: FxHashSet<BlockKey>,
    pub(super) glass: FxHashSet<BlockKey>,
    pub(super) water: FxHashSet<BlockKey>,
    pub(super) lava: FxHashSet<BlockKey>,
    /// Cells whose primary block is not the fluid itself but which carry a
    /// liquid layer (waterlogging). Same-fluid face culling must treat them
    /// as fluid cells, otherwise every adjacent waterlogged pair draws two
    /// coplanar walls that blend through the neighbouring block faces.
    pub(super) waterlogged_water: FxHashSet<BlockKey>,
    pub(super) waterlogged_lava: FxHashSet<BlockKey>,
    // Only fluid levels other than the source height need storage: a missing
    // entry means the official source surface height (8/9). Shared between
    // adjacent faces and mesh waves.
    pub(super) partial_fluid_heights: FxHashMap<BlockKey, f32>,
    pub(super) connection_kinds: FxHashMap<BlockKey, ConnectionKind>,
    pub(super) stair_states: FxHashMap<BlockKey, StairState>,
    pub(super) opaque_blocks: Vec<BlockRecord>,
    pub(super) glass_blocks: Vec<BlockRecord>,
    pub(super) detail_blocks: Vec<DetailBlock>,
    pub(super) glass_detail_blocks: Vec<DetailBlock>,
    pub(super) water_blocks: Vec<FluidBlock>,
    pub(super) lava_blocks: Vec<FluidBlock>,
    pub(super) min_y: i16,
    pub(super) max_y: i16,
    pub(super) min_x: i32,
    pub(super) max_x: i32,
    pub(super) min_z: i32,
    pub(super) max_z: i32,
    pub(super) internally_culled_blocks: usize,
}

/// Neighbor-only data retained after the full column has been released.
/// Compact edge data for building adjoining chunk geometry.
pub struct ChunkBoundary {
    pub(super) classes: FxHashMap<i32, Box<[u8; 2048]>>,
    // Neighbor-only summaries store partial levels at the 16x16 boundary,
    // so different-height fluid seams remain correct after block eviction.
    pub(super) partial_fluid_heights: FxHashMap<BlockKey, f32>,
    // Cutout foliage cells on the chunk edge: fluid and glass faces stay
    // visible behind them instead of being culled as opaque neighbors.
    pub(super) foliage: FxHashSet<BlockKey>,
    // Waterlogged cells on the chunk edge: adjacent fluid faces must treat
    // them as same-fluid neighbours, exactly like the primary-fluid sets.
    pub(super) waterlogged_water: FxHashSet<BlockKey>,
    pub(super) waterlogged_lava: FxHashSet<BlockKey>,
    pub(super) connection_kinds: FxHashMap<BlockKey, ConnectionKind>,
    pub(super) stair_states: FxHashMap<BlockKey, StairState>,
    pub(super) coarse_opaque: FxHashSet<BlockKey>,
    pub(super) coarse_glass: FxHashSet<BlockKey>,
    pub(super) coarse_water: FxHashSet<BlockKey>,
    pub(super) coarse_lava: FxHashSet<BlockKey>,
}

impl ChunkBoundary {
    pub(super) fn class_at(&self, block: BlockKey) -> Option<BlockClass> {
        let index = (block.y.rem_euclid(16) * 256
            + block.z.rem_euclid(16) * 16
            + block.x.rem_euclid(16)) as usize;
        let packed = self
            .classes
            .get(&block.y.div_euclid(16))?
            .get(index / 2)
            .copied()?;
        match (packed >> ((index % 2) * 4)) & 0x07 {
            1 => Some(BlockClass::Opaque),
            2 => Some(BlockClass::TransparentGlass),
            3 => Some(BlockClass::Water),
            4 => Some(BlockClass::Lava),
            _ => None,
        }
    }

    /// Returns the approximate size of the retained CPU data.
    pub fn estimated_cpu_bytes(&self) -> usize {
        let keys = self.coarse_opaque.len()
            + self.coarse_glass.len()
            + self.coarse_water.len()
            + self.coarse_lava.len()
            + self.waterlogged_water.len()
            + self.waterlogged_lava.len()
            + self.foliage.len();
        std::mem::size_of::<Self>()
            .saturating_add(self.classes.len().saturating_mul(2048))
            .saturating_add(
                self.partial_fluid_heights
                    .len()
                    .saturating_mul(std::mem::size_of::<(BlockKey, f32)>() * 2),
            )
            .saturating_add(keys.saturating_mul(std::mem::size_of::<BlockKey>() * 2))
            .saturating_add(
                self.connection_kinds
                    .len()
                    .saturating_mul(std::mem::size_of::<(BlockKey, ConnectionKind)>() * 2),
            )
            .saturating_add(
                self.stair_states
                    .len()
                    .saturating_mul(std::mem::size_of::<(BlockKey, StairState)>() * 2),
            )
    }
}

#[derive(Clone, Debug)]
pub(super) struct FluidBlock {
    pub(super) key: BlockKey,
    pub(super) color: [f32; 4],
    pub(super) material: MaterialName,
    pub(super) height: f32,
}

impl Default for ChunkBlocks {
    fn default() -> Self {
        Self {
            ao_exclusions: FxHashSet::default(),
            occupied: FxHashSet::default(),
            foliage: FxHashSet::default(),
            glass: FxHashSet::default(),
            water: FxHashSet::default(),
            lava: FxHashSet::default(),
            waterlogged_water: FxHashSet::default(),
            waterlogged_lava: FxHashSet::default(),
            partial_fluid_heights: FxHashMap::default(),
            connection_kinds: FxHashMap::default(),
            stair_states: FxHashMap::default(),
            opaque_blocks: Vec::new(),
            glass_blocks: Vec::new(),
            detail_blocks: Vec::new(),
            glass_detail_blocks: Vec::new(),
            water_blocks: Vec::new(),
            lava_blocks: Vec::new(),
            min_y: i16::MAX,
            max_y: i16::MIN,
            min_x: i32::MAX,
            max_x: i32::MIN,
            min_z: i32::MAX,
            max_z: i32::MIN,
            internally_culled_blocks: 0,
        }
    }
}

#[cfg(test)]
mod portal_mask_tests {
    use super::*;

    #[test]
    pub(super) fn visibility_mask_excludes_cutout_cubes_and_handles_negative_sections() {
        let mut blocks = ChunkBlocks::test_cubes(&[[-1, -1, -1], [-2, -1, -1]]);
        blocks.opaque_blocks[1].material = Arc::from("minecraft_oak_leaves");
        blocks.rebuild_neighbor_indexes();
        let masks = blocks.visibility_solid_masks();
        let last_cell = 15 * 256 + 15 * 16 + 15;
        let leaves_cell = 15 * 256 + 15 * 16 + 14;
        let section = &masks[&-1];
        assert_ne!(section[last_cell / 64] & (1 << (last_cell % 64)), 0);
        assert_eq!(section[leaves_cell / 64] & (1 << (leaves_cell % 64)), 0);
    }
}

impl ChunkBlocks {
    /// Full opaque cube occupancy for 16^3 Section visibility propagation.
    /// Unlike per-Section render records, `occupied` retains internal blocks
    /// removed by meshing, so a solid wall is never mistaken for an air tunnel.
    /// Detail geometry, glass, water and model-dependent opacity are NOT
    /// certified blockers; letting those cells pass is conservative.
    /// AO exclusions are based on is_full_opaque_block and also exclude
    /// cutout leaves/alpha-tested cube materials, not merely missing geometry.
    pub fn visibility_solid_masks(&self) -> BTreeMap<i8, [u64; 64]> {
        let mut masks = BTreeMap::<i8, [u64; 64]>::new();
        for key in &self.occupied {
            if self.ao_exclusions.contains(key) {
                continue;
            }
            let Ok(section_y) = i8::try_from(key.y.div_euclid(16)) else {
                continue;
            };
            let index = (key.y.rem_euclid(16) * 256
                + key.z.rem_euclid(16) * 16
                + key.x.rem_euclid(16)) as usize;
            masks.entry(section_y).or_insert([0; 64])[index / 64] |= 1u64 << (index % 64);
        }
        masks
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Creates synthetic solid stone cells for scene tests.
    pub fn test_cubes(positions: &[[i32; 3]]) -> Self {
        let mut blocks = Self::default();
        for &[x, y, z] in positions {
            let key = BlockKey { x, y, z };
            blocks.occupied.insert(key);
            blocks.opaque_blocks.push(BlockRecord::uniform(
                key,
                [1.0; 4],
                Arc::from("minecraft_stone"),
            ));
            include_block_bounds(
                key,
                &mut blocks.min_x,
                &mut blocks.max_x,
                &mut blocks.min_y,
                &mut blocks.max_y,
                &mut blocks.min_z,
                &mut blocks.max_z,
            );
        }
        blocks
    }

    /// Whether newly discovered blocks can change a directly adjacent column's faces.
    pub fn touches_neighbor(&self, pos: ChunkPos, neighbor: ChunkPos) -> bool {
        if self.min_x > self.max_x || self.min_z > self.max_z {
            return false;
        }
        let x = i64::from(pos.x) * 16;
        let z = i64::from(pos.z) * 16;
        match (
            i64::from(neighbor.x) - i64::from(pos.x),
            i64::from(neighbor.z) - i64::from(pos.z),
        ) {
            (-1, 0) => i64::from(self.min_x) <= x,
            (1, 0) => i64::from(self.max_x) >= x + 15,
            (0, -1) => i64::from(self.min_z) <= z,
            (0, 1) => i64::from(self.max_z) >= z + 15,
            (-1, -1) => i64::from(self.min_x) <= x && i64::from(self.min_z) <= z,
            (-1, 1) => i64::from(self.min_x) <= x && i64::from(self.max_z) >= z + 15,
            (1, -1) => i64::from(self.max_x) >= x + 15 && i64::from(self.min_z) <= z,
            (1, 1) => i64::from(self.max_x) >= x + 15 && i64::from(self.max_z) >= z + 15,
            _ => true,
        }
    }

    pub(super) fn compact_storage(&mut self) {
        fn compact_vec<T>(values: &mut Vec<T>) {
            if values.capacity() > values.len().saturating_mul(2).max(256) {
                values.shrink_to_fit();
            }
        }
        fn compact_set<T: Eq + std::hash::Hash>(values: &mut FxHashSet<T>) {
            if values.capacity() > values.len().saturating_mul(2).max(256) {
                values.shrink_to_fit();
            }
        }
        compact_set(&mut self.occupied);
        compact_set(&mut self.ao_exclusions);
        compact_set(&mut self.foliage);
        compact_set(&mut self.glass);
        compact_set(&mut self.water);
        compact_set(&mut self.lava);
        compact_vec(&mut self.opaque_blocks);
        compact_vec(&mut self.glass_blocks);
        compact_vec(&mut self.detail_blocks);
        compact_vec(&mut self.glass_detail_blocks);
        compact_vec(&mut self.water_blocks);
        compact_vec(&mut self.lava_blocks);
    }

    pub fn boundary_snapshot(&mut self) -> ChunkBoundary {
        self.rebuild_neighbor_indexes();
        let mut classes = FxHashMap::<i32, Box<[u8; 2048]>>::default();
        for (keys, class) in [
            (&self.lava, 4),
            (&self.water, 3),
            (&self.glass, 2),
            (&self.occupied, 1),
        ] {
            for key in keys {
                let index = (key.y.rem_euclid(16) * 256
                    + key.z.rem_euclid(16) * 16
                    + key.x.rem_euclid(16)) as usize;
                let packed = &mut classes
                    .entry(key.y.div_euclid(16))
                    .or_insert_with(|| Box::new([0; 2048]))[index / 2];
                let shift = (index % 2) * 4;
                let ao = u8::from(class == 1 && !self.ao_exclusions.contains(key)) << 3;
                *packed = (*packed & !(0x0f << shift)) | ((class | ao) << shift);
            }
        }
        let coarse_key = |key: BlockKey| BlockKey {
            x: key.x.div_euclid(4),
            y: key.y.div_euclid(4),
            z: key.z.div_euclid(4),
        };
        ChunkBoundary {
            classes,
            // The builder only asks a different chunk for immediate neighbor
            // cells, so discard every interior partial height in its halo.
            partial_fluid_heights: self
                .partial_fluid_heights
                .iter()
                .filter(|(key, _)| {
                    let x = key.x.rem_euclid(16);
                    let z = key.z.rem_euclid(16);
                    x == 0 || x == 15 || z == 0 || z == 15
                })
                .map(|(&key, &height)| (key, height))
                .collect(),
            foliage: self
                .foliage
                .iter()
                .filter(|key| {
                    let x = key.x.rem_euclid(16);
                    let z = key.z.rem_euclid(16);
                    x == 0 || x == 15 || z == 0 || z == 15
                })
                .copied()
                .collect(),
            waterlogged_water: self
                .waterlogged_water
                .iter()
                .filter(|key| {
                    let x = key.x.rem_euclid(16);
                    let z = key.z.rem_euclid(16);
                    x == 0 || x == 15 || z == 0 || z == 15
                })
                .copied()
                .collect(),
            waterlogged_lava: self
                .waterlogged_lava
                .iter()
                .filter(|key| {
                    let x = key.x.rem_euclid(16);
                    let z = key.z.rem_euclid(16);
                    x == 0 || x == 15 || z == 0 || z == 15
                })
                .copied()
                .collect(),
            connection_kinds: self.connection_kinds.clone(),
            stair_states: self.stair_states.clone(),
            coarse_opaque: self
                .opaque_blocks
                .iter()
                .map(|block| coarse_key(block.key))
                .collect(),
            coarse_glass: self
                .glass_blocks
                .iter()
                .map(|block| coarse_key(block.key))
                .collect(),
            coarse_water: self
                .water_blocks
                .iter()
                .map(|block| coarse_key(block.key))
                .collect(),
            coarse_lava: self
                .lava_blocks
                .iter()
                .map(|block| coarse_key(block.key))
                .collect(),
        }
    }

    pub(super) fn rebuild_neighbor_indexes(&mut self) {
        self.refresh_ambient_exclusions();
        self.stair_states.clear();
        // Preserve heights captured before internal-liquid pruning: a
        // culled low-level cell still influences its visible neighbour's
        // exposed upper strip. Chunk collections are immutable after their
        // decode/filter stage; edits build a new chunk snapshot.
        // Ordinary source-level fluid has no entry: a missing neighbour
        // entry means the official source surface height (level/9 = 8/9).
        for block in self.water_blocks.iter().chain(self.lava_blocks.iter()) {
            if (block.height - SOURCE_FLUID_HEIGHT).abs() > 1e-3 {
                self.partial_fluid_heights.insert(block.key, block.height);
            }
        }
        for block in self
            .detail_blocks
            .iter()
            .chain(self.glass_detail_blocks.iter())
        {
            if let Some(state) = block.stair_state {
                self.stair_states.insert(block.key, state);
            }
        }
    }

    pub(super) fn class_at(&self, block: BlockKey) -> Option<BlockClass> {
        if self.occupied.contains(&block) {
            return Some(BlockClass::Opaque);
        }
        if self.glass.contains(&block) {
            return Some(BlockClass::TransparentGlass);
        }
        if self.water.contains(&block) {
            return Some(BlockClass::Water);
        }
        if self.lava.contains(&block) {
            return Some(BlockClass::Lava);
        }
        None
    }

    pub(super) fn connection_kind_at(&self, block: BlockKey) -> Option<ConnectionKind> {
        self.connection_kinds.get(&block).copied()
    }

    pub(super) fn stair_state_at(&self, block: BlockKey) -> Option<StairState> {
        self.stair_states.get(&block).copied()
    }
}
impl ChunkBlocks {
    /// Approximate retained CPU bytes for this chunk block set.
    pub fn estimated_cpu_bytes(&self) -> usize {
        let records = |len: usize| len.saturating_mul(std::mem::size_of::<BlockRecord>());
        let details = |capacity: usize, len: usize| {
            capacity
                .saturating_mul(std::mem::size_of::<DetailBlock>())
                .saturating_add(len.saturating_mul(64))
        };
        let fluids = |len: usize| len.saturating_mul(std::mem::size_of::<FluidBlock>());
        std::mem::size_of::<Self>()
            .saturating_add(
                self.occupied
                    .capacity()
                    .saturating_mul(std::mem::size_of::<BlockKey>() * 2),
            )
            .saturating_add(
                self.ao_exclusions
                    .capacity()
                    .saturating_mul(std::mem::size_of::<BlockKey>() * 2),
            )
            .saturating_add(
                self.glass
                    .capacity()
                    .saturating_mul(std::mem::size_of::<BlockKey>() * 2),
            )
            .saturating_add(
                (self.water.capacity() + self.lava.capacity())
                    .saturating_mul(std::mem::size_of::<BlockKey>() * 2),
            )
            .saturating_add(records(self.opaque_blocks.capacity()))
            .saturating_add(records(self.glass_blocks.capacity()))
            .saturating_add(details(
                self.detail_blocks.capacity(),
                self.detail_blocks.len(),
            ))
            .saturating_add(details(
                self.glass_detail_blocks.capacity(),
                self.glass_detail_blocks.len(),
            ))
            .saturating_add(fluids(self.water_blocks.capacity()))
            .saturating_add(fluids(self.lava_blocks.capacity()))
            .saturating_add(
                self.partial_fluid_heights
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(BlockKey, f32)>() * 2),
            )
    }
}

pub use super::builder::{PreparedChunk, prepare};
pub use super::collect::collect;
