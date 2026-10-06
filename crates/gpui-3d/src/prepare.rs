use crate::{
    Aabb, AlphaMode, Camera, CameraError, EvaluatedScene, Light, Mat4, Material, Mesh, MeshPart,
    Node, NodeId, PreparedLight, Scene, Vec2, Vec3,
};
use std::{
    cmp::Ordering,
    sync::{Arc, OnceLock},
};

static DEFAULT_MATERIAL: OnceLock<Arc<Material>> = OnceLock::new();

/// Shared default material used when a mesh part has no material slot on its node.
fn default_material() -> Arc<Material> {
    DEFAULT_MATERIAL
        .get_or_init(|| Arc::new(Material::default()))
        .clone()
}

/// Prepared geometry and material draw.
#[derive(Clone, Debug)]
pub struct PreparedDraw {
    /// Scene node that owns this draw.
    pub node: NodeId,
    /// Immutable geometry.
    pub mesh: Arc<Mesh>,
    /// Indexed range and material slot within the mesh.
    pub part: MeshPart,
    /// Immutable material.
    pub material: Arc<Material>,
    /// World transform.
    pub world: Mat4,
    /// World-space bounds.
    pub bounds: Aabb,
    /// Distance along the camera's forward axis to the bounds center.
    pub view_depth: f32,
    /// Post-projection offset in render-target pixels.
    pub pixel_offset: Vec2,
    /// Signed post-projection depth adjustment in zero-to-one depth units.
    pub depth_bias: f32,
}

/// Consecutive prepared draws that can share one indexed instanced submission.
///
/// Opaque and masked draws are grouped when mesh, mesh part, and complete material values match.
/// Blended draws remain singletons so back-to-front ordering is preserved.
#[derive(Clone, Copy, Debug)]
pub struct DrawBatch<'a> {
    first_instance: usize,
    draws: &'a [PreparedDraw],
}

impl<'a> DrawBatch<'a> {
    /// First index in [`PreparedScene::draws`] used as the GPU base instance.
    pub fn first_instance(self) -> usize {
        self.first_instance
    }

    /// Prepared draws represented by this batch, in their original order.
    pub fn draws(self) -> &'a [PreparedDraw] {
        self.draws
    }
}

struct DrawBatchIter<'a> {
    draws: &'a [PreparedDraw],
    next: usize,
}

impl<'a> Iterator for DrawBatchIter<'a> {
    type Item = DrawBatch<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let first_instance = self.next;
        let first = self.draws.get(first_instance)?;
        self.next += 1;
        if first.material.alpha_mode != AlphaMode::Blend {
            while self.draws.get(self.next).is_some_and(|candidate| {
                candidate.material.alpha_mode != AlphaMode::Blend
                    && same_instance_draw(first, candidate)
            }) {
                self.next += 1;
            }
        }
        Some(DrawBatch {
            first_instance,
            draws: &self.draws[first_instance..self.next],
        })
    }
}

fn same_instance_draw(left: &PreparedDraw, right: &PreparedDraw) -> bool {
    left.mesh.id() == right.mesh.id()
        && left.mesh.generation() == right.mesh.generation()
        && left.part == right.part
        && left.material.as_ref() == right.material.as_ref()
}

/// Camera-culled immutable scene snapshot ready for a renderer.
#[derive(Clone, Debug, Default)]
pub struct PreparedScene {
    /// Opaque and masked draws followed by back-to-front blended draws.
    pub draws: Vec<PreparedDraw>,
    /// Lights in world space.
    pub lights: Vec<PreparedLight>,
    /// Camera view-projection matrix.
    pub view_projection: Mat4,
    walk_stack: Vec<(NodeId, Mat4)>,
    /// Un-culled draws from the last scene walk.
    ///
    /// A camera move keeps this list and only re-culls and re-sorts it, so dragging an orbit camera
    /// does not re-walk the scene graph or rebuild world transforms for every node.
    candidates: Vec<PreparedDraw>,
    camera: Option<Camera>,
    aspect: Option<f32>,
}

