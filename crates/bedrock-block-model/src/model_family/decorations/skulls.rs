//! Skull and head block models.
//!
//! Vanilla `blocks.json` points every skull block at the shared `skull` key, whose texture is the
//! `soul_sand` placeholder, and the pack ships no per-face block art for heads. The real models
//! are a client entity: `entity/skull.entity.json` maps each variant to a `geometry.*_head` model
//! in `models/mobs.json` plus an entity texture, and the piglin head uses the `head` bone of
//! `models/entity/piglin.geo.json`.
//!
//! This module rebuilds those entity cubes inside one block cell. The authored cubes stay in
//! entity space (X/Z centred on the entity, Y measured up from the feet), so the placement rule
//! below is the only conversion: a floor skull rests its bounding box on the block floor, a
//! ceiling skull hangs upside down from the block ceiling, and a wall skull stands on its back
//! against the block face and is rotated to the wall direction.

use crate::material::BlockFace;
use crate::model_family::decorations::objects::{Facing, cardinal_from_facing, facing_from_state};
use crate::model_family::direction::CardinalDirection;
use crate::model_family::shape::{ModelCuboid, ModelShape, box_face_corners, box_unwrap_uv};
use crate::state::BlockStateQuery;

/// One authored entity cube: model-space bounds plus the texture origin of its box unwrap.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HeadCube {
    min: [f32; 3],
    max: [f32; 3],
    uv: [f32; 2],
}

/// `geometry.player_head`: the head cube of a player skin. `skull.entity.json` binds the
/// `default`/`steve` variant to `textures/entity/steve`, a 64x64 skin.
const PLAYER_HEAD: &[HeadCube] = &[HeadCube {
    min: [-4.0, 24.0, -4.0],
    max: [4.0, 32.0, 4.0],
    uv: [0.0, 0.0],
}];

/// `geometry.mob_head`: skeleton, wither skeleton, zombie and creeper share this 8 px cube, read
/// from a 64x32 skin.
const MOB_HEAD: &[HeadCube] = PLAYER_HEAD;

/// `geometry.dragon_head`: a 16 px skull with horns, a 16 px snout and a separate jaw, unwrapped
/// from the 256x256 dragon skin.
const DRAGON_HEAD: &[HeadCube] = &[
    HeadCube {
        min: [-8.0, 16.0, -10.0],
        max: [8.0, 32.0, 6.0],
        uv: [112.0, 30.0],
    },
    HeadCube {
        min: [-5.0, 32.0, -4.0],
        max: [-3.0, 36.0, 2.0],
        uv: [0.0, 0.0],
    },
    HeadCube {
        min: [3.0, 32.0, -4.0],
        max: [5.0, 36.0, 2.0],
        uv: [0.0, 0.0],
    },
    HeadCube {
        min: [-6.0, 20.0, -24.0],
        max: [6.0, 25.0, -8.0],
        uv: [176.0, 44.0],
    },
    HeadCube {
        min: [-5.0, 25.0, -22.0],
        max: [-3.0, 27.0, -18.0],
        uv: [112.0, 0.0],
    },
    HeadCube {
        min: [3.0, 25.0, -22.0],
        max: [5.0, 27.0, -18.0],
        uv: [112.0, 0.0],
    },
    HeadCube {
        min: [-6.0, 16.0, -24.0],
        max: [6.0, 20.0, -8.0],
        uv: [176.0, 65.0],
    },
];

/// The `head` bone of `geometry.piglin`: a 10 px skull with a 1 px snout, on a 64x64 skin.
const PIGLIN_HEAD: &[HeadCube] = &[
    HeadCube {
        min: [-5.0, 24.0, -4.0],
        max: [5.0, 32.0, 4.0],
        uv: [0.0, 0.0],
    },
    HeadCube {
        min: [-2.0, 24.0, -5.0],
        max: [2.0, 28.0, -4.0],
        uv: [31.0, 1.0],
    },
    HeadCube {
        min: [-3.0, 24.0, -5.0],
        max: [-2.0, 26.0, -4.0],
        uv: [2.0, 0.0],
    },
    HeadCube {
        min: [2.0, 24.0, -5.0],
        max: [3.0, 26.0, -4.0],
        uv: [2.0, 4.0],
    },
];

