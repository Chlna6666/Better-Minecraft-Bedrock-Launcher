use super::*;

impl SceneBuilder {
    pub(super) fn is_glass_detail_face_hidden(&self, face: &Face) -> bool {
        // Glass detail faces are transparent; cull when the neighbor cell is opaque OR a solid
        // glass block. JE pane templates mark their end faces `cullface`, so a pane touching a
        // full glass block drops the 2 px rim end instead of drawing a stub against the glass.
        // A pane neighbour is not a full cube, but two joined panes continue the same wall —
        // the end caps between them are internal, so they cull too (vanilla merges the run).
        let mut min = [f64::INFINITY; 3];
        for corner in &face.corners {
            for (axis, value) in corner.iter().enumerate() {
                min[axis] = min[axis].min(*value);
            }
        }
        let normal = face.normal;
        let block = BlockKey {
            x: (min[0] - f64::from(normal[0]) * 0.5).floor() as i32,
            y: (min[1] - f64::from(normal[1]) * 0.5).floor() as i32,
            z: (min[2] - f64::from(normal[2]) * 0.5).floor() as i32,
        };
        let neighbor = BlockKey {
            x: block.x + normal[0],
            y: block.y + normal[1],
            z: block.z + normal[2],
        };
        matches!(
            self.block_class_at(neighbor),
            Some(BlockClass::TransparentGlass)
        ) || self.connection_kind_at(neighbor) == Some(ConnectionKind::Pane)
            || self.opaque_neighbor_hides_transparent(neighbor)
    }

    /// Whether a full opaque-classified neighbor really hides a transparent
    /// face. Cutout foliage (leaves) is full-cube for cube culling but its
    /// texture gaps leave the face behind visible, so transparent geometry
    /// keeps it (vanilla renders transparent faces against leaves).
    pub(super) fn opaque_neighbor_hides_transparent(&self, neighbor: BlockKey) -> bool {
        self.block_class_at(neighbor) == Some(BlockClass::Opaque) && !self.foliage_at(neighbor)
    }

    pub(super) fn is_opaque_neighbor(&self, block: BlockKey, face: FaceDefinition) -> bool {
        let neighbor = block.neighbor(face);
        self.block_class_at(neighbor) == Some(BlockClass::Opaque)
    }

    pub(super) fn is_glass_neighbor(&self, block: BlockKey, face: FaceDefinition) -> bool {
        let neighbor = block.neighbor(face);
        matches!(
            self.block_class_at(neighbor),
            Some(BlockClass::TransparentGlass)
        ) || self.opaque_neighbor_hides_transparent(neighbor)
    }

    /// Whether a cell holds the given liquid, either as the primary block or
    /// as a waterlogging layer. Adjacent cells carrying the same liquid form
    /// one body: the walls between them must not render (official rule — the
    /// vanilla fluid renderer checks the neighbour's `FluidState`, not its
    /// primary block).
    pub(super) fn fluid_cell_has(&self, key: BlockKey, fluid: BlockClass) -> bool {
        if self.block_class_at(key) == Some(fluid) {
            return true;
        }
        let key_chunk = ChunkKey::from_block(key);
        if let Some(chunk) = self.block_chunks.get(&key_chunk) {
            match fluid {
                BlockClass::Water => {
                    return chunk.waterlogged_water.contains(&key);
                }
                BlockClass::Lava => {
                    return chunk.waterlogged_lava.contains(&key);
                }
                _ => return false,
            }
        }
        self.boundary_chunks.get(&key_chunk).is_some_and(|chunk| {
            match fluid {
                BlockClass::Water => chunk.waterlogged_water.contains(&key),
                BlockClass::Lava => chunk.waterlogged_lava.contains(&key),
                _ => false,
            }
        })
    }

    /// Official surface height of a fluid cell (minecraft.wiki/w/Water): the
    /// state height (level/9), raised to a full block when the cell above
    /// holds the same fluid.
    pub(super) fn fluid_surface_height(&self, key: BlockKey, raw: f32, fluid: BlockClass) -> f32 {
        if self.fluid_cell_has(key.above(), fluid) {
            1.0
        } else {
            raw
        }
    }