impl PreparedScene {
    /// Iterates adjacent draws that can be submitted together with one instance draw.
    ///
    /// The batch records a base index into this prepared scene's draw and instance arrays. The
    /// iterator preserves draw order and performs no allocation. Opaque and masked draws merge only
    /// when mesh, part, and material values match; blended draws are always returned individually.
    pub fn draw_batches(&self) -> impl Iterator<Item = DrawBatch<'_>> + '_ {
        DrawBatchIter {
            draws: &self.draws,
            next: 0,
        }
    }

    /// Builds an immutable scene snapshot and culls geometry outside the camera frustum.
    ///
    /// Every mesh part becomes a draw. Missing material slots use a default white material.
    /// Nodes that contain only transforms group their descendants without producing a draw.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError`] when the camera pose, projection, or aspect ratio is invalid.
    pub fn new(scene: &Scene, camera: Camera, aspect: f32) -> Result<Self, CameraError> {
        let mut prepared = Self::default();
        prepared.update(scene, camera, aspect)?;
        Ok(prepared)
    }

    /// Builds a prepared snapshot from transforms evaluated at one absolute clip time.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError`] for an invalid camera or aspect ratio.
    pub fn new_evaluated(
        scene: &EvaluatedScene<'_>,
        camera: Camera,
        aspect: f32,
    ) -> Result<Self, CameraError> {
        let mut prepared = Self::default();
        prepared.update_evaluated(scene, camera, aspect)?;
        Ok(prepared)
    }

    /// Rebuilds this snapshot while reusing its draw, light, and traversal buffers.
    ///
    /// Geometry and material data remain shared through `Arc`; the prepared vectors are owned by
    /// this value and may be retained across frames to avoid repeated capacity growth.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError`] when the camera pose, projection, or aspect ratio is invalid.
    pub fn update(
        &mut self,
        scene: &Scene,
        camera: Camera,
        aspect: f32,
    ) -> Result<(), CameraError> {
        self.update_with(scene, camera, aspect, Mat4::IDENTITY, |_, node| {
            node.transform
        })
    }

    /// Rebuilds a snapshot from evaluated transforms while reusing retained traversal buffers.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError`] when the camera pose, projection, or aspect ratio is invalid.
    pub fn update_evaluated(
        &mut self,
        scene: &EvaluatedScene<'_>,
        camera: Camera,
        aspect: f32,
    ) -> Result<(), CameraError> {
        self.update_with(scene.source, camera, aspect, Mat4::IDENTITY, |id, node| {
            scene.local_transform(id).unwrap_or(node.transform)
        })
    }

    pub(crate) fn update_transformed(
        &mut self,
        scene: &Scene,
        camera: Camera,
        aspect: f32,
        scene_transform: Mat4,
    ) -> Result<(), CameraError> {
        self.update_with(scene, camera, aspect, scene_transform, |_, node| {
            node.transform
        })
    }

    pub(crate) fn update_transformed_evaluated(
        &mut self,
        scene: &EvaluatedScene<'_>,
        camera: Camera,
        aspect: f32,
        scene_transform: Mat4,
    ) -> Result<(), CameraError> {
        self.update_with(scene.source, camera, aspect, scene_transform, |id, node| {
            scene.local_transform(id).unwrap_or(node.transform)
        })
    }

    /// Re-culls and re-sorts the retained draws for a new camera without re-walking the scene.
    ///
    /// Use this for camera-only changes such as dragging an orbit camera. The draw list comes from
    /// the last [`PreparedScene::update`] or [`PreparedScene::update_evaluated`] call, so animation
    /// and scene changes still require a full update. Returns `false` when no scene walk has
    /// populated the retained draws yet, in which case the caller must update the scene first.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError`] when the camera pose, projection, or aspect ratio is invalid.
    pub fn update_camera(&mut self, camera: Camera, aspect: f32) -> Result<bool, CameraError> {
        if self.aspect.is_none() {
            return Ok(false);
        }
        self.apply_camera(camera, aspect)?;
        Ok(true)
    }

    /// Camera used by the retained draw list, when one has been applied.
    #[must_use]
    pub const fn camera(&self) -> Option<Camera> {
        self.camera
    }