/// The cubes and skin size a head variant reads.
struct HeadSkin {
    cubes: &'static [HeadCube],
    span: [f32; 2],
}

fn head_skin(name: &str) -> Option<HeadSkin> {
    let skin = match name {
        "dragon_head" => HeadSkin {
            cubes: DRAGON_HEAD,
            span: [256.0, 256.0],
        },
        "piglin_head" => HeadSkin {
            cubes: PIGLIN_HEAD,
            span: [64.0, 64.0],
        },
        "player_head" => HeadSkin {
            cubes: PLAYER_HEAD,
            span: [64.0, 64.0],
        },
        "skeleton_skull" | "wither_skeleton_skull" | "zombie_head" | "creeper_head" => HeadSkin {
            cubes: MOB_HEAD,
            span: [64.0, 32.0],
        },
        _ => return None,
    };
    Some(skin)
}

/// How a head is mounted in its block cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mount {
    Floor,
    Ceiling,
    Wall(CardinalDirection),
}

impl Mount {
    fn from_facing(facing: Facing) -> Self {
        match facing {
            Facing::Up => Self::Floor,
            Facing::Down => Self::Ceiling,
            _ => Self::Wall(cardinal_from_facing(facing).unwrap_or(CardinalDirection::South)),
        }
    }

    /// Block-space point of one authored model point.
    fn point(self, point: [f32; 3], bounds: HeadBounds) -> [f32; 3] {
        let x = point[0] / 16.0 + 0.5;
        let z = point[2] / 16.0 + 0.5;
        match self {
            // A floor skull rests on the block floor.
            Self::Floor => [x, (point[1] - bounds.min_y) / 16.0, z],
            // A ceiling skull hangs upside down facing the same way, which is a half turn about
            // the model's facing axis (Z).
            Self::Ceiling => [1.0 - x, 1.0 - (point[1] - bounds.min_y) / 16.0, z],
            // A wall skull stands on its back against the block face, centred vertically.
            Self::Wall(direction) => rotate_y(
                [
                    x,
                    (point[1] - bounds.center_y()) / 16.0 + 0.5,
                    (point[2] - bounds.max_z) / 16.0 + 1.0,
                ],
                direction,
            ),
        }
    }

    /// Block face an authored face normal points at after placement.
    fn face(self, normal: [i32; 3]) -> BlockFace {
        let direction = match self {
            Self::Floor => normal,
            Self::Ceiling => [-normal[0], -normal[1], normal[2]],
            Self::Wall(direction) => rotate_y_direction(normal, direction),
        };
        face_from_normal(direction)
    }
}

/// Bounds of one authored head model, used by every placement rule.
#[derive(Clone, Copy, Debug)]
struct HeadBounds {
    min_y: f32,
    max_y: f32,
    max_z: f32,
}

impl HeadBounds {
    fn of(cubes: &[HeadCube]) -> Self {
        let mut bounds = Self {
            min_y: f32::INFINITY,
            max_y: f32::NEG_INFINITY,
            max_z: f32::NEG_INFINITY,
        };
        for cube in cubes {
            bounds.min_y = bounds.min_y.min(cube.min[1]);
            bounds.max_y = bounds.max_y.max(cube.max[1]);
            bounds.max_z = bounds.max_z.max(cube.max[2]);
        }
        bounds
    }

    fn center_y(self) -> f32 {
        (self.min_y + self.max_y) * 0.5
    }
}

pub(crate) fn head_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    let Some(skin) = head_skin(name) else {
        return ModelShape::default();
    };
    let bounds = HeadBounds::of(skin.cubes);
    let mount = Mount::from_facing(facing_from_state(state));
    ModelShape::from_cuboids(
        skin.cubes
            .iter()
            .map(|cube| head_cuboid(*cube, bounds, mount, skin.span))
            .collect::<Vec<_>>(),
    )
}

