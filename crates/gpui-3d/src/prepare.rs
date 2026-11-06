use crate::{
    Aabb, AlphaMode, Camera, CameraError, EvaluatedScene, Light, Mat4, Material, Mesh, MeshPart,
    Node, NodeId, PreparedLight, Scene, Vec2, Vec3,
};
use std::{
    cmp::Ordering,
    sync::{Arc, OnceLock},
};

static DEFAULT_MATERIAL: OnceLock<Arc<Material>> = OnceLock::new();
mod incremental;
mod occluders;

/// Applies a new-position -> previous-position permutation using reusable cycle storage.
pub(crate) fn apply_order<T>(values: &mut [T], order: &[usize], positions: &mut Vec<usize>) {
    debug_assert_eq!(values.len(), order.len());
    positions.resize(order.len(), 0);
    for (next, &previous) in order.iter().enumerate() {
        positions[previous] = next;
    }
    for index in 0..order.len() {
        while positions[index] != index {
            let target = positions[index];
            values.swap(index, target);
            positions.swap(index, target);
        }
    }
}

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
    /// World-space node origin when optional scene-view bounds apply to this draw.
    pub culling_origin: Option<Vec3>,
    /// Distance along the camera's forward axis to the bounds center.
    pub view_depth: f32,
    /// Post-projection offset in render-target pixels.
    pub pixel_offset: Vec2,
    /// Signed post-projection depth adjustment in zero-to-one depth units.
    pub depth_bias: f32,
}

impl PreparedDraw {
    pub(crate) fn is_mirrored(&self) -> bool {
        let x = self.world.transform_vector(Vec3::X);
        let y = self.world.transform_vector(Vec3::Y);
        let z = self.world.transform_vector(Vec3::Z);
        x.cross(y).dot(z) < 0.0
    }

    pub(crate) fn raster_front_face(&self) -> Option<gfx_core::FrontFace> {
        if self.material.double_sided {
            return None;
        }
        Some(match (self.material.front_face, self.is_mirrored()) {
            (gfx_core::FrontFace::Ccw, false) | (gfx_core::FrontFace::Cw, true) => {
                gfx_core::FrontFace::Ccw
            }
            _ => gfx_core::FrontFace::Cw,
        })
    }
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
        && (left.material.double_sided || left.is_mirrored() == right.is_mirrored())
}

/// Camera-culled immutable scene snapshot ready for a renderer.
#[derive(Clone, Debug, Default)]
pub struct PreparedScene {
    occluders: std::collections::HashMap<NodeId, (Option<Vec3>, Arc<[crate::Occluder]>)>,
    /// Opaque and masked draws followed by back-to-front blended draws.
    pub draws: Vec<PreparedDraw>,
    /// Lights in world space.
    pub lights: Vec<PreparedLight>,
    /// Camera view-projection matrix.
    pub view_projection: Mat4,
    walk_stack: Vec<(NodeId, Mat4, bool)>,
    /// Un-culled draws from the last scene walk.
    ///
    /// A camera move keeps this list and only re-culls and re-sorts it, so dragging an orbit camera
    /// does not re-walk the scene graph or rebuild world transforms for every node.
    candidates: Vec<PreparedDraw>,
    /// Reusable workspace for linear incremental scene updates.
    candidate_scratch: Vec<PreparedDraw>,
    // Draws from the last successful incremental update. Only these leaves need
    // resource validation and uploads when the texture table and shader mode stay stable.
    changed_asset_draws: Vec<PreparedDraw>,
    candidate_groups: Vec<CandidateGroup>,
    /// Stable lookup for sparse Section deltas; recomputed only when the
    /// structural order of candidate groups changes.
    candidate_group_index: std::collections::HashMap<NodeId, usize>,
    visible_candidates: Vec<usize>,
    next_visible_candidates: Vec<usize>,
    camera: Option<Camera>,
    aspect: Option<f32>,
    lod_active: bool,
    culling_bounds: Option<Aabb>,
    gpu_culling: bool,
    gpu_candidates_dirty: bool,
    /// The resident GPU draw array follows candidate order only for an
    /// entirely opaque GPU-culled scene. Transparent ordering and CPU
    /// visibility never use the direct Section->Draw Slot path.
    opaque_resident_order: bool,
    /// Dirty logical draws from the most recent fixed-layout Section Delta.
    /// Empty means no changed GPU record; None means a full update is needed.
    resident_delta_draws: Option<Vec<usize>>,
    blend_order: Vec<usize>,
    blend_positions: Vec<usize>,
}

