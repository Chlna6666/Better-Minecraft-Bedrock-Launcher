//! Immutable block placements and checked world-coordinate transforms.

use super::block_edit::{BlockEdit, BlockEditOptions};
use super::block_edit_plan::{BlockEditPlan, plan_block_edits};
use crate::{
    BedrockWorldError, BlockPos, BlockState, BlockStateQueryControl, ChunkCapabilities, ChunkPos,
    ChunkVersion, Dimension, Result, StorageBackend, World, WorldTransaction, WriteGuard,
};
use std::collections::{BTreeSet, HashSet};

/// One block position relative to a placement origin, with positive Z pointing south.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BlockOffset {
    /// Eastward displacement in blocks.
    pub x: i32,
    /// Upward displacement in blocks.
    pub y: i32,
    /// Southward displacement in blocks.
    pub z: i32,
}

/// One block and its local offset, independent of a world position.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementBlock {
    /// Position relative to the placement origin.
    pub offset: BlockOffset,
    /// Bedrock primary-layer state to place.
    pub state: BlockState,
}

/// Immutable primary-layer blocks produced by an image or model conversion.
///
/// This value contains no world coordinates and performs no LevelDB writes. Duplicate local
/// positions are rejected so preview, collision scan and write observe the same block set.
#[derive(Debug, Clone)]
pub struct BlockPlacementPlan {
    blocks: Vec<PlacementBlock>,
}

impl BlockPlacementPlan {
    /// Creates a nonempty placement with unique local block positions.
    ///
    /// # Errors
    ///
    /// Returns a validation error for an empty plan or duplicate local positions.
    pub fn new(blocks: Vec<PlacementBlock>) -> Result<Self> {
        if blocks.is_empty() {
            return Err(BedrockWorldError::Validation(
                "block placement is empty".to_string(),
            ));
        }
        let mut seen = BTreeSet::new();
        if blocks.iter().any(|block| !seen.insert(block.offset)) {
            return Err(BedrockWorldError::Validation(
                "block placement contains duplicate positions".to_string(),
            ));
        }
        Ok(Self { blocks })
    }

    /// Returns local blocks without exposing mutation of the plan.
    #[must_use]
    pub fn blocks(&self) -> &[PlacementBlock] {
        &self.blocks
    }