/// Places one authored cube and gives each of its faces the box-unwrap UV the entity model uses.
fn head_cuboid(cube: HeadCube, bounds: HeadBounds, mount: Mount, span: [f32; 2]) -> ModelCuboid {
    let corners = cube_corners(&cube);
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for point in corners {
        let placed = mount.point(point, bounds);
        for axis in 0..3 {
            min[axis] = min[axis].min(placed[axis]);
            max[axis] = max[axis].max(placed[axis]);
        }
    }

    let mut cuboid = ModelCuboid::new(min, max);
    for (face, normal) in [
        (BlockFace::Up, [0, 1, 0]),
        (BlockFace::Down, [0, -1, 0]),
        (BlockFace::North, [0, 0, -1]),
        (BlockFace::South, [0, 0, 1]),
        (BlockFace::East, [1, 0, 0]),
        (BlockFace::West, [-1, 0, 0]),
    ] {
        let key = mount.face(normal);
        // The authored corner order is the renderer's face order, and placement is a rigid
        // transform, so the transformed corners keep that order for the new face. Only the UV
        // lookup uses the authored points.
        let uv = box_face_corners(cube.min, cube.max, face).map(|point| {
            let [u, v] = box_unwrap_uv(face, cube.uv, cube.min, cube.max, point);
            [u / span[0], v / span[1]]
        });
        cuboid = cuboid.with_face_uv(key, uv);
    }
    cuboid
}

/// The eight authored corners of a cube, X-major so the componentwise min/max is exact.
fn cube_corners(cube: &HeadCube) -> [[f32; 3]; 8] {
    let [x0, y0, z0] = cube.min;
    let [x1, y1, z1] = cube.max;
    [
        [x0, y0, z0],
        [x1, y0, z0],
        [x0, y1, z0],
        [x1, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x0, y1, z1],
        [x1, y1, z1],
    ]
}

/// Rotates a direction by one quarter turn per `direction` step, without the translation a point
/// rotation carries.
fn rotate_y_direction(direction_normal: [i32; 3], direction: CardinalDirection) -> [i32; 3] {
    let turns = match direction {
        CardinalDirection::South => 0,
        CardinalDirection::West => 1,
        CardinalDirection::North => 2,
        CardinalDirection::East => 3,
    };
    let mut normal = direction_normal;
    for _ in 0..turns {
        normal = [-normal[2], normal[1], normal[0]];
    }
    normal
}

/// Rotates a block-space point one quarter turn per `direction` step, like `rotate_cuboid_from_south`.
fn rotate_y(point: [f32; 3], direction: CardinalDirection) -> [f32; 3] {
    let turns = match direction {
        CardinalDirection::South => 0,
        CardinalDirection::West => 1,
        CardinalDirection::North => 2,
        CardinalDirection::East => 3,
    };
    let mut point = point;
    for _ in 0..turns {
        point = [1.0 - point[2], point[1], point[0]];
    }
    point
}