#[derive(Clone, Debug)]
struct CandidateGroup {
    start: usize,
    end: usize,
    bounds: Aabb,
    culling_origin: Option<Vec3>,
}

impl PreparedScene {
    pub(crate) fn blended_order(&self) -> &[usize] {
        &self.blend_order
    }

    /// Exact logical indices needing GPU resident record refresh. Available
    /// only when every retained opaque Draw keeps its previous logical slot.
    pub(crate) fn resident_delta_draws(&self) -> Option<&[usize]> {
        self.resident_delta_draws.as_deref()
    }

    /// Native Quad batching can permute opaque draws away from the candidate
    /// order. Such a scene must not issue logical Candidate indices directly
    /// as GPU Draw Slot indices until the next full camera rebuild.
    pub(crate) fn invalidate_resident_candidate_order(&mut self) {
        self.opaque_resident_order = false;
        self.resident_delta_draws = None;
    }
    /// Only visible leaf draws introduced or updated by the last incremental scene delta.
    pub(crate) fn incremental_asset_draws(&self) -> &[PreparedDraw] {
        &self.changed_asset_draws
    }
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

    /// Whether the retained candidates selected a distance-based LOD mesh.
    ///
    /// Camera-only retained updates must re-walk the scene when this is true because LOD selection
    /// itself depends on the camera pose.
    #[must_use]
    pub const fn uses_lod(&self) -> bool {
        self.lod_active
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

    pub(crate) fn set_culling_bounds(&mut self, bounds: Option<Aabb>) {
        self.gpu_candidates_dirty |= !self.gpu_culling && self.culling_bounds != bounds;
        self.culling_bounds = bounds;
    }

    pub(crate) fn set_gpu_culling(&mut self, enabled: bool) -> bool {
        let changed = self.gpu_culling != enabled;
        self.gpu_culling = enabled;
        self.gpu_candidates_dirty |= changed;
        if changed {
            self.opaque_resident_order = false;
            self.resident_delta_draws = None;
        }
        changed
    }

    /// Camera used by the retained draw list, when one has been applied.
    #[must_use]
    pub const fn camera(&self) -> Option<Camera> {
        self.camera
    }

    #[cfg(test)]
    fn is_bounds_visible(&self, bounds: Aabb) -> bool {
        self.bounds_visibility()(bounds)
    }

    pub(crate) fn bounds_visibility(&self) -> impl Fn(Aabb) -> bool {
        let planes = frustum_planes(self.view_projection);
        move |bounds| visible(bounds, &planes)
    }

    pub(crate) fn is_draw_in_culling_bounds(&self, draw: &PreparedDraw) -> bool {
        !draw
            .culling_origin
            .zip(self.culling_bounds)
            .is_some_and(|(origin, bounds)| !contains_origin(bounds, origin))
    }

    fn update_with(
        &mut self,
        scene: &Scene,
        camera: Camera,
        aspect: f32,
        scene_transform: Mat4,
        local_transform: impl Fn(NodeId, &Node) -> crate::Transform,
    ) -> Result<(), CameraError> {
        self.walk(scene, scene_transform, local_transform, camera)?;
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
        self.changed_asset_draws.clear();
        self.occluders.clear();
        self.blend_order.clear();
        self.blend_positions.clear();
        self.gpu_candidates_dirty = true;
        self.opaque_resident_order = false;
        self.resident_delta_draws = None;
        self.candidate_groups.clear();
        self.candidate_group_index.clear();
        self.draws.clear();
        self.visible_candidates.clear();
        self.lights.clear();
        self.walk_stack.clear();
        self.lod_active = false;
        self.walk_stack.extend(
            scene
                .roots()
                .iter()
                .copied()
                .map(|id| (id, scene_transform, true)),
        );
        self.walk_pending(scene, local_transform, camera)
    }

    fn walk_pending(
        &mut self,
        scene: &Scene,
        local_transform: impl Fn(NodeId, &Node) -> crate::Transform,
        camera: Camera,
    ) -> Result<(), CameraError> {
        while let Some((id, parent_world, parent_visible)) = self.walk_stack.pop() {
            let Some(node) = scene.node(id) else { continue };
            // Hidden nodes hide their whole subtree; skip traversal instead of emitting draws and
            // lights that the renderer would have to discard later.
            if !parent_visible || !node.visible {
                continue;
            }
            let world = parent_world * local_transform(id, node).matrix();
            self.record_occluders(id, node, world);
            if let Some(light) = node.light {
                self.lights.push(transform_light(light, world));
            }
            let level_materials: &[Arc<crate::Material>];
            let selected_mesh = if node.lod.is_empty() {
                level_materials = &node.materials;
                node.mesh.clone()
            } else {
                self.lod_active = true;
                let world_position = world.transform_point(Vec3::ZERO);
                let distance = (world_position - camera.eye).length();
                let level = node
                    .lod
                    .iter()
                    .find(|level| distance <= level.max_view_distance)
                    .or_else(|| node.lod.last())
                    .expect("non-empty LOD chain always has a fallback level");
                level_materials = if level.materials.is_empty() {
                    &node.materials
                } else {
                    &level.materials
                };
                Some(level.mesh.clone())
            };
            if let Some(mesh) = selected_mesh {
                let group_start = self.candidates.len();
                for (part_index, part) in mesh.parts().iter().copied().enumerate() {
                    let Some(bounds) = mesh
                        .part_bounds(part_index)
                        .map(|bounds| bounds.transformed(world))
                    else {
                        continue;
                    };
                    let center = (bounds.min + bounds.max) * 0.5;
                    self.candidates.push(PreparedDraw {
                        node: id,
                        mesh: mesh.clone(),
                        part,
                        material: usize::try_from(part.material_slot())
                            .ok()
                            .and_then(|slot| level_materials.get(slot))
                            .cloned()
                            .unwrap_or_else(default_material),
                        world,
                        bounds,
                        culling_origin: node
                            .cull_by_origin
                            .then(|| world.transform_point(Vec3::ZERO)),
                        view_depth: (center - camera.eye).dot(Vec3::ZERO),
                        pixel_offset: node.pixel_offset(),
                        depth_bias: node.depth_bias(),
                    });
                }
                if group_start < self.candidates.len()
                    && let Some(bounds) = mesh.bounds()
                {
                    let group_index = self.candidate_groups.len();
                    self.candidate_groups.push(CandidateGroup {
                        start: group_start,
                        end: self.candidates.len(),
                        bounds: bounds.transformed(world),
                        culling_origin: node
                            .cull_by_origin
                            .then(|| world.transform_point(Vec3::ZERO)),
                    });
                    self.candidate_group_index.insert(id, group_index);
                }
            }
            if let Some(children) = scene.children(id) {
                self.walk_stack.extend(
                    children
                        .iter()
                        .rev()
                        .copied()
                        .map(|child| (child, world, true)),
                );
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
        if self.gpu_culling && !self.gpu_candidates_dirty {
            self.sort_blended_draws(camera, view_direction);
            self.view_projection = view_projection;
            self.camera = Some(camera);
            self.aspect = Some(aspect);
            return Ok(());
        }
        let planes = frustum_planes(view_projection);
        self.next_visible_candidates.clear();
        for group in &self.candidate_groups {
            if (!self.gpu_culling && !visible(group.bounds, &planes))
                || (!self.gpu_culling
                    && group
                        .culling_origin
                        .zip(self.culling_bounds)
                        .is_some_and(|(origin, bounds)| !contains_origin(bounds, origin)))
            {
                continue;
            }
            for index in group.start..group.end {
                if self.gpu_culling || visible(self.candidates[index].bounds, &planes) {
                    self.next_visible_candidates.push(index);
                }
            }
        }
        if self.next_visible_candidates == self.visible_candidates
            && self.draws.len() == self.visible_candidates.len()
        {
            for draw in &mut self.draws {
                if self.gpu_culling && draw.material.alpha_mode != AlphaMode::Blend {
                    continue;
                }
                let center = (draw.bounds.min + draw.bounds.max) * 0.5;
                draw.view_depth = (center - camera.eye).dot(view_direction);
            }
            let blended_start = self
                .draws
                .partition_point(|draw| draw.material.alpha_mode != crate::AlphaMode::Blend);
            self.draws[blended_start..]
                .sort_by(|left, right| right.view_depth.total_cmp(&left.view_depth));
        } else {
            self.draws.clear();
            if self.gpu_culling {
                // The GPU owns visibility and opaque/alpha-test depth ordering.
                // A streaming Section delta only needs a stable partition:
                // preserve the native opaque candidate order without sorting
                // 100K+ records or computing per-opaque CPU view depths.
                self.draws.extend(
                    self.next_visible_candidates.iter()
                        .map(|&index| &self.candidates[index])
                        .filter(|draw| draw.material.alpha_mode != crate::AlphaMode::Blend)
                        .cloned(),
                );
                let blended_start = self.draws.len();
                self.draws.extend(
                    self.next_visible_candidates.iter()
                        .map(|&index| &self.candidates[index])
                        .filter(|draw| draw.material.alpha_mode == crate::AlphaMode::Blend)
                        .map(|draw| {
                            let center = (draw.bounds.min + draw.bounds.max) * 0.5;
                            PreparedDraw {
                                view_depth: (center - camera.eye).dot(view_direction),
                                ..draw.clone()
                            }
                        }),
                );
                // Only the transparent tail needs a CPU back-to-front order.
                self.draws[blended_start..].sort_by(|left, right| {
                    right.view_depth.total_cmp(&left.view_depth)
                });
            } else {
                // CPU culling keeps the original material/mesh order.
                self.draws.extend(self.next_visible_candidates.iter().map(|&index| {
                    let draw = &self.candidates[index];
                    let center = (draw.bounds.min + draw.bounds.max) * 0.5;
                    PreparedDraw {
                        view_depth: (center - camera.eye).dot(view_direction),
                        ..draw.clone()
                    }
                }));
                self.draws.sort_by(|left, right| {
                    let left_blended = left.material.alpha_mode == crate::AlphaMode::Blend;
                    let right_blended = right.material.alpha_mode == crate::AlphaMode::Blend;
                    match (left_blended, right_blended) {
                        (false, true) => Ordering::Less,
                        (true, false) => Ordering::Greater,
                        (true, true) => right.view_depth.total_cmp(&left.view_depth),
                        (false, false) => left.material.id().cmp(&right.material.id())
                            .then_with(|| left.mesh.id().cmp(&right.mesh.id())),
                    }
                });
            }
            std::mem::swap(
                &mut self.visible_candidates,
                &mut self.next_visible_candidates,
            );
        }
        self.view_projection = view_projection;
        self.camera = Some(camera);
        self.aspect = Some(aspect);
        self.gpu_candidates_dirty = false;
        // Full GPU culling emits every candidate, and an entirely opaque
        // scene preserves their identity/order. Compute this only on an
        // already-full publication; sparse deltas keep the proven state.
        self.opaque_resident_order = self.gpu_culling
            && self.draws.len() == self.candidates.len()
            && self.draws.iter().all(|draw| draw.material.alpha_mode != AlphaMode::Blend);
        self.resident_delta_draws = None;
        Ok(())
    }

    fn sort_blended_draws(&mut self, camera: Camera, direction: Vec3) {
        let start = self
            .draws
            .partition_point(|draw| draw.material.alpha_mode != AlphaMode::Blend);
        for draw in &mut self.draws[start..] {
            let center = (draw.bounds.min + draw.bounds.max) * 0.5;
            draw.view_depth = (center - camera.eye).dot(direction);
        }
        let draws = &mut self.draws[start..];
        self.blend_order.clear();
        self.blend_order.extend(0..draws.len());
        // Small camera changes often preserve depth order. Keep the identity
        // mapping without sorting or touching the draw permutation workspace.
        if draws
            .windows(2)
            .all(|pair| pair[0].view_depth.total_cmp(&pair[1].view_depth) != Ordering::Less)
        {
            return;
        }
        self.blend_order.sort_unstable_by(|&left, &right| {
            draws[right]
                .view_depth
                .total_cmp(&draws[left].view_depth)
                .then(left.cmp(&right))
        });
        apply_order(draws, &self.blend_order, &mut self.blend_positions);
    }

    /// Releases retained traversal and output capacity after a large scene is no longer needed.
    pub fn trim(&mut self) {
        self.draws.shrink_to_fit();
        self.candidates.shrink_to_fit();
        self.candidate_scratch.shrink_to_fit();
        self.changed_asset_draws.shrink_to_fit();
        self.lights.shrink_to_fit();
        self.walk_stack.shrink_to_fit();
        self.candidate_groups.shrink_to_fit();
        self.visible_candidates.shrink_to_fit();
        self.next_visible_candidates.shrink_to_fit();
        self.blend_order.shrink_to_fit();
        self.blend_positions.shrink_to_fit();
    }
}

fn contains_origin(bounds: Aabb, origin: Vec3) -> bool {
    origin.x >= bounds.min.x
        && origin.x < bounds.max.x
        && origin.y >= bounds.min.y
        && origin.y < bounds.max.y
        && origin.z >= bounds.min.z
        && origin.z < bounds.max.z
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
        Light::Spherical {
            position,
            color,
            intensity,
            range,
            radius,
            diffuse_only,
        } => PreparedLight::Spherical {
            position: world.transform_point(position),
            color,
            intensity,
            range: range * max_world_scale(world),
            radius: radius * max_world_scale(world),
            diffuse_only,
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
    use crate::{LodLevel, Node, SpotLight, Transform, Vec2, Vertex};

    #[test]
    fn gpu_streaming_stably_partitions_opaque_and_sorts_only_transparent() {
        let camera = Camera::perspective(
            Vec3::new(0., 0., 20.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.,
        );
        let mut scene = Scene::new();
        let opaque = Arc::new(crate::Material::new());
        let mut transparent = crate::Material::new();
        transparent.alpha_mode = crate::AlphaMode::Blend;
        let transparent = Arc::new(transparent);
        let mesh = mesh();
        for index in 0..48 {
            let material = if index % 3 == 0 { transparent.clone() } else { opaque.clone() };
            scene.insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_materials([material])
                    .with_transform(Transform {
                        translation: Vec3::new((index % 5) as f32, 0., -(index as f32)),
                        ..Transform::IDENTITY
                    }),
            ).unwrap();
        }
        let mut prepared = PreparedScene::default();
        prepared.set_gpu_culling(true);
        prepared.update(&scene, camera, 1.).unwrap();

        // Baseline: the previous stable full-record sort of GPU candidates.
        let direction = (camera.target - camera.eye).normalized().unwrap();
        let mut expected = prepared.candidates.clone();
        for draw in &mut expected {
            let center = (draw.bounds.min + draw.bounds.max) * 0.5;
            draw.view_depth = (center - camera.eye).dot(direction);
        }
        expected.sort_by(|a, b| {
            let ab = a.material.alpha_mode == crate::AlphaMode::Blend;
            let bb = b.material.alpha_mode == crate::AlphaMode::Blend;
            match (ab, bb) {
                (false, true) => Ordering::Less,
                (true, false) => Ordering::Greater,
                (true, true) => b.view_depth.total_cmp(&a.view_depth),
                (false, false) => Ordering::Equal,
            }
        });
        assert_eq!(
            prepared.draws.iter().map(|draw| draw.node).collect::<Vec<_>>(),
            expected.iter().map(|draw| draw.node).collect::<Vec<_>>(),
        );
        let start = prepared.draws.partition_point(|draw| draw.material.alpha_mode != crate::AlphaMode::Blend);
        assert!(prepared.draws[..start].iter().all(|draw| draw.material.alpha_mode != crate::AlphaMode::Blend));
        assert!(prepared.draws[start..].windows(2).all(|pair| pair[0].view_depth >= pair[1].view_depth));
    }

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
    fn gpu_camera_retains_candidates_and_refreshes_changed_culling_bounds() {
        let mut scene = Scene::new();
        for x in [0.0, 100.0] {
            let origin = Vec3::new(x, 0.0, -3.0);
            scene
                .insert(
                    None,
                    Node::new()
                        .with_mesh(mesh())
                        .with_culling_origin()
                        .with_transform(Transform {
                            translation: origin,
                            ..Transform::IDENTITY
                        }),
                )
                .unwrap();
        }
        let camera = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 200.0);
        let mut prepared = PreparedScene::default();
        prepared.set_gpu_culling(true);
        prepared.update(&scene, camera, 1.0).unwrap();
        assert_eq!(
            prepared.draws.len(),
            2,
            "GPU culling must retain offscreen candidates"
        );
        assert_eq!(
            prepared
                .draws
                .iter()
                .filter(|draw| prepared.is_bounds_visible(draw.bounds))
                .count(),
            1,
            "direct indexed fallbacks still need a valid CPU frustum test"
        );
        let nodes = prepared
            .draws
            .iter()
            .map(|draw| draw.node)
            .collect::<Vec<_>>();
        let turned = Camera::perspective(Vec3::ZERO, Vec3::Z, Vec3::Y, 1.0, 0.1, 200.0);
        prepared.update_camera(turned, 1.0).unwrap();
        assert!(
            prepared
                .draws
                .iter()
                .all(|draw| !prepared.is_bounds_visible(draw.bounds))
        );
        assert_eq!(
            prepared
                .draws
                .iter()
                .map(|draw| draw.node)
                .collect::<Vec<_>>(),
            nodes
        );
        prepared.set_culling_bounds(Aabb::new(
            Vec3::new(-16.0, -16.0, -16.0),
            Vec3::new(16.0, 16.0, 16.0),
        ));
        prepared.update_camera(turned, 1.0).unwrap();
        assert_eq!(
            prepared.draws.len(),
            2,
            "GPU range changes must preserve resident candidates"
        );
        assert_eq!(
            prepared
                .draws
                .iter()
                .filter(|draw| prepared.is_draw_in_culling_bounds(draw))
                .count(),
            1
        );
        prepared.set_gpu_culling(false);
        prepared.update_camera(camera, 1.0).unwrap();
        assert_eq!(
            prepared.draws.len(),
            1,
            "CPU fallback must restore normal frustum culling"
        );
        prepared.set_culling_bounds(None);
        prepared.update_camera(turned, 1.0).unwrap();
        assert!(prepared.draws.is_empty());
    }

    #[test]
    fn gpu_blend_permutation_replays_camera_order_and_keeps_equal_depth_stable() {
        let mut scene = Scene::new();
        let mut material = Material::new();
        material.alpha_mode = AlphaMode::Blend;
        let material = Arc::new(material);
        scene.insert(None, Node::new().with_mesh(mesh())).unwrap();
        for z in [-2.0, -4.0, -4.0, -6.0] {
            scene
                .insert(
                    None,
                    Node::new()
                        .with_mesh(mesh())
                        .with_materials([material.clone()])
                        .with_transform(Transform {
                            translation: Vec3::new(0.0, 0.0, z),
                            ..Transform::IDENTITY
                        }),
                )
                .unwrap();
        }
        let front = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 100.0);
        let back = Camera::perspective(Vec3::ZERO, Vec3::Z, Vec3::Y, 1.0, 0.1, 100.0);
        let mut prepared = PreparedScene::default();
        prepared.set_gpu_culling(true);
        prepared.update(&scene, front, 1.0).unwrap();
        prepared
            .update_camera(
                Camera {
                    target: Vec3::new(0.01, 0.0, -1.0),
                    ..front
                },
                1.0,
            )
            .unwrap();
        assert_eq!(prepared.blended_order(), [0, 1, 2, 3]);
        assert!(prepared.blend_positions.is_empty());
        let opaque = prepared.draws[0].node;
        let mut positions = Vec::new();
        for camera in [back, front, front] {
            let mut previous = prepared.draws[1..]
                .iter()
                .map(|draw| draw.node)
                .collect::<Vec<_>>();
            prepared.update_camera(camera, 1.0).unwrap();
            assert_eq!(prepared.draws[0].node, opaque);
            apply_order(&mut previous, prepared.blended_order(), &mut positions);
            assert_eq!(
                previous,
                prepared.draws[1..]
                    .iter()
                    .map(|draw| draw.node)
                    .collect::<Vec<_>>()
            );
            assert!(
                prepared.draws[1..]
                    .windows(2)
                    .all(|pair| pair[0].view_depth >= pair[1].view_depth)
            );
        }
        assert_eq!(prepared.blended_order(), [0, 1, 2, 3]);
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
    fn gpu_preparation_preserves_opaque_order_and_sorts_blended_depth() {
        let low = Arc::new(Material::new());
        let high = Arc::new(Material::new());
        let mut transparent = Material::new();
        transparent.alpha_mode = AlphaMode::Blend;
        let transparent = Arc::new(transparent);
        let mut scene = Scene::new();
        for (material, z) in [
            (low.clone(), -4.0),
            (high.clone(), -5.0),
            (transparent.clone(), -6.0),
            (transparent, -9.0),
        ] {
            scene
                .insert(
                    None,
                    Node::new()
                        .with_mesh(mesh())
                        .with_materials([material])
                        .with_transform(Transform {
                            translation: Vec3::new(0.0, 0.0, z),
                            ..Transform::IDENTITY
                        }),
                )
                .unwrap();
        }
        let camera = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 100.0);
        let mut prepared = PreparedScene::default();
        prepared.set_gpu_culling(true);
        prepared.update(&scene, camera, 1.0).unwrap();
        assert_eq!(
            prepared
                .draws
                .iter()
                .map(|draw| draw.material.id())
                .take(2)
                .collect::<Vec<_>>(),
            [high.id(), low.id()]
        );
        assert!(prepared.draws[2].view_depth > prepared.draws[3].view_depth);
        prepared.set_gpu_culling(false);
        prepared.update(&scene, camera, 1.0).unwrap();
        assert_eq!(
            prepared
                .draws
                .iter()
                .map(|draw| draw.material.id())
                .take(2)
                .collect::<Vec<_>>(),
            [low.id(), high.id()]
        );
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
    fn camera_update_reuses_visible_draws_and_resorts_blended_depth() {
        let mut material = Material::new();
        material.alpha_mode = AlphaMode::Blend;
        let material = Arc::new(material);
        let mut scene = Scene::new();
        for x in [-2.0, 2.0] {
            scene
                .insert(
                    None,
                    Node::new()
                        .with_mesh(mesh())
                        .with_materials([material.clone()])
                        .with_transform(Transform {
                            translation: Vec3::new(x, 0.0, -10.0),
                            ..Transform::IDENTITY
                        }),
                )
                .unwrap();
        }
        let left_camera = Camera::perspective(
            Vec3::ZERO,
            Vec3::new(-1.0, 0.0, -10.0),
            Vec3::Y,
            1.5,
            0.1,
            100.0,
        );
        let right_camera = Camera::perspective(
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, -10.0),
            Vec3::Y,
            1.5,
            0.1,
            100.0,
        );
        let mut prepared = PreparedScene::new(&scene, left_camera, 2.0).unwrap();
        let before = prepared
            .draws
            .iter()
            .map(|draw| draw.node)
            .collect::<Vec<_>>();
        assert_eq!(prepared.visible_candidates.len(), 2);
        assert!(prepared.update_camera(right_camera, 2.0).unwrap());
        let after = prepared
            .draws
            .iter()
            .map(|draw| draw.node)
            .collect::<Vec<_>>();
        assert_eq!(after, before.into_iter().rev().collect::<Vec<_>>());
        let rebuilt = PreparedScene::new(&scene, right_camera, 2.0).unwrap();
        assert_eq!(
            after,
            rebuilt
                .draws
                .iter()
                .map(|draw| draw.node)
                .collect::<Vec<_>>()
        );
        prepared.draws.clear();
        assert!(prepared.update_camera(right_camera, 2.0).unwrap());
        assert_eq!(prepared.draws.len(), 2);
    }

    #[test]
    fn changing_culling_bounds_reculls_retained_scene() {
        let mut scene = Scene::new();
        let mut nodes = Vec::new();
        for x in [-2.0, 2.0] {
            nodes.push(
                scene
                    .insert(
                        None,
                        Node::new()
                            .with_mesh(mesh())
                            .with_culling_origin()
                            .with_transform(Transform {
                                translation: Vec3::new(x, 0.0, -10.0),
                                ..Transform::IDENTITY
                            }),
                    )
                    .unwrap(),
            );
        }
        let unbounded = scene
            .insert(
                None,
                Node::new().with_mesh(mesh()).with_transform(Transform {
                    translation: Vec3::new(0.0, 0.0, -10.0),
                    ..Transform::IDENTITY
                }),
            )
            .unwrap();
        let camera = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.5, 0.1, 100.0);
        let mut prepared = PreparedScene::new(&scene, camera, 2.0).unwrap();
        assert_eq!(prepared.draws.len(), 3);
        let left_bounds =
            Aabb::new(Vec3::new(-4.0, -2.0, -12.0), Vec3::new(0.0, 2.0, -8.0)).unwrap();
        assert!(contains_origin(left_bounds, Vec3::new(-4.0, 0.0, -10.0)));
        assert!(!contains_origin(left_bounds, Vec3::new(0.0, 0.0, -10.0)));
        prepared.set_culling_bounds(Some(left_bounds));
        prepared.update_camera(camera, 2.0).unwrap();
        assert_eq!(prepared.draws.len(), 2);
        assert!(prepared.draws.iter().any(|draw| draw.node == nodes[0]));
        assert!(prepared.draws.iter().any(|draw| draw.node == unbounded));
        prepared.set_culling_bounds(Some(
            Aabb::new(Vec3::new(0.0, -2.0, -12.0), Vec3::new(4.0, 2.0, -8.0)).unwrap(),
        ));
        prepared.update_camera(camera, 2.0).unwrap();
        assert_eq!(prepared.draws.len(), 2);
        assert!(prepared.draws.iter().any(|draw| draw.node == nodes[1]));
        assert!(prepared.draws.iter().any(|draw| draw.node == unbounded));
        prepared.set_culling_bounds(None);
        prepared.update_camera(camera, 2.0).unwrap();
        assert_eq!(prepared.draws.len(), 3);
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

    #[test]
    fn lod_levels_select_by_camera_distance_and_work_without_an_authored_mesh() {
        let near_mesh = mesh();
        let far_mesh = Arc::new(
            Mesh::new(
                vec![
                    Vertex {
                        position: Vec3::new(-1.0, -1.0, 0.0),
                        normal: Vec3::Z,
                        uv: Vec2::new(0.0, 0.0),
                        color: [1.0; 4],
                    },
                    Vertex {
                        position: Vec3::new(1.0, -1.0, 0.0),
                        normal: Vec3::Z,
                        uv: Vec2::new(1.0, 0.0),
                        color: [1.0; 4],
                    },
                    Vertex {
                        position: Vec3::new(0.0, 1.0, 0.0),
                        normal: Vec3::Z,
                        uv: Vec2::new(0.5, 1.0),
                        color: [1.0; 4],
                    },
                ],
                vec![0, 1, 2],
            )
            .unwrap(),
        );
        let mut scene = Scene::new();
        let node = scene
            .insert(
                None,
                Node::new().with_mesh(near_mesh.clone()).with_lod([
                    LodLevel::new(near_mesh.clone(), 2.0).unwrap(),
                    LodLevel::new(far_mesh.clone(), 50.0).unwrap(),
                ]),
            )
            .unwrap();
        let lod_only = scene
            .insert(
                None,
                Node::new().with_lod([LodLevel::new(far_mesh.clone(), 50.0).unwrap()]),
            )
            .unwrap();
        let camera_at = |z: f32| {
            Camera::perspective(Vec3::new(0.0, 0.0, z), Vec3::ZERO, Vec3::Y, 1.0, 0.1, 100.0)
        };

        let prepared = PreparedScene::new(&scene, camera_at(1.5), 1.0).unwrap();
        assert!(prepared.uses_lod());
        assert_eq!(prepared.draws.len(), 2);
        let node_draw = prepared
            .draws
            .iter()
            .find(|draw| draw.node == node)
            .unwrap();
        assert_eq!(node_draw.mesh.id(), near_mesh.id());

        let prepared = PreparedScene::new(&scene, camera_at(10.0), 1.0).unwrap();
        let node_draw = prepared
            .draws
            .iter()
            .find(|draw| draw.node == node)
            .unwrap();
        assert_eq!(node_draw.mesh.id(), far_mesh.id());
        assert!(prepared.draws.iter().any(|draw| draw.node == lod_only));

        assert!(LodLevel::new(near_mesh, 0.0).is_err());
        assert!(LodLevel::new(far_mesh, f32::NAN).is_err());
    }

    #[test]
    fn hidden_nodes_and_subtrees_are_skipped_and_visibility_toggles_reevaluate() {
        let mut scene = Scene::new();
        let hidden_parent = scene.insert(None, Node::new().with_visible(false)).unwrap();
        scene
            .insert(Some(hidden_parent), Node::new().with_mesh(mesh()))
            .unwrap();
        let hidden_leaf = scene
            .insert(None, Node::new().with_mesh(mesh()).with_visible(false))
            .unwrap();
        let visible = scene.insert(None, Node::new().with_mesh(mesh())).unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            100.0,
        );

        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();
        assert_eq!(prepared.draws.len(), 1);
        assert_eq!(prepared.draws[0].node, visible);
        assert!(prepared.lights.is_empty());

        scene.set_visible(hidden_parent, true);
        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();
        assert_eq!(prepared.draws.len(), 2);
        assert!(prepared.draws.iter().all(|draw| draw.node != hidden_leaf));

        scene.set_visible(visible, false);
        assert_eq!(
            PreparedScene::new(&scene, camera, 1.0).unwrap().draws.len(),
            1
        );
    }
}