    fn update_with(
        &mut self,
        scene: &Scene,
        camera: Camera,
        aspect: f32,
        scene_transform: Mat4,
        local_transform: impl Fn(NodeId, &Node) -> crate::Transform,
    ) -> Result<(), CameraError> {
        self.walk(
            scene,
            scene_transform,
            local_transform,
            camera,
        )?;
        self.apply_camera(camera, aspect)
    }

    /// Walks the scene graph once and records every non-empty mesh part in world space.
    fn walk(
        &mut self,
        scene: &Scene,
        scene_transform: Mat4,
        local_transform: impl Fn(NodeId, &Node) -> crate::Transform,
        camera: Camera,
    ) -> Result<(), CameraError> {
        self.candidates.clear();
        self.lights.clear();
        self.walk_stack.clear();
        self.walk_stack.extend(
            scene
                .roots()
                .iter()
                .copied()
                .map(|id| (id, scene_transform)),
        );
        let (cull_planes, cull_ready) = match self.aspect {
            Some(aspect) => (
                Some(frustum_planes(camera.view_projection(aspect)?)),
                true,
            ),
            None => (None, false),
        };
        while let Some((id, parent_world)) = self.walk_stack.pop() {
            let Some(node) = scene.node(id) else { continue };
            let world = parent_world * local_transform(id, node).matrix();
            if let Some(light) = node.light {
                self.lights.push(transform_light(light, world));
            }
            if let Some(mesh) = &node.mesh {
                for (part_index, part) in mesh.parts().iter().copied().enumerate() {
                    let Some(bounds) = mesh
                        .part_bounds(part_index)
                        .map(|bounds| bounds.transformed(world))
                    else {
                        continue;
                    };
                    if cull_ready
                        && let Some(planes) = &cull_planes
                        && !visible(bounds, planes)
                    {
                        continue;
                    }
                    let center = (bounds.min + bounds.max) * 0.5;
                    self.candidates.push(PreparedDraw {
                        node: id,
                        mesh: mesh.clone(),
                        part,
                        material: usize::try_from(part.material_slot())
                            .ok()
                            .and_then(|slot| node.materials.get(slot))
                            .cloned()
                            .unwrap_or_else(default_material),
                        world,
                        bounds,
                        view_depth: (center - camera.eye).dot(Vec3::ZERO),
                        pixel_offset: node.pixel_offset(),
                        depth_bias: node.depth_bias(),
                    });
                }
            }
            if let Some(children) = scene.children(id) {
                self.walk_stack
                    .extend(children.iter().rev().copied().map(|child| (child, world)));
            }
        }
        Ok(())
    }

    /// Culls the retained draws for one camera and rebuilds the ordered draw list.
    fn apply_camera(&mut self, camera: Camera, aspect: f32) -> Result<(), CameraError> {
        let view_projection = camera.view_projection(aspect)?;
        let view_direction = (camera.target - camera.eye)
            .normalized()
            .ok_or(CameraError::InvalidPose)?;
        let planes = frustum_planes(view_projection);
        self.draws.clear();
        self.draws.extend(self.candidates.iter().filter_map(|draw| {
            if !visible(draw.bounds, &planes) {
                return None;
            }
            let center = (draw.bounds.min + draw.bounds.max) * 0.5;
            Some(PreparedDraw {
                view_depth: (center - camera.eye).dot(view_direction),
                ..draw.clone()
            })
        }));
        self.draws.sort_by(|left, right| {
            let left_blended = left.material.alpha_mode == crate::AlphaMode::Blend;
            let right_blended = right.material.alpha_mode == crate::AlphaMode::Blend;
            match (left_blended, right_blended) {
                (false, true) => Ordering::Less,
                (true, false) => Ordering::Greater,
                (true, true) => right.view_depth.total_cmp(&left.view_depth),
                (false, false) => left
                    .material
                    .id()
                    .cmp(&right.material.id())
                    .then_with(|| left.mesh.id().cmp(&right.mesh.id())),
            }
        });
        self.view_projection = view_projection;
        self.camera = Some(camera);
        self.aspect = Some(aspect);
        Ok(())
    }