fn face_from_normal(normal: [i32; 3]) -> BlockFace {
    if normal[0] > 0 {
        BlockFace::East
    } else if normal[0] < 0 {
        BlockFace::West
    } else if normal[1] > 0 {
        BlockFace::Up
    } else if normal[1] < 0 {
        BlockFace::Down
    } else if normal[2] > 0 {
        BlockFace::South
    } else {
        BlockFace::North
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::BlockStateQuery;

    fn state_with_facing(value: i64) -> BlockStateQuery {
        BlockStateQuery::new("minecraft:skull").with_state("facing_direction", value)
    }

    fn shape_for(name: &str, facing: i64) -> ModelShape {
        head_shape(name, &state_with_facing(facing))
    }

    #[test]
    fn floor_skull_rests_on_the_block_floor() {
        let shape = shape_for("skeleton_skull", 1);
        assert_eq!(shape.cuboids.len(), 1);
        let cuboid = &shape.cuboids[0];
        assert!((cuboid.min[1] - 0.0).abs() < 1e-6);
        assert!((cuboid.max[1] - 0.5).abs() < 1e-6);
        assert!((cuboid.min[0] - 0.25).abs() < 1e-6);
        assert!((cuboid.max[0] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn ceiling_skull_hangs_from_the_block_ceiling() {
        let shape = shape_for("skeleton_skull", 0);
        let cuboid = &shape.cuboids[0];
        assert!((cuboid.min[1] - 0.5).abs() < 1e-6);
        assert!((cuboid.max[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn wall_skull_stands_on_its_back_against_the_block_face() {
        // `facing_direction` 3 is south, so the skull sits on the south face and looks south.
        let shape = shape_for("zombie_head", 3);
        let cuboid = &shape.cuboids[0];
        assert!((cuboid.min[2] - 0.5).abs() < 1e-6);
        assert!((cuboid.max[2] - 1.0).abs() < 1e-6);
        assert!((cuboid.min[1] - 0.25).abs() < 1e-6);
        assert!((cuboid.max[1] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn mob_skull_uvs_match_the_64x32_skin_layout() {
        // The head cube unwraps from skin (0,0) on 8x8x8 px: front at (8,8), right at (0,8),
        // left at (16,8) and back at (24,8).
        let shape = shape_for("skeleton_skull", 1);
        let cuboid = &shape.cuboids[0];
        let rect = |face: BlockFace| {
            let uv = cuboid.face_uvs.get(&face).copied().expect("face uv");
            [
                (uv[0][0] * 64.0).round(),
                (uv[0][1] * 32.0).round(),
                (uv[2][0] * 64.0).round(),
                (uv[2][1] * 32.0).round(),
            ]
        };
        // Corner 0 of every face is its bottom-left, so the first row is the lower skin row.
        assert_eq!(rect(BlockFace::North), [8.0, 16.0, 16.0, 8.0]);
        assert_eq!(rect(BlockFace::West), [0.0, 16.0, 8.0, 8.0]);
        assert_eq!(rect(BlockFace::East), [16.0, 16.0, 24.0, 8.0]);
        assert_eq!(rect(BlockFace::South), [24.0, 16.0, 32.0, 8.0]);
        assert_eq!(rect(BlockFace::Up), [8.0, 0.0, 16.0, 8.0]);
        assert_eq!(rect(BlockFace::Down), [16.0, 0.0, 24.0, 8.0]);
    }

    #[test]
    fn player_head_reads_the_64x64_skin_not_the_mob_layout() {
        let shape = shape_for("player_head", 1);
        let uv = shape.cuboids[0].face_uvs[&BlockFace::North];
        // Head art occupies rows 8..16 of the 64-row player skin, so V stays in 0.25..0.5 of the
        // full sprite; the 64x32 mob layout would map it to 0.25..0.5 of a 32-row half.
        assert!((uv[0][1] * 64.0 - 16.0).abs() < 1e-4);
        assert!((uv[2][1] * 64.0 - 8.0).abs() < 1e-4);
    }

    #[test]
    fn dragon_head_keeps_the_snout_and_horns() {
        let shape = shape_for("dragon_head", 1);
        assert_eq!(shape.cuboids.len(), DRAGON_HEAD.len());
        // The skull is 16 px wide and rests on the floor; the snout reaches past the block.
        let skull = &shape.cuboids[0];
        assert!((skull.min[1]).abs() < 1e-6);
        assert!((skull.max[1] - 1.0).abs() < 1e-6);
        let min_z = shape
            .cuboids
            .iter()
            .map(|cuboid| cuboid.min[2])
            .fold(f32::INFINITY, f32::min);
        assert!(min_z < -0.5, "snout must reach past the block, got {min_z}");
    }

    #[test]
    fn piglin_head_uses_the_piglin_skin_head_bone() {
        let shape = shape_for("piglin_head", 1);
        assert_eq!(shape.cuboids.len(), PIGLIN_HEAD.len());
        let skull = &shape.cuboids[0];
        // A 10 px wide head leaves 3 px of margin on each side.
        assert!((skull.min[0] - 0.1875).abs() < 1e-6);
        assert!((skull.max[0] - 0.8125).abs() < 1e-6);
    }

    #[test]
    fn every_head_face_samples_inside_the_skin() {
        for name in [
            "skeleton_skull",
            "wither_skeleton_skull",
            "zombie_head",
            "creeper_head",
            "player_head",
            "piglin_head",
            "dragon_head",
        ] {
            for facing in 0..=5 {
                let shape = shape_for(name, facing);
                assert!(!shape.cuboids.is_empty(), "{name} facing {facing}");
                for cuboid in &shape.cuboids {
                    assert_eq!(cuboid.face_uvs.len(), 6, "{name} facing {facing}");
                    for (face, uv) in &cuboid.face_uvs {
                        for point in uv {
                            assert!(
                                (0.0..=1.0).contains(&point[0]) && (0.0..=1.0).contains(&point[1]),
                                "{name} facing {facing} {face:?} samples {point:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