    /// Surface height of a neighbouring fluid cell: its stored state height
    /// (missing = source height) raised to a full block under the same rule.
    fn neighbour_fluid_surface(&self, neighbour: BlockKey, fluid: BlockClass) -> f32 {
        let key = ChunkKey::from_block(neighbour);
        let height = self
            .block_chunks
            .get(&key)
            .and_then(|chunk| chunk.partial_fluid_heights.get(&neighbour))
            .or_else(|| {
                self.boundary_chunks
                    .get(&key)
                    .and_then(|chunk| chunk.partial_fluid_heights.get(&neighbour))
            })
            .copied()
            .unwrap_or(SOURCE_FLUID_HEIGHT);
        self.fluid_surface_height(neighbour, height, fluid)
    }
    // `neighbour_fluid_surface`'s above-cell check runs through
    // `fluid_cell_has`, so a waterlogged cell above raises its neighbour's
    // wall to the full height exactly like a plain fluid cell would.

    /// Fraction of a fluid side actually exposed to air or a lower-level
    /// liquid. A same-fluid neighbour does not hide the whole vertical quad
    /// when its surface is lower. No full block-state lookup is performed.
    pub(super) fn exposed_fluid_face_bottom(
        &self,
        block: &FluidBlock,
        face: FaceDefinition,
        fluid: BlockClass,
    ) -> Option<f32> {
        let neighbour = block.key.neighbor(face);
        // A horizontal face at a not-yet-loaded Chunk seam has no proof of
        // adjacent air. Defer its liquid wall rather than producing a full
        // height blue curtain against unknown world data. The normal neighbor
        // reconciliation rebuilds this face after the adjacent Chunk arrives.
        let neighbour_column = ChunkKey::from_block(neighbour);
        if face.normal[1] == 0
            && neighbour_column != ChunkKey::from_block(block.key)
            && !self.block_chunks.contains_key(&neighbour_column)
            && !self.boundary_chunks.contains_key(&neighbour_column)
        {
            return None;
        }
        let surface = self.fluid_surface_height(block.key, block.height, fluid);
        // Same fluid wins over every occlusion class: a neighbouring cell that
        // carries the same liquid (waterlogging included) is part of this body.
        if self.fluid_cell_has(neighbour, fluid) {
            if face.normal[1] != 0 {
                return None;
            }
            let height = self.neighbour_fluid_surface(neighbour, fluid);
            return (surface > height + 0.0001).then_some(height);
        }
        match self.block_class_at(neighbour) {
            // Cutout foliage keeps the fluid face: the water must stay
            // visible through the leaf texture gaps (vanilla draws it).
            Some(BlockClass::Opaque) if !self.foliage_at(neighbour) => None,
            _ => Some(0.0),
        }
    }

    pub(super) fn block_class_at(&self, block: BlockKey) -> Option<BlockClass> {
        if self.is_unprocessed_selected_neighbor(block) {
            return Some(BlockClass::Opaque);
        }
        let key = ChunkKey::from_block(block);
        self.block_chunks
            .get(&key)
            .and_then(|chunk| chunk.class_at(block))
            .or_else(|| {
                self.boundary_chunks
                    .get(&key)
                    .and_then(|chunk| chunk.class_at(block))
            })
    }

    /// Whether the cell holds opaque-classified cutout foliage. Such neighbors
    /// occlude cube faces but must not hide fluid, glass or detail faces,
    /// whose texture gaps leave the geometry behind visible.
    pub(super) fn foliage_at(&self, block: BlockKey) -> bool {
        let key = ChunkKey::from_block(block);
        if let Some(chunk) = self.block_chunks.get(&key) {
            return chunk.foliage.contains(&block);
        }
        self.boundary_chunks
            .get(&key)
            .is_some_and(|chunk| chunk.foliage.contains(&block))
    }

    pub(super) fn inferred_detail_shape(&self, block: &DetailBlock) -> Option<DetailShape> {
        self.detail_shape_query(block)
            .and_then(|query| model_family::resolve_shape(&query))
            .map(detail_shape_from_model)
    }