    /// Releases retained traversal and output capacity after a large scene is no longer needed.
    pub fn trim(&mut self) {
        self.draws.shrink_to_fit();
        self.lights.shrink_to_fit();
        self.walk_stack.shrink_to_fit();
    }
}

#[derive(Clone, Copy)]
struct Plane {
    normal: Vec3,
    distance: f32,
}

fn frustum_planes(matrix: Mat4) -> [Plane; 6] {
    let row0 = matrix.row(0);
    let row1 = matrix.row(1);
    let row2 = matrix.row(2);
    let row3 = matrix.row(3);
    [
        plane(add(row3, row0)),
        plane(sub(row3, row0)),
        plane(add(row3, row1)),
        plane(sub(row3, row1)),
        plane(row2),
        plane(sub(row3, row2)),
    ]
}

fn plane(value: [f32; 4]) -> Plane {
    let normal = Vec3::new(value[0], value[1], value[2]);
    let length = normal.length();
    if length <= f32::EPSILON {
        Plane {
            normal,
            distance: value[3],
        }
    } else {
        Plane {
            normal: normal / length,
            distance: value[3] / length,
        }
    }
}

fn add(left: [f32; 4], right: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|index| left[index] + right[index])
}

fn sub(left: [f32; 4], right: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|index| left[index] - right[index])
}

fn visible(bounds: Aabb, planes: &[Plane; 6]) -> bool {
    planes.iter().all(|plane| {
        let positive = Vec3::new(
            if plane.normal.x >= 0.0 {
                bounds.max.x
            } else {
                bounds.min.x
            },
            if plane.normal.y >= 0.0 {
                bounds.max.y
            } else {
                bounds.min.y
            },
            if plane.normal.z >= 0.0 {
                bounds.max.z
            } else {
                bounds.min.z
            },
        );
        plane.normal.dot(positive) + plane.distance >= 0.0
    })
}

fn transform_light(light: Light, world: Mat4) -> PreparedLight {
    match light {
        Light::Directional {
            direction,
            color,
            intensity,
        } => PreparedLight::Directional {
            direction: world
                .transform_vector(direction)
                .normalized()
                .unwrap_or(-Vec3::Z),
            color,
            intensity,
        },
        Light::Point {
            position,
            color,
            intensity,
            range,
        } => PreparedLight::Point {
            position: world.transform_point(position),
            color,
            intensity,
            range: range * max_world_scale(world),
        },
        Light::Spot(light) => PreparedLight::Spot {
            position: world.transform_point(light.position()),
            direction: world
                .transform_vector(light.direction())
                .normalized()
                .unwrap_or(light.direction()),
            color: light.color(),
            intensity: light.intensity(),
            range: light.range() * max_world_scale(world),
            inner_angle: light.cone().inner_angle(),
            outer_angle: light.cone().outer_angle(),
        },
        Light::Ambient { color, intensity } => PreparedLight::Ambient { color, intensity },
    }
}