    /// Prepares typed edits and scans their exact source snapshot for unsafe target cells.
    ///
    /// Every target chunk must exist and support direct typed writes. A block entity at a target
    /// position blocks the placement, including when its block state is air. Ordinary occupied
    /// blocks are counted for explicit overwrite confirmation. The resulting edit plan rechecks
    /// complete source chunk records under the world transaction's commit lock.
    ///
    /// This performs no persistent write. Direct raw storage writes and external Minecraft
    /// processes remain outside the in-process transaction lock.
    ///
    /// # Errors
    ///
    /// Returns validation errors for missing, unsupported or protected chunks, target block
    /// entities, and coordinates outside the target build height; storage and decode errors are
    /// propagated from the existing typed block editor.
    pub fn validate<S>(
        &self,
        world: &World<S>,
        transform: PlacementTransform,
        dimension: Dimension,
        version: ChunkVersion,
        guard: &WriteGuard,
        options: BlockEditOptions,
    ) -> Result<PlacementValidation>
    where
        S: StorageBackend,
    {
        transform.check(self, dimension, version)?;
        let edits = self
            .blocks()
            .iter()
            .map(|block| {
                Ok(BlockEdit::new(
                    dimension,
                    transform.position(block.offset)?,
                    block.state.clone(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let plan = plan_block_edits(world, &edits, &[], guard, options)?.ok_or_else(|| {
            BedrockWorldError::Validation("unconditional placement was rejected".to_string())
        })?;
        let source = plan.source_world(world)?;
        let positions = edits.iter().map(|edit| edit.position).collect::<Vec<_>>();
        let targets = positions.iter().copied().collect::<HashSet<_>>();
        for chunk_pos in plan.affected_chunks() {
            let chunk = source.chunk(*chunk_pos)?;
            if chunk.records.is_empty() {
                return Err(BedrockWorldError::Validation(format!(
                    "target chunk {chunk_pos:?} is missing"
                )));
            }
            if !ChunkCapabilities::inspect(&chunk.records).directly_writable() {
                return Err(BedrockWorldError::Validation(format!(
                    "target chunk {chunk_pos:?} contains unsupported or protected records"
                )));
            }
            for record in source.block_entities(*chunk_pos)? {
                let Some([x, y, z]) = record.entity.position else {
                    return Err(BedrockWorldError::Validation(format!(
                        "target chunk {chunk_pos:?} contains a block entity without coordinates"
                    )));
                };
                if targets.contains(&BlockPos { x, y, z }) {
                    return Err(BedrockWorldError::Validation(format!(
                        "placement targets block entity at ({x}, {y}, {z})"
                    )));
                }
            }
        }
        let mut occupied_blocks = 0;
        source.for_each_block_state_at(dimension, positions, |entry| {
            if entry.state.is_some_and(|state| {
                !matches!(
                    state.name.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                )
            }) {
                occupied_blocks += 1;
            }
            Ok(BlockStateQueryControl::Continue)
        })?;
        Ok(PlacementValidation {
            plan,
            occupied_blocks,
        })
    }
}

/// A prepared placement and its collision count from the same immutable source chunks.
///
/// `stage` records source preconditions in one [`WorldTransaction`]. The caller must use that
/// transaction's single commit as the write boundary; no per-chunk fallback is provided.
#[derive(Debug)]
pub struct PlacementValidation {
    plan: BlockEditPlan,
    occupied_blocks: usize,
}

impl PlacementValidation {
    /// Returns the number of ordinary occupied target positions requiring overwrite confirmation.
    #[must_use]
    pub const fn occupied_blocks(&self) -> usize {
        self.occupied_blocks
    }

    /// Returns the number of planned primary-layer block edits.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.plan.edited_blocks()
    }

    /// Returns chunks that will be refreshed if the placement commits.
    #[must_use]
    pub fn affected_chunks(&self) -> &BTreeSet<ChunkPos> {
        self.plan.affected_chunks()
    }

    /// Stages the entire placement in one transaction after explicit overwrite approval.
    ///
    /// This does not commit. A cancelled operation can drop the transaction without writing;
    /// commit rechecks complete source chunk records under the mutation lock.
    ///
    /// # Errors
    ///
    /// Returns a validation error if occupied blocks exist without overwrite confirmation.
    pub fn stage<S>(
        self,
        transaction: &mut WorldTransaction<'_, S>,
        confirm_overwrite: bool,
    ) -> Result<()>
    where
        S: StorageBackend,
    {
        if self.occupied_blocks > 0 && !confirm_overwrite {
            return Err(BedrockWorldError::Validation(format!(
                "placement would overwrite {} occupied blocks",
                self.occupied_blocks
            )));
        }
        self.plan.stage(transaction);
        Ok(())
    }
}

/// Rotation about the upward Y axis, applied after X/Z mirrors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementRotation {
    /// Preserve east and south axes.
    None,
    /// Rotate clockwise by 90 degrees when viewed from above.
    Clockwise90,
    /// Rotate by 180 degrees.
    HalfTurn,
    /// Rotate clockwise by 270 degrees when viewed from above.
    Clockwise270,
}

/// Checked position and orientation of one immutable placement.
///
/// Coordinates are validated against i32 storage limits and the dimension's build height before
/// this value can be created. It does not scan target blocks or grant permission to overwrite them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementTransform {
    origin: BlockPos,
    rotation: PlacementRotation,
    mirror_x: bool,
    mirror_z: bool,
}

impl PlacementTransform {
    /// Checks every block in `plan` against the target build height and coordinate range.
    ///
    /// # Errors
    ///
    /// Returns a validation error when a transformed coordinate is outside i32 or the dimension's
    /// build height. No world record is read or written.
    pub fn checked(
        plan: &BlockPlacementPlan,
        origin: BlockPos,
        rotation: PlacementRotation,
        mirror_x: bool,
        mirror_z: bool,
        dimension: Dimension,
        version: ChunkVersion,
    ) -> Result<Self> {
        let transform = Self {
            origin,
            rotation,
            mirror_x,
            mirror_z,
        };
        transform.check(plan, dimension, version)?;
        Ok(transform)
    }

    fn check(
        self,
        plan: &BlockPlacementPlan,
        dimension: Dimension,
        version: ChunkVersion,
    ) -> Result<()> {
        let (min_y, max_y) = ChunkPos {
            x: 0,
            z: 0,
            dimension,
        }
        .y_range(version);
        for block in plan.blocks() {
            let position = self.position(block.offset)?;
            if position.y < min_y || position.y > max_y {
                return Err(BedrockWorldError::Validation(format!(
                    "placement y={} is outside {dimension:?} build height {min_y}..={max_y}",
                    position.y
                )));
            }
        }
        Ok(())
    }

    /// Maps one local block into the world using checked arithmetic.
    ///
    /// # Errors
    ///
    /// Returns a validation error if any absolute coordinate is outside i32.
    pub fn position(self, offset: BlockOffset) -> Result<BlockPos> {
        let x = i64::from(offset.x) * if self.mirror_x { -1 } else { 1 };
        let z = i64::from(offset.z) * if self.mirror_z { -1 } else { 1 };
        let (x, z) = match self.rotation {
            PlacementRotation::None => (x, z),
            PlacementRotation::Clockwise90 => (-z, x),
            PlacementRotation::HalfTurn => (-x, -z),
            PlacementRotation::Clockwise270 => (z, -x),
        };
        let coordinate = |origin: i32, displacement: i64| {
            i32::try_from(i64::from(origin) + displacement).map_err(|_| {
                BedrockWorldError::Validation("placement coordinate exceeds i32".to_string())
            })
        };
        Ok(BlockPos {
            x: coordinate(self.origin.x, x)?,
            y: coordinate(self.origin.y, i64::from(offset.y))?,
            z: coordinate(self.origin.z, z)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Biome2d, ChunkKey, ChunkRecordTag, MemoryStorage, OpenOptions, WorldStorage};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn plan() -> BlockPlacementPlan {
        BlockPlacementPlan::new(vec![PlacementBlock {
            offset: BlockOffset { x: 2, y: 0, z: 3 },
            state: BlockState {
                name: "minecraft:stone".to_string(),
                states: BTreeMap::new(),
                version: None,
            },
        }])
        .unwrap()
    }

    #[test]
    fn checked_transform_rotates_and_mirrors_local_coordinates() {
        let plan = plan();
        let transform = PlacementTransform::checked(
            &plan,
            BlockPos {
                x: 10,
                y: 64,
                z: 20,
            },
            PlacementRotation::Clockwise90,
            true,
            false,
            Dimension::Overworld,
            ChunkVersion::New,
        )
        .unwrap();
        assert_eq!(
            transform.position(plan.blocks()[0].offset).unwrap(),
            BlockPos { x: 7, y: 64, z: 18 }
        );
    }

    #[test]
    fn checked_transform_rejects_overflow_and_build_height() {
        let plan = plan();
        assert!(
            PlacementTransform::checked(
                &plan,
                BlockPos {
                    x: i32::MAX,
                    y: 64,
                    z: 0
                },
                PlacementRotation::None,
                false,
                false,
                Dimension::Overworld,
                ChunkVersion::New,
            )
            .is_err()
        );
        assert!(
            PlacementTransform::checked(
                &plan,
                BlockPos { x: 0, y: 320, z: 0 },
                PlacementRotation::None,
                false,
                false,
                Dimension::Overworld,
                ChunkVersion::New,
            )
            .is_err()
        );
    }

    #[test]
    fn minimum_world_coordinate_maps_to_chunk_without_overflow() {
        let position = BlockPos {
            x: i32::MIN,
            y: 0,
            z: i32::MIN,
        };
        assert_eq!(
            position.to_chunk_pos(Dimension::Overworld),
            ChunkPos {
                x: -134_217_728,
                z: -134_217_728,
                dimension: Dimension::Overworld,
            }
        );
    }

    #[test]
    fn placement_rejects_duplicate_local_positions() {
        let block = plan().blocks()[0].clone();
        assert!(BlockPlacementPlan::new(vec![block.clone(), block]).is_err());
    }

    #[test]
    fn placement_requires_overwrite_confirmation_and_rechecks_source_at_commit() {
        let storage = Arc::new(MemoryStorage::new());
        let world = World::from_storage(
            "memory",
            storage.clone(),
            OpenOptions {
                read_only: false,
                ..OpenOptions::default()
            },
        );
        let chunk = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let version_key = ChunkKey::new(chunk, ChunkRecordTag::Version).encode();
        storage.put(&version_key, &[9]).unwrap();
        storage
            .put(
                &ChunkKey::new(chunk, ChunkRecordTag::Data2D).encode(),
                &Biome2d::new(vec![64; 256], vec![1; 256])
                    .unwrap()
                    .encode()
                    .unwrap(),
            )
            .unwrap();
        let position = BlockPos { x: 1, y: 64, z: 1 };
        let placement = BlockPlacementPlan::new(vec![PlacementBlock {
            offset: BlockOffset { x: 0, y: 0, z: 0 },
            state: BlockState {
                name: "minecraft:stone".to_string(),
                states: BTreeMap::new(),
                version: Some(17_959_425),
            },
        }])
        .unwrap();
        let transform = PlacementTransform::checked(
            &placement,
            position,
            PlacementRotation::None,
            false,
            false,
            Dimension::Overworld,
            ChunkVersion::Old,
        )
        .unwrap();
        let guard = WriteGuard::confirmed(world.path(), "placement test");
        let validate = || {
            placement.validate(
                &world,
                transform,
                Dimension::Overworld,
                ChunkVersion::Old,
                &guard,
                BlockEditOptions::default(),
            )
        };

        let first = validate().unwrap();
        assert_eq!(first.occupied_blocks(), 0);
        let mut cancelled = world.transaction();
        first.stage(&mut cancelled, false).unwrap();
        drop(cancelled);
        assert!(
            world
                .block_state(Dimension::Overworld, position)
                .unwrap()
                .is_none()
        );

        let first = validate().unwrap();
        let mut transaction = world.transaction();
        first.stage(&mut transaction, false).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            world
                .block_state(Dimension::Overworld, position)
                .unwrap()
                .unwrap()
                .name,
            "minecraft:stone"
        );

        let occupied = validate().unwrap();
        assert_eq!(occupied.occupied_blocks(), 1);
        let mut transaction = world.transaction();
        assert!(occupied.stage(&mut transaction, false).is_err());
        drop(transaction);

        let stale = validate().unwrap();
        let mut transaction = world.transaction();
        stale.stage(&mut transaction, true).unwrap();
        storage.put(&version_key, &[10]).unwrap();
        assert!(matches!(
            transaction.commit(),
            Err(BedrockWorldError::ConcurrentWrite(_))
        ));
    }
}