    pub(super) fn detail_shape_query(&self, block: &DetailBlock) -> Option<BlockStateQuery> {
        if block.neighbor_model == NeighborModel::None {
            return None;
        }
        let block_name = if block.normalized_name.starts_with("minecraft:") {
            block.normalized_name.to_string()
        } else {
            format!("minecraft:{}", block.normalized_name)
        };
        let mut query = BlockStateQuery::new(block_name);
        match block.neighbor_model {
            NeighborModel::None => return None,
            NeighborModel::Pane => {
                for direction in CardinalDirection::ALL {
                    query = query.with_state(
                        direction.state_key(),
                        self.pane_neighbor_connects(block.key, direction),
                    );
                }
            }
            NeighborModel::Tripwire => {
                // A wire joins a neighbouring wire or a hook; the four booleans drive the
                // multipart geometry (the wire only runs along connected axes).
                for direction in CardinalDirection::ALL {
                    let kind = self.connection_kind_at(block.key.cardinal_neighbor(direction));
                    query = query.with_state(
                        direction.state_key(),
                        matches!(kind, Some(ConnectionKind::Tripwire | ConnectionKind::TripwireHook)),
                    );
                }
            }
            NeighborModel::Fence => {
                for direction in CardinalDirection::ALL {
                    query = query.with_state(
                        direction.state_key(),
                        self.fence_neighbor_connects(block.key, direction),
                    );
                }
            }
            NeighborModel::Wall => {
                let mut connected = Vec::with_capacity(4);
                for direction in CardinalDirection::ALL {
                    let connects = self.wall_neighbor_connects(block.key, direction);
                    if connects {
                        connected.push(direction);
                    }
                    query = query
                        .with_state(direction.state_key(), if connects { "low" } else { "none" });
                }
                let straight = connected.len() == 2
                    && connected[0].opposite() == connected[1]
                    && self.block_class_at(block.key.above()) != Some(BlockClass::Opaque);
                query = query.with_state("up", !straight);
            }
            NeighborModel::RedstoneWire { power } => {
                query = query.with_state("redstone_signal", i32::from(power));
                query = query.with_state("power", i32::from(power));
                for direction in CardinalDirection::ALL {
                    query = query.with_state(
                        direction.state_key(),
                        self.redstone_connection(block.key, direction),
                    );
                }
            }
            NeighborModel::Stairs => {
                let current = block.stair_state?;
                query = query
                    .with_state("facing", current.facing.as_str())
                    .with_state("vertical_half", if current.top { "top" } else { "bottom" })
                    .with_state("half", if current.top { "top" } else { "bottom" })
                    .with_state("shape", self.stair_shape(block.key, current));
            }
            NeighborModel::PistonBase(facing) => {
                // Modern Bedrock states carry no `extended_bit`; an extended base is the
                // one with the arm block in the facing direction. Without it the cached
                // palette shape (the retracted platform cube) is already correct, so no
                // query is returned and the caller keeps `block.shape`.
                let neighbor = BlockKey {
                    x: block.key.x + facing.normal[0],
                    y: block.key.y + facing.normal[1],
                    z: block.key.z + facing.normal[2],
                };
                if self.connection_kind_at(neighbor) != Some(ConnectionKind::PistonArm) {
                    return None;
                }
                query = query
                    .with_state("minecraft:facing_direction", facing.direction)
                    .with_state("extended_bit", true);
            }
            NeighborModel::DoorUpper(direction) => {
                // The upper half's own hinge/open storage is unreliable (a right hinge is
                // not encodable, `open_bit` degenerate), so the placement mirrors the
                // functional lower half below. A missing/mismatched half below degrades
                // to the closed hinge-left door.
                let (hinge, open) = match self.connection_kind_at(block.key.below()) {
                    Some(ConnectionKind::DoorLower { hinge, open }) => (hinge, open),
                    _ => (false, false),
                };
                query = query
                    .with_state("minecraft:cardinal_direction", direction.as_str())
                    .with_state("open_bit", open)
                    .with_state("door_hinge_bit", hinge)
                    .with_state("upper_block_bit", true);
            }
        }
        Some(query)
    }

    pub(super) fn connection_kind_at(&self, block: BlockKey) -> Option<ConnectionKind> {
        let key = ChunkKey::from_block(block);
        self.block_chunks
            .get(&key)
            .and_then(|chunk| chunk.connection_kind_at(block))
            .or_else(|| {
                self.boundary_chunks
                    .get(&key)
                    .and_then(|chunk| chunk.connection_kinds.get(&block).copied())
            })
    }

    pub(super) fn pane_neighbor_connects(
        &self,
        block: BlockKey,
        direction: CardinalDirection,
    ) -> bool {
        let neighbor = block.cardinal_neighbor(direction);
        matches!(
            self.block_class_at(neighbor),
            Some(BlockClass::Opaque | BlockClass::TransparentGlass)
        ) || self.connection_kind_at(neighbor) == Some(ConnectionKind::Pane)
    }

    pub(super) fn fence_neighbor_connects(
        &self,
        block: BlockKey,
        direction: CardinalDirection,
    ) -> bool {
        let neighbor = block.cardinal_neighbor(direction);
        self.block_class_at(neighbor) == Some(BlockClass::Opaque)
            || matches!(
                self.connection_kind_at(neighbor),
                Some(ConnectionKind::Fence | ConnectionKind::FenceGate | ConnectionKind::Wall)
            )
    }

