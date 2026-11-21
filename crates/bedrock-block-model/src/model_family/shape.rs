use std::collections::BTreeMap;

use crate::material::BlockFace;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelShape {
    pub cuboids: Vec<ModelCuboid>,
    pub planes: Vec<ModelPlane>,
}

impl ModelShape {
    #[must_use]
    pub fn from_cuboids(cuboids: impl Into<Vec<ModelCuboid>>) -> Self {
        Self {
            cuboids: cuboids.into(),
            planes: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_planes(mut self, planes: impl Into<Vec<ModelPlane>>) -> Self {
        self.planes = planes.into();
        self
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.cuboids.is_empty() && self.planes.is_empty()
    }

    #[must_use]
    pub fn cull_faces(mut self, hidden_faces: &[BlockFace]) -> Self {
        for cuboid in &mut self.cuboids {
            cuboid.cull_faces(hidden_faces);
        }
        self.planes.retain(|plane| {
            let face = match plane.normal {
                [0, 1, 0] => BlockFace::Up,
                [0, -1, 0] => BlockFace::Down,
                [0, 0, -1] => BlockFace::North,
                [0, 0, 1] => BlockFace::South,
                [1, 0, 0] => BlockFace::East,
                [-1, 0, 0] => BlockFace::West,
                _ => return true,
            };
            !hidden_faces.contains(&face)
        });
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelCuboid {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub material_slot: Option<String>,
    pub face_material_slots: BTreeMap<BlockFace, String>,
    pub face_uvs: BTreeMap<BlockFace, [[f32; 2]; 4]>,
}

impl ModelCuboid {
    #[must_use]
    pub fn new(min: [f32; 3], max: [f32; 3]) -> Self {
        Self {
            min,
            max,
            material_slot: None,
            face_material_slots: BTreeMap::new(),
            face_uvs: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_material_slot(mut self, material_slot: impl Into<String>) -> Self {
        self.material_slot = Some(material_slot.into());
        self
    }

    #[must_use]
    pub fn with_face_material_slot(mut self, face: BlockFace, slot: impl Into<String>) -> Self {
        self.face_material_slots.insert(face, slot.into());
        self
    }

    #[must_use]
    pub fn with_face_uv(mut self, face: BlockFace, uv: [[f32; 2]; 4]) -> Self {
        self.face_uvs.insert(face, uv);
        self
    }

    pub fn cull_faces(&mut self, hidden_faces: &[BlockFace]) {
        for face in hidden_faces {
            let is_on_boundary = match face {
                BlockFace::Up => (self.max[1] - 1.0).abs() < 0.001,
                BlockFace::Down => self.min[1].abs() < 0.001,
                BlockFace::North => self.min[2].abs() < 0.001,
                BlockFace::South => (self.max[2] - 1.0).abs() < 0.001,
                BlockFace::West => self.min[0].abs() < 0.001,
                BlockFace::East => (self.max[0] - 1.0).abs() < 0.001,
                _ => false,
            };
            if is_on_boundary {
                self.face_material_slots.remove(face);
                self.face_uvs.remove(face);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ModelPlaneSidedness {
    /// Thin/decal geometry such as vines and crossed plants is visible from both directions.
    #[default]
    DoubleSided,
    /// One physical surface of a solid or rotated element. Renderers must not synthesize a back face.
    FrontOnly,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelPlane {
    pub corners: [[f32; 3]; 4],
    pub normal: [i32; 3],
    pub material_slot: Option<String>,
    pub uv: Option<[[f32; 2]; 4]>,
    pub sidedness: ModelPlaneSidedness,
}

impl ModelPlane {
    #[must_use]
    pub fn new(corners: [[f32; 3]; 4], normal: [i32; 3]) -> Self {
        Self {
            corners,
            normal,
            material_slot: None,
            uv: None,
            sidedness: ModelPlaneSidedness::DoubleSided,
        }
    }

    /// Marks this quad as one physical face rather than a thin two-sided plane.
    #[must_use]
    pub const fn front_only(mut self) -> Self {
        self.sidedness = ModelPlaneSidedness::FrontOnly;
        self
    }

    #[must_use]
    pub fn with_material_slot(mut self, material_slot: impl Into<String>) -> Self {
        self.material_slot = Some(material_slot.into());
        self
    }

    #[must_use]
    pub fn with_uv(mut self, uv: [[f32; 2]; 4]) -> Self {
        self.uv = Some(uv);
        self
    }
}

#[must_use]
pub fn rect_uv(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    [[u0, v0], [u1, v0], [u1, v1], [u0, v1]]
}

#[must_use]
pub fn uv16(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    rect_uv(u0 / 16.0, v0 / 16.0, u1 / 16.0, v1 / 16.0)
}

#[must_use]
pub fn full_texture_uv() -> [[f32; 2]; 4] {
    uv16(0.0, 0.0, 16.0, 16.0)
}

/// Full-tile UVs for one authored face, in that face's corner order.
///
/// The renderer emits every face from outside the block starting at its **bottom-left** corner,
/// and the documented convention is that V grows downward from the block top. A whole-tile
/// sample therefore has to start at `v = 1` on a vertical face; `full_texture_uv` starts at
/// `v = 0` and is only correct for the two caps, whose corner order starts elsewhere.
///
/// Applying `full_texture_uv` to a north/south/east/west face is what made plants, item frames
/// and shelves read as upside down: the art was sampled top-down while the quad was built
/// bottom-up.
#[must_use]
pub fn full_texture_face_uv(face: BlockFace) -> [[f32; 2]; 4] {
    match face {
        BlockFace::North | BlockFace::South | BlockFace::East | BlockFace::West => {
            upright_full_texture_uv()
        }
        _ => full_texture_uv(),
    }
}

/// Full-texture UVs for an upright plane whose corners start at the bottom-left.
///
/// Vertical decorations such as flowers, vines and rails use this so the source texture is not
/// flipped upside down.
#[must_use]
pub fn upright_full_texture_uv() -> [[f32; 2]; 4] {
    [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]
}

/// The same full-tile quad flipped vertically, for plants that grow downwards.
///
/// Hanging moss, roots and vines hang from the block above, so their art has to point down
/// instead of standing on the block floor.
#[must_use]
pub fn hanging_full_texture_uv() -> [[f32; 2]; 4] {
    [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
}

/// The four corners of one authored box face, in this renderer's face order.
///
/// Corner 0 is the bottom-left of the face as seen from outside, which is why unwrapped V grows
/// downward: `box_unwrap_uv` maps corner 0 to the lower skin row.
#[must_use]
pub fn box_face_corners(min: [f32; 3], max: [f32; 3], face: BlockFace) -> [[f32; 3]; 4] {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    match face {
        BlockFace::Up => [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
        BlockFace::Down => [[x0, y0, z1], [x1, y0, z1], [x1, y0, z0], [x0, y0, z0]],
        BlockFace::North => [[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0]],
        BlockFace::South => [[x1, y0, z1], [x0, y0, z1], [x0, y1, z1], [x1, y1, z1]],
        BlockFace::East => [[x1, y0, z0], [x1, y0, z1], [x1, y1, z1], [x1, y1, z0]],
        BlockFace::West => [[x0, y0, z1], [x0, y0, z0], [x0, y1, z0], [x0, y1, z1]],
        // Aggregate selectors never name one geometric face.
        _ => [[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0]],
    }
}

/// Per-face UVs for one authored box, unwrapped by `box_unwrap_uv` and normalised to `span`.
///
/// `min` and `max` are authoring pixels and `span` is the skin size the box reads, so a `64x64`
/// entity sheet passes `[64.0, 64.0]`.
#[must_use]
pub fn box_unwrap_face_uvs(
    min: [f32; 3],
    max: [f32; 3],
    origin: [f32; 2],
    span: [f32; 2],
) -> [(BlockFace, [[f32; 2]; 4]); 6] {
    const FACES: [BlockFace; 6] = [
        BlockFace::Up,
        BlockFace::Down,
        BlockFace::North,
        BlockFace::South,
        BlockFace::East,
        BlockFace::West,
    ];
    FACES.map(|face| {
        let uv = box_face_corners(min, max, face).map(|point| {
            let [u, v] = box_unwrap_uv(face, origin, min, max, point);
            [u / span[0], v / span[1]]
        });
        (face, uv)
    })
}

/// The texture point an authored Bedrock cube samples at `point` on `face`.
///
/// Every Bedrock cube unwraps as one strip of four side faces with the two caps above the front
/// face, so side faces share a `depth + height` band. `origin` is the cube's `uv` in authoring
/// pixels; the result is in authoring pixels too, and a caller reading a `64x64` skin divides by
/// 64.
#[must_use]
pub fn box_unwrap_uv(
    face: BlockFace,
    origin: [f32; 2],
    min: [f32; 3],
    max: [f32; 3],
    point: [f32; 3],
) -> [f32; 2] {
    let [u0, v0] = origin;
    let [x0, _, z0] = min;
    let [x1, y1, z1] = max;
    let width = x1 - x0;
    let depth = z1 - z0;
    match face {
        // +Y cap: u across X, v across Z.
        BlockFace::Up => [u0 + depth + (point[0] - x0), v0 + (point[2] - z0)],
        // -Y cap, mirrored in Z.
        BlockFace::Down => [u0 + depth + width + (point[0] - x0), v0 + (z1 - point[2])],
        // -Z face: u across X, v up from the cube's base.
        BlockFace::North => [u0 + depth + (point[0] - x0), v0 + depth + (y1 - point[1])],
        // +Z face: u mirrored in X.
        BlockFace::South => [
            u0 + depth + width + depth + (x1 - point[0]),
            v0 + depth + (y1 - point[1]),
        ],
        // +X face: u across Z.
        BlockFace::East => [
            u0 + depth + width + (point[2] - z0),
            v0 + depth + (y1 - point[1]),
        ],
        // -X face: u mirrored in Z.
        BlockFace::West => [u0 + (z1 - point[2]), v0 + depth + (y1 - point[1])],
        // Aggregate selectors never name one geometric face; the cube origin keeps this total.
        _ => [u0, v0],
    }
}

/// Turns a cuboid authored facing north onto one of the four cardinals.
///
/// Bedrock rotates a north-authored block model with `rotation: [0, 0, 0]` for north,
/// `[0, 90, 0]` for west, `[0, 180, 0]` for south and `[0, -90, 0]` for east, so the model's
/// north face travels onto the named face. Each step is a quarter turn clockwise seen from
/// above: north becomes east, then south, then west. Face slots and UV quads travel with the
/// geometry, so a mirrored x/z swap is not an acceptable substitute.
#[must_use]
pub(super) fn rotate_cuboid_cardinal(
    cuboid: &ModelCuboid,
    direction: super::direction::CardinalDirection,
) -> ModelCuboid {
    use super::direction::CardinalDirection;
    let turns = match direction {
        CardinalDirection::North => 0,
        CardinalDirection::East => 1,
        CardinalDirection::South => 2,
        CardinalDirection::West => 3,
    };
    (0..turns).fold(cuboid.clone(), |cuboid, _| quarter_turn_clockwise(&cuboid))
}

/// Turns a plane authored facing north onto one of the four cardinals.
///
/// The corners keep their order and UVs, so the texture turns with the geometry; only the
/// normal is re-derived from the same quarter turns.
#[must_use]
pub(super) fn rotate_plane_cardinal(
    plane: &ModelPlane,
    direction: super::direction::CardinalDirection,
) -> ModelPlane {
    use super::direction::CardinalDirection;
    let quarter_turns = match direction {
        CardinalDirection::North => 0,
        CardinalDirection::East => 1,
        CardinalDirection::South => 2,
        CardinalDirection::West => 3,
    };
    let mut corners = plane.corners;
    let mut normal = plane.normal;
    for _ in 0..quarter_turns {
        for corner in &mut corners {
            let (x, z) = (corner[0], corner[2]);
            corner[0] = 1.0 - z;
            corner[2] = x;
        }
        let (nx, nz) = (normal[0], normal[2]);
        normal = [-nz, normal[1], nx];
    }
    let mut rotated = ModelPlane::new(corners, normal);
    rotated.material_slot = plane.material_slot.clone();
    rotated.uv = plane.uv;
    rotated
}

/// Turns a whole north-authored shape onto one of the four cardinals.
#[must_use]
pub(super) fn rotate_shape_cardinal(
    shape: ModelShape,
    direction: super::direction::CardinalDirection,
) -> ModelShape {
    let mut rotated = ModelShape::from_cuboids(
        shape
            .cuboids
            .iter()
            .map(|cuboid| rotate_cuboid_cardinal(cuboid, direction))
            .collect::<Vec<_>>(),
    );
    rotated.planes = shape
        .planes
        .iter()
        .map(|plane| rotate_plane_cardinal(plane, direction))
        .collect();
    rotated
}

/// Turns a whole shape about the block's vertical centre by an arbitrary angle in degrees
/// (positive = clockwise seen from above).
///
/// Standing signs and banners have sixteen 22.5 degree orientations, so their cuboids become
/// quads: an axis-aligned box cannot express the turn, and the per-face slots and UVs still
/// travel with each face.
#[must_use]
pub(super) fn rotate_shape_y_degrees(shape: ModelShape, degrees: f32) -> ModelShape {
    if degrees.abs() < 0.001 {
        // Keep the solid cuboid representation for the south-facing default.
        return shape;
    }
    let (sin, cos) = degrees.to_radians().sin_cos();
    let rotate = |point: [f32; 3]| {
        [
            0.5 + (point[0] - 0.5) * cos - (point[2] - 0.5) * sin,
            point[1],
            0.5 + (point[0] - 0.5) * sin + (point[2] - 0.5) * cos,
        ]
    };
    let mut rotated = ModelShape::default();
    for cuboid in &shape.cuboids {
        for (face, normal) in [
            (BlockFace::Up, [0, 1, 0]),
            (BlockFace::Down, [0, -1, 0]),
            (BlockFace::North, [0, 0, -1]),
            (BlockFace::South, [0, 0, 1]),
            (BlockFace::East, [1, 0, 0]),
            (BlockFace::West, [-1, 0, 0]),
        ] {
            let corners = box_face_corners(cuboid.min, cuboid.max, face).map(rotate);
            let mut plane = ModelPlane::new(corners, normal).front_only();
            if let Some(uv) = cuboid.face_uvs.get(&face).copied() {
                plane = plane.with_uv(uv);
            }
            if let Some(slot) = cuboid
                .face_material_slots
                .get(&face)
                .or_else(|| cuboid.material_slot.as_ref())
            {
                plane = plane.with_material_slot(slot);
            }
            rotated.planes.push(plane);
        }
    }
    for plane in &shape.planes {
        let mut turned = plane.clone();
        turned.corners = plane.corners.map(rotate);
        rotated.planes.push(turned);
    }
    rotated
}

/// Turns a shape about the block's X axis in quarter steps (positive = `y` towards `z`).
///
/// Levers, buttons and other wall-mounted parts use this to move a floor-authored base onto a
/// wall or ceiling; face slots and UVs travel with the geometry.
#[must_use]
pub(super) fn rotate_shape_x(shape: ModelShape, quarter_turns: i32) -> ModelShape {
    let turns = quarter_turns.rem_euclid(4);
    let mut rotated = ModelShape::from_cuboids(
        shape
            .cuboids
            .iter()
            .map(|cuboid| (0..turns).fold(cuboid.clone(), |cuboid, _| quarter_turn_x(&cuboid)))
            .collect::<Vec<_>>(),
    );
    rotated.planes = shape
        .planes
        .iter()
        .map(|plane| {
            let mut plane = plane.clone();
            for _ in 0..turns {
                plane = quarter_turn_plane_x(&plane);
            }
            plane
        })
        .collect();
    rotated
}

/// Rotates a face's UV quad inside the face by whole quarter turns (clockwise).
///
/// Blocks whose art carries direction (the anvil top, the observer face) only express their
/// facing through the texture once their boxes are symmetric, so the geometry turn alone is not
/// enough.
#[must_use]
pub(super) fn rotate_uv_quarter(uv: [[f32; 2]; 4], quarter_turns: i32) -> [[f32; 2]; 4] {
    let turns = quarter_turns.rem_euclid(4) as usize;
    let mut rotated = uv;
    for _ in 0..turns {
        rotated = [rotated[1], rotated[2], rotated[3], rotated[0]];
    }
    rotated
}

fn quarter_turn_x(cuboid: &ModelCuboid) -> ModelCuboid {
    let mut rotated = ModelCuboid::new(
        [cuboid.min[0], 1.0 - cuboid.max[2], cuboid.min[1]],
        [cuboid.max[0], 1.0 - cuboid.min[2], cuboid.max[1]],
    );
    rotated.material_slot = cuboid.material_slot.clone();
    for (face, slot) in &cuboid.face_material_slots {
        rotated
            .face_material_slots
            .insert(x_quarter_face(*face), slot.clone());
    }
    for (face, uv) in &cuboid.face_uvs {
        // Rigid pitch: every face's art rotates one quarter with the geometry.
        rotated
            .face_uvs
            .insert(x_quarter_face(*face), rotate_uv_quarter(*uv, 1));
    }
    rotated
}

fn quarter_turn_plane_x(plane: &ModelPlane) -> ModelPlane {
    let corners = plane
        .corners
        .map(|corner| [corner[0], 1.0 - corner[2], corner[1]]);
    let mut rotated = ModelPlane::new(corners, x_quarter_normal(plane.normal));
    rotated.material_slot = plane.material_slot.clone();
    rotated.uv = plane.uv;
    rotated.sidedness = plane.sidedness;
    rotated
}

const fn x_quarter_face(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::Up => BlockFace::South,
        BlockFace::South => BlockFace::Down,
        BlockFace::Down => BlockFace::North,
        BlockFace::North => BlockFace::Up,
        other => other,
    }
}

const fn x_quarter_normal(normal: [i32; 3]) -> [i32; 3] {
    [normal[0], -normal[2], normal[1]]
}

fn quarter_turn_clockwise(cuboid: &ModelCuboid) -> ModelCuboid {
    let mut rotated = ModelCuboid::new(
        [1.0 - cuboid.max[2], cuboid.min[1], cuboid.min[0]],
        [1.0 - cuboid.min[2], cuboid.max[1], cuboid.max[0]],
    );
    rotated.material_slot = cuboid.material_slot.clone();
    for (face, slot) in &cuboid.face_material_slots {
        rotated
            .face_material_slots
            .insert(clockwise_face(*face), slot.clone());
    }
    for (face, uv) in &cuboid.face_uvs {
        // Rigid model turn: every face's art rotates with the block. The caps already
        // carried this (a directional top texture — the repeater's mechanism grooves, a
        // comparator's arrow — stays put if only the face slot moves); the side faces
        // need it too: a clockwise turn seen from outside a side face reads as a
        // clockwise in-face rotation, and the underside reads counter-clockwise.
        let uv = match face {
            BlockFace::Up => rotate_uv_quarter(*uv, 1),
            BlockFace::Down => rotate_uv_quarter(*uv, 3),
            _ => rotate_uv_quarter(*uv, 1),
        };
        rotated.face_uvs.insert(clockwise_face(*face), uv);
    }
    rotated
}

fn clockwise_face(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::North => BlockFace::East,
        BlockFace::East => BlockFace::South,
        BlockFace::South => BlockFace::West,
        BlockFace::West => BlockFace::North,
        other => other,
    }
}

/// Derives per-face UVs for a cuboid from its block-space bounds.
///
/// The renderer builds each face from outside the block with a fixed corner order. These UVs
/// follow that order: U grows to the image right and V grows down as the face is seen from
/// outside, and V starts at the block top (1 - y). That keeps every texture upright and
/// unmirrored on all six faces, including partially inset elements such as slabs and fences.
#[must_use]
pub fn detail_cuboid_with_local_uv(cuboid: ModelCuboid) -> ModelCuboid {
    let [x0, y0, z0] = cuboid.min;
    let [x1, y1, z1] = cuboid.max;
    let v_top = 1.0 - y1;
    let v_bottom = 1.0 - y0;
    // Faces that already carry explicit UVs (rigid-rotation caps, unfolded art) keep them;
    // the derived projection would silently overwrite the authored orientation.
    let authored = cuboid.face_uvs.clone();
    let derived = move |face: BlockFace, uv: [[f32; 2]; 4]| -> [[f32; 2]; 4] {
        authored.get(&face).copied().unwrap_or(uv)
    };
    cuboid
        .with_face_uv(BlockFace::Up, derived(BlockFace::Up, rect_uv(x0, z0, x1, z1)))
        .with_face_uv(
            BlockFace::Down,
            derived(BlockFace::Down, rect_uv(x0, 1.0 - z1, x1, 1.0 - z0)),
        )
        .with_face_uv(
            BlockFace::North,
            derived(
                BlockFace::North,
                [
                    [1.0 - x0, v_bottom],
                    [1.0 - x1, v_bottom],
                    [1.0 - x1, v_top],
                    [1.0 - x0, v_top],
                ],
            ),
        )
        .with_face_uv(
            BlockFace::South,
            derived(
                BlockFace::South,
                [[x1, v_bottom], [x0, v_bottom], [x0, v_top], [x1, v_top]],
            ),
        )
        .with_face_uv(
            BlockFace::West,
            derived(
                BlockFace::West,
                [[z1, v_bottom], [z0, v_bottom], [z0, v_top], [z1, v_top]],
            ),
        )
        .with_face_uv(
            BlockFace::East,
            derived(
                BlockFace::East,
                [
                    [1.0 - z0, v_bottom],
                    [1.0 - z1, v_bottom],
                    [1.0 - z1, v_top],
                    [1.0 - z0, v_top],
                ],
            ),
        )
}

#[must_use]
pub(crate) fn projected_cuboid_with_uv(cuboid: ModelCuboid) -> ModelCuboid {
    let min = cuboid.min;
    let max = cuboid.max;
    cuboid
        .with_face_uv(
            BlockFace::West,
            rect_uv(min[2], 1.0 - max[1], max[2], 1.0 - min[1]),
        )
        .with_face_uv(
            BlockFace::East,
            rect_uv(1.0 - max[2], 1.0 - max[1], 1.0 - min[2], 1.0 - min[1]),
        )
        .with_face_uv(
            BlockFace::Down,
            rect_uv(1.0 - max[0], 1.0 - max[2], 1.0 - min[0], 1.0 - min[2]),
        )
        .with_face_uv(
            BlockFace::Up,
            rect_uv(1.0 - max[0], min[2], 1.0 - min[0], max[2]),
        )
        .with_face_uv(
            BlockFace::North,
            rect_uv(min[0], 1.0 - max[1], max[0], 1.0 - min[1]),
        )
        .with_face_uv(
            BlockFace::South,
            rect_uv(1.0 - max[0], 1.0 - max[1], 1.0 - min[0], 1.0 - min[1]),
        )
}

#[must_use]
pub(super) fn jmc_face_uv(face: BlockFace, uv: [[f32; 2]; 4]) -> [[f32; 2]; 4] {
    match face {
        BlockFace::South => uv,
        BlockFace::Up => [uv[3], uv[2], uv[1], uv[0]],
        BlockFace::North | BlockFace::East | BlockFace::West | BlockFace::Down => {
            [uv[1], uv[0], uv[3], uv[2]]
        }
        BlockFace::Side | BlockFace::All | BlockFace::Default => uv,
    }
}

#[must_use]
pub(super) fn apply_jmc_box_uv(
    cuboid: ModelCuboid,
    top_uv: [[f32; 2]; 4],
    side_uv: [[f32; 2]; 4],
) -> ModelCuboid {
    cuboid
        .with_face_uv(BlockFace::Up, jmc_face_uv(BlockFace::Up, top_uv))
        .with_face_uv(BlockFace::Down, jmc_face_uv(BlockFace::Down, top_uv))
        .with_face_uv(BlockFace::North, jmc_face_uv(BlockFace::North, side_uv))
        .with_face_uv(BlockFace::South, jmc_face_uv(BlockFace::South, side_uv))
        .with_face_uv(BlockFace::West, jmc_face_uv(BlockFace::West, side_uv))
        .with_face_uv(BlockFace::East, jmc_face_uv(BlockFace::East, side_uv))
}

#[must_use]
pub fn ground_plane(
    corners: [[f32; 3]; 4],
    material_slot: Option<&str>,
    uv: [[f32; 2]; 4],
) -> ModelPlane {
    let plane = ModelPlane::new(corners, [0, 1, 0]).with_uv(uv);
    if let Some(material_slot) = material_slot {
        plane.with_material_slot(material_slot)
    } else {
        plane
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cull_faces_should_remove_outer_boundary_faces() {
        let cuboid =
            detail_cuboid_with_local_uv(ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]))
                .with_face_material_slot(BlockFace::Up, "up")
                .with_face_material_slot(BlockFace::Down, "down")
                .with_face_material_slot(BlockFace::North, "north");
        let shape =
            ModelShape::from_cuboids([cuboid]).cull_faces(&[BlockFace::North, BlockFace::Up]);

        assert!(
            !shape.cuboids[0]
                .face_material_slots
                .contains_key(&BlockFace::North)
        );
        assert!(
            !shape.cuboids[0]
                .face_material_slots
                .contains_key(&BlockFace::Up)
        );
        assert!(
            shape.cuboids[0]
                .face_material_slots
                .contains_key(&BlockFace::Down)
        );
    }

    #[test]
    fn model_planes_are_double_sided_by_default_but_can_be_front_only() {
        let plane = ModelPlane::new(
            [
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            [0, 0, -1],
        );
        assert_eq!(plane.sidedness, ModelPlaneSidedness::DoubleSided);
        assert_eq!(plane.front_only().sidedness, ModelPlaneSidedness::FrontOnly);
    }
}