fn max_world_scale(world: Mat4) -> f32 {
    let columns = world.columns();
    let basis = [
        Vec3::new(columns[0][0], columns[0][1], columns[0][2]),
        Vec3::new(columns[1][0], columns[1][1], columns[1][2]),
        Vec3::new(columns[2][0], columns[2][1], columns[2][2]),
    ];
    let gram = [
        [
            basis[0].dot(basis[0]),
            basis[0].dot(basis[1]),
            basis[0].dot(basis[2]),
        ],
        [
            basis[1].dot(basis[0]),
            basis[1].dot(basis[1]),
            basis[1].dot(basis[2]),
        ],
        [
            basis[2].dot(basis[0]),
            basis[2].dot(basis[1]),
            basis[2].dot(basis[2]),
        ],
    ];
    let largest_eigenvalue_bound = (0..3)
        .map(|row| {
            gram[row][row]
                + (0..3)
                    .filter(|column| *column != row)
                    .map(|column| gram[row][column].abs())
                    .sum::<f32>()
        })
        .fold(0.0_f32, f32::max);
    largest_eigenvalue_bound.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, SpotLight, Transform, Vec2, Vertex};

    fn mesh() -> Arc<Mesh> {
        Arc::new(
            Mesh::new(
                vec![
                    Vertex {
                        position: Vec3::new(-0.5, -0.5, 0.0),
                        normal: Vec3::Z,
                        uv: Vec2::new(0.0, 0.0),
                        color: [1.0; 4],
                    },
                    Vertex {
                        position: Vec3::new(0.5, -0.5, 0.0),
                        normal: Vec3::Z,
                        uv: Vec2::new(1.0, 0.0),
                        color: [1.0; 4],
                    },
                    Vertex {
                        position: Vec3::new(0.0, 0.5, 0.0),
                        normal: Vec3::Z,
                        uv: Vec2::new(0.5, 1.0),
                        color: [1.0; 4],
                    },
                ],
                vec![0, 1, 2],
            )
            .unwrap(),
        )
    }

    #[test]
    fn prepares_parent_transforms_and_culls_outside_meshes() {
        let mut scene = Scene::new();
        let parent = scene
            .insert(
                None,
                Node::new().with_transform(Transform {
                    translation: Vec3::new(0.0, 0.0, -3.0),
                    ..Transform::IDENTITY
                }),
            )
            .unwrap();
        scene
            .insert(Some(parent), Node::new().with_mesh(mesh()))
            .unwrap();
        scene
            .insert(
                None,
                Node::new().with_mesh(mesh()).with_transform(Transform {
                    translation: Vec3::new(100.0, 0.0, -3.0),
                    ..Transform::IDENTITY
                }),
            )
            .unwrap();
        let camera = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 100.0);
        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();
        assert_eq!(prepared.draws.len(), 1);
        assert!((prepared.draws[0].world.transform_point(Vec3::ZERO).z + 3.0).abs() < 1e-5);
        let capacity = prepared.draws.capacity();
        let mut prepared = prepared;
        prepared.update(&scene, camera, 1.0).unwrap();
        assert_eq!(prepared.draws.capacity(), capacity);
    }

    #[test]
    fn draw_batches_merge_compatible_solid_draws_only() {
        let mesh = mesh();
        let opaque = Arc::new(Material::new());
        let mut changed_material = opaque.as_ref().clone();
        changed_material.base_color[0] = 0.5;
        let changed_material = Arc::new(changed_material);
        let mut masked_material = Material::new();
        masked_material.alpha_mode = AlphaMode::Mask;
        let masked = Arc::new(masked_material);
        let mut blended_material = Material::new();
        blended_material.alpha_mode = AlphaMode::Blend;
        let blended = Arc::new(blended_material);
        let mut scene = Scene::new();

        for x in [-1.0, 1.0] {
            scene
                .insert(
                    None,
                    Node::new()
                        .with_mesh(mesh.clone())
                        .with_materials([opaque.clone()])
                        .with_transform(Transform {
                            translation: Vec3::new(x, 0.0, 0.0),
                            ..Transform::IDENTITY
                        }),
                )
                .unwrap();
        }
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_materials([changed_material]),
            )
            .unwrap();
        for material in [masked, blended] {
            for x in [-1.0, 1.0] {
                scene
                    .insert(
                        None,
                        Node::new()
                            .with_mesh(mesh.clone())
                            .with_materials([material.clone()])
                            .with_transform(Transform {
                                translation: Vec3::new(x, 0.0, 0.0),
                                ..Transform::IDENTITY
                            }),
                    )
                    .unwrap();
            }
        }
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();
        let batches = prepared.draw_batches().collect::<Vec<_>>();

        let mut sizes = batches
            .iter()
            .map(|batch| batch.draws().len())
            .collect::<Vec<_>>();
        sizes.sort_unstable();
        assert_eq!(sizes, [1, 1, 1, 2, 2]);

        let mut next_instance = 0;
        for batch in batches {
            assert_eq!(batch.first_instance(), next_instance);
            next_instance += batch.draws().len();
            let first = &batch.draws()[0];
            assert!(batch.draws().iter().all(|draw| {
                first.material.alpha_mode != AlphaMode::Blend && same_instance_draw(first, draw)
                    || std::ptr::eq(first, draw)
            }));
        }
        assert_eq!(next_instance, prepared.draws.len());
    }

    #[test]
    fn prepared_draw_retains_pixel_offset_and_depth_bias() {
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh())
                    .with_transform(Transform {
                        translation: Vec3::new(0.0, 0.0, -3.0),
                        ..Transform::IDENTITY
                    })
                    .with_pixel_offset(Vec2::new(0.45, -0.45))
                    .unwrap()
                    .with_depth_bias(0.002)
                    .unwrap(),
            )
            .unwrap();
        let camera = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 100.0);
        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();

        assert_eq!(prepared.draws[0].pixel_offset, Vec2::new(0.45, -0.45));
        assert_eq!(prepared.draws[0].depth_bias, 0.002);
    }

    #[test]
    fn empty_mesh_does_not_create_a_draw() {
        let empty_mesh = Arc::new(Mesh::new(Vec::<Vertex>::new(), Vec::<u32>::new()).unwrap());
        let mut scene = Scene::new();
        scene
            .insert(None, Node::new().with_mesh(empty_mesh))
            .unwrap();
        let camera = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 100.0);

        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();

        assert!(prepared.draws.is_empty());
    }

    #[test]
    fn prepares_each_mesh_part_with_its_node_material_slot() {
        let first_material = Arc::new(Material::new());
        let second_material = Arc::new(Material::new());
        let mesh = Arc::new(
            Mesh::with_parts(
                vec![
                    Vertex {
                        position: Vec3::new(-0.6, -0.5, 0.0),
                        normal: Vec3::Z,
                        uv: Vec2::ZERO,
                        color: [1.0; 4],
                    },
                    Vertex {
                        position: Vec3::new(-0.1, -0.5, 0.0),
                        normal: Vec3::Z,
                        uv: Vec2::ZERO,
                        color: [1.0; 4],
                    },
                    Vertex {
                        position: Vec3::new(-0.35, 0.2, 0.0),
                        normal: Vec3::Z,
                        uv: Vec2::ZERO,
                        color: [1.0; 4],
                    },
                    Vertex {
                        position: Vec3::new(0.1, -0.5, 0.0),
                        normal: Vec3::Z,
                        uv: Vec2::ZERO,
                        color: [1.0; 4],
                    },
                    Vertex {
                        position: Vec3::new(0.6, -0.5, 0.0),
                        normal: Vec3::Z,
                        uv: Vec2::ZERO,
                        color: [1.0; 4],
                    },
                    Vertex {
                        position: Vec3::new(0.35, 0.2, 0.0),
                        normal: Vec3::Z,
                        uv: Vec2::ZERO,
                        color: [1.0; 4],
                    },
                ],
                vec![0, 1, 2, 3, 4, 5],
                vec![MeshPart::new(0, 3, 0), MeshPart::new(3, 3, 1)],
            )
            .unwrap(),
        );
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh)
                    .with_materials([first_material.clone(), second_material.clone()])
                    .with_transform(crate::Transform {
                        translation: Vec3::new(0.0, 0.0, -3.0),
                        ..crate::Transform::IDENTITY
                    }),
            )
            .unwrap();
        let camera = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 100.0);

        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();

        assert_eq!(prepared.draws.len(), 2);
        for draw in &prepared.draws {
            let expected = match draw.part.material_slot() {
                0 => first_material.id(),
                1 => second_material.id(),
                slot => panic!("unexpected material slot {slot}"),
            };
            assert_eq!(draw.material.id(), expected);
            assert_eq!(draw.part.index_count(), 3);
        }
    }

    #[test]
    fn sorts_blended_draws_by_camera_depth() {
        let mut material = Material::new();
        material.alpha_mode = crate::AlphaMode::Blend;
        let material = Arc::new(material);
        let mut scene = Scene::new();
        let nearer_but_laterally_far = scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh())
                    .with_materials([material.clone()])
                    .with_transform(Transform {
                        translation: Vec3::new(10.0, 0.0, -8.0),
                        ..Transform::IDENTITY
                    }),
            )
            .unwrap();
        let farther = scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh())
                    .with_materials([material])
                    .with_transform(Transform {
                        translation: Vec3::new(0.0, 0.0, -12.0),
                        ..Transform::IDENTITY
                    }),
            )
            .unwrap();
        let camera = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 100.0);

        let prepared = PreparedScene::new(&scene, camera, 4.0).unwrap();

        assert_eq!(prepared.draws.len(), 2);
        assert_eq!(prepared.draws[0].node, farther);
        assert!((prepared.draws[0].view_depth - 12.0).abs() < f32::EPSILON);
        assert_eq!(prepared.draws[1].node, nearer_but_laterally_far);
        assert!((prepared.draws[1].view_depth - 8.0).abs() < f32::EPSILON);
    }

    #[test]
    fn transforms_point_light_range_into_world_units() {
        let mut scene = Scene::new();
        let parent = scene
            .insert(
                None,
                Node::new().with_transform(Transform {
                    scale: Vec3::new(3.0, 1.0, 2.0),
                    ..Transform::IDENTITY
                }),
            )
            .unwrap();
        scene
            .insert(
                Some(parent),
                Node::new().with_light(Light::Point {
                    position: Vec3::ZERO,
                    color: [1.0; 3],
                    intensity: 1.0,
                    range: 2.0,
                }),
            )
            .unwrap();
        let camera = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 100.0);

        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();

        assert_eq!(
            prepared.lights,
            [PreparedLight::Point {
                position: Vec3::ZERO,
                color: [1.0; 3],
                intensity: 1.0,
                range: 6.0,
            }]
        );
    }

    #[test]
    fn transforms_spot_light_pose_and_range_into_world_space() {
        let mut scene = Scene::new();
        let parent = scene
            .insert(
                None,
                Node::new().with_transform(Transform {
                    translation: Vec3::new(1.0, 2.0, 3.0),
                    scale: Vec3::new(3.0, 3.0, 3.0),
                    ..Transform::IDENTITY
                }),
            )
            .unwrap();
        let cone = crate::SpotCone::new(0.2, 0.5).unwrap();
        let spot = SpotLight::new(Vec3::new(0.0, 0.0, 1.0), -Vec3::Z, 2.0, cone).unwrap();
        scene
            .insert(Some(parent), Node::new().with_light(Light::Spot(spot)))
            .unwrap();
        let camera = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 100.0);

        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();

        assert_eq!(
            prepared.lights,
            [PreparedLight::Spot {
                position: Vec3::new(1.0, 2.0, 6.0),
                direction: -Vec3::Z,
                color: [1.0; 3],
                intensity: 1.0,
                range: 6.0,
                inner_angle: 0.2,
                outer_angle: 0.5,
            }]
        );
    }

    #[test]
    fn prepared_draws_and_raycast_share_the_evaluated_transform() {
        use crate::{Keyframe, Ray, Vec3Track};
        use std::time::Duration;

        let mut scene = Scene::new();
        let node = scene
            .insert(None, Node::new().with_mesh(Arc::new(Mesh::cube())))
            .unwrap();
        let translation = Vec3Track::new([
            Keyframe::new(Duration::ZERO, Vec3::ZERO),
            Keyframe::new(Duration::from_secs(1), Vec3::new(5.0, 0.0, 0.0)),
        ])
        .unwrap();
        let track = crate::TransformTrack::new(node).with_translation(translation);
        let evaluated = scene.evaluate(&[track], Duration::from_secs(1)).unwrap();
        let camera = Camera::perspective(
            Vec3::new(5.0, 0.0, 3.0),
            Vec3::new(5.0, 0.0, 0.0),
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let prepared = PreparedScene::new_evaluated(&evaluated, camera, 1.0).unwrap();
        assert_eq!(prepared.draws.len(), 1);
        assert_eq!(
            prepared.draws[0].world.transform_point(Vec3::ZERO),
            Vec3::new(5.0, 0.0, 0.0)
        );

        let ray = Ray::new(Vec3::new(5.0, 0.0, 3.0), -Vec3::Z).unwrap();
        let mut scratch = crate::RaycastScratch::new();
        assert!(evaluated.raycast(ray, &mut scratch).unwrap().is_some());
        assert!(scene.raycast(ray, &mut scratch).unwrap().is_none());
        assert_eq!(scene.node(node).unwrap().transform.translation, Vec3::ZERO);
    }
}