    pub(super) fn wall_neighbor_connects(
        &self,
        block: BlockKey,
        direction: CardinalDirection,
    ) -> bool {
        let neighbor = block.cardinal_neighbor(direction);
        self.block_class_at(neighbor) == Some(BlockClass::Opaque)
            || matches!(
                self.connection_kind_at(neighbor),
                Some(ConnectionKind::Wall | ConnectionKind::FenceGate)
            )
    }

    pub(super) fn redstone_connection(
        &self,
        block: BlockKey,
        direction: CardinalDirection,
    ) -> &'static str {
        let neighbor = block.cardinal_neighbor(direction);
        if self.redstone_component_connects(neighbor, direction) {
            return "side";
        }
        if self.block_class_at(neighbor) == Some(BlockClass::Opaque)
            && self.block_class_at(block.above()) != Some(BlockClass::Opaque)
            && self.connection_kind_at(neighbor.above()) == Some(ConnectionKind::RedstoneWire)
        {
            return "up";
        }
        if self.block_class_at(neighbor) != Some(BlockClass::Opaque)
            && self.connection_kind_at(neighbor.below()) == Some(ConnectionKind::RedstoneWire)
        {
            return "side";
        }
        "none"
    }

    pub(super) fn redstone_component_connects(
        &self,
        block: BlockKey,
        direction: CardinalDirection,
    ) -> bool {
        match self.connection_kind_at(block) {
            // The piston arm keeps its old redstone-connectable behaviour even though it
            // now carries its own connection kind.
            Some(ConnectionKind::RedstoneWire | ConnectionKind::RedstoneAny | ConnectionKind::PistonArm) => true,
            Some(ConnectionKind::RedstoneAxisX) => {
                matches!(direction, CardinalDirection::East | CardinalDirection::West)
            }
            Some(ConnectionKind::RedstoneAxisZ) => {
                matches!(
                    direction,
                    CardinalDirection::North | CardinalDirection::South
                )
            }
            _ => false,
        }
    }

    pub(super) fn stair_shape(&self, block: BlockKey, current: StairState) -> &'static str {
        let front = block.cardinal_neighbor(current.facing);
        if let Some(neighbor) = self.stair_state_at(front)
            && neighbor.top == current.top
            && current.facing.is_perpendicular(neighbor.facing)
            && self.stair_can_take_shape(block, current, neighbor.facing.opposite())
        {
            return if neighbor.facing == current.facing.counter_clockwise() {
                "outer_left"
            } else {
                "outer_right"
            };
        }
        let back = block.cardinal_neighbor(current.facing.opposite());
        if let Some(neighbor) = self.stair_state_at(back)
            && neighbor.top == current.top
            && current.facing.is_perpendicular(neighbor.facing)
            && self.stair_can_take_shape(block, current, neighbor.facing)
        {
            return if neighbor.facing == current.facing.counter_clockwise() {
                "inner_left"
            } else {
                "inner_right"
            };
        }
        "straight"
    }

    pub(super) fn stair_can_take_shape(
        &self,
        block: BlockKey,
        current: StairState,
        direction: CardinalDirection,
    ) -> bool {
        self.stair_state_at(block.cardinal_neighbor(direction))
            .is_none_or(|neighbor| neighbor.facing != current.facing || neighbor.top != current.top)
    }

    pub(super) fn stair_state_at(&self, block: BlockKey) -> Option<StairState> {
        let key = ChunkKey::from_block(block);
        self.block_chunks
            .get(&key)
            .and_then(|chunk| chunk.stair_state_at(block))
            .or_else(|| {
                self.boundary_chunks
                    .get(&key)
                    .and_then(|chunk| chunk.stair_states.get(&block).copied())
            })
    }

    pub(super) fn is_unprocessed_selected_neighbor(&self, block: BlockKey) -> bool {
        if !self.mask_unprocessed_neighbors {
            return false;
        }
        let chunk_count = bounds_chunk_count(self.bounds);
        if self.processed_chunk_count == 0 || self.processed_chunk_count >= chunk_count {
            return false;
        }
        let chunk_key = ChunkKey::from_block(block);
        self.chunk_key_in_bounds(chunk_key) && !self.processed_chunk_keys.contains(&chunk_key)
    }

    pub(super) fn chunk_key_in_bounds(&self, chunk_key: ChunkKey) -> bool {
        chunk_key.x >= self.bounds.min_chunk_x
            && chunk_key.x <= self.bounds.max_chunk_x
            && chunk_key.z >= self.bounds.min_chunk_z
            && chunk_key.z <= self.bounds.max_chunk_z
    }
}
