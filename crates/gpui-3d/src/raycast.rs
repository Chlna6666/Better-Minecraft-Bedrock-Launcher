use crate::{
    EvaluatedScene, Mat4, MeshId, MeshPart, NodeId, Ray, Scene, Transform, Vec2, Vec3,
    mesh::BvhNode,
};
use std::collections::HashMap;

/// Ray query input error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RaycastError {
    /// Ray origin/direction are non-finite or direction is not normalized.
    InvalidRay,
}

impl std::fmt::Display for RaycastError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ray must have a finite origin and normalized direction")
    }
}

impl std::error::Error for RaycastError {}

/// Reusable traversal storage for repeated scene ray queries.
#[derive(Debug, Default)]
pub struct RaycastScratch {
    scene_nodes: Vec<(NodeId, Mat4)>,
    mesh_nodes: Vec<u32>,
}

impl RaycastScratch {
    /// Creates empty query storage.
    pub fn new() -> Self {
        Self::default()
    }

    /// Releases retained traversal capacity.
    pub fn trim(&mut self) {
        self.scene_nodes.shrink_to_fit();
        self.mesh_nodes.shrink_to_fit();
    }
}

/// Nearest mesh intersection returned by [`Scene::raycast`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RaycastHit {
    /// Intersected scene node.
    pub node: NodeId,
    /// Intersected geometry.
    pub mesh: MeshId,
    /// Triangle number within the mesh index buffer.
    pub triangle: u32,
    /// Material part containing this triangle.
    pub part: MeshPart,
    /// Distance from ray origin in world units.
    pub distance: f32,
    /// World-space hit position.
    pub position: Vec3,
    /// World-space geometric normal.
    pub normal: Vec3,
    /// Barycentrically interpolated texture coordinate.
    pub uv: Vec2,
}

#[derive(Clone, Copy)]
struct LocalRay {
    origin: Vec3,
    direction: Vec3,
}

impl Scene {
    /// Returns the nearest world-space triangle hit along a normalized ray.
    ///
    /// The query traverses node transforms and first rejects meshes whose transformed local
    /// bounds miss the ray. It reports geometric face normals; material alpha and back-face
    /// visibility do not alter geometric selection. The caller-owned scratch retains traversal
    /// storage between calls.
    ///
    /// # Errors
    ///
    /// Returns [`RaycastError::InvalidRay`] when the ray origin/direction is non-finite or the
    /// direction is not unit length within `1e-3`.
    pub fn raycast(
        &self,
        ray: Ray,
        scratch: &mut RaycastScratch,
    ) -> Result<Option<RaycastHit>, RaycastError> {
        raycast(self, None, ray, scratch)
    }
}

impl EvaluatedScene<'_> {
    /// Returns the nearest world-space triangle hit using this evaluated pose.
    ///
    /// The query observes the same sampled local transforms as [`crate::PreparedScene::new_evaluated`].
    /// It reports geometric face normals; material alpha and back-face visibility do not alter
    /// geometric selection.
    ///
    /// # Errors
    ///
    /// Returns [`RaycastError::InvalidRay`] when the ray origin/direction is non-finite or the
    /// direction is not unit length within `1e-3`.
    pub fn raycast(
        &self,
        ray: Ray,
        scratch: &mut RaycastScratch,
    ) -> Result<Option<RaycastHit>, RaycastError> {
        raycast(self.source, Some(&self.transforms), ray, scratch)
    }
}

fn raycast(
    scene: &Scene,
    transforms: Option<&HashMap<NodeId, Transform>>,
    ray: Ray,
    scratch: &mut RaycastScratch,
) -> Result<Option<RaycastHit>, RaycastError> {
    if !ray.origin.is_finite()
        || !ray.direction.is_finite()
        || (ray.direction.length_squared() - 1.0).abs() > 1e-3
    {
        return Err(RaycastError::InvalidRay);
    }
    scratch.scene_nodes.clear();
    scratch.mesh_nodes.clear();
    scratch
        .scene_nodes
        .extend(scene.roots().iter().copied().map(|id| (id, Mat4::IDENTITY)));
    let mut nearest = None;
    while let Some((id, parent_world)) = scratch.scene_nodes.pop() {
        let Some(node) = scene.node(id) else { continue };
        let local = transforms
            .and_then(|transforms| transforms.get(&id))
            .copied()
            .unwrap_or(node.transform);
        let world = parent_world * local.matrix();
        if let Some(mesh) = &node.mesh
            && let Some(inverse_world) = world.inverse()
        {
            let local_origin = inverse_world.transform_point(ray.origin);
            let local_direction = inverse_world.transform_vector(ray.direction);
            let local_ray = LocalRay {
                origin: local_origin,
                direction: local_direction,
            };
            if local_ray.origin.is_finite()
                && local_ray.direction.is_finite()
                && mesh.bounds().is_some_and(|bounds| {
                    bounds
                        .ray_entry(
                            local_ray.origin,
                            local_ray.direction,
                            nearest.map_or(f32::INFINITY, |hit: RaycastHit| hit.distance),
                        )
                        .is_some()
                })
            {
                visit_mesh(
                    id,
                    mesh,
                    inverse_world,
                    local_ray,
                    ray,
                    scratch,
                    &mut nearest,
                );
            }
        }
        if let Some(children) = scene.children(id) {
            scratch
                .scene_nodes
                .extend(children.iter().rev().copied().map(|child| (child, world)));
        }
    }
    Ok(nearest)
}

fn visit_mesh(
    node: NodeId,
    mesh: &crate::Mesh,
    inverse_world: Mat4,
    local_ray: LocalRay,
    world_ray: Ray,
    scratch: &mut RaycastScratch,
    nearest: &mut Option<RaycastHit>,
) {
    let Some(root) = mesh.bvh().first() else {
        return;
    };
    if root
        .bounds()
        .ray_entry(
            local_ray.origin,
            local_ray.direction,
            nearest.map_or(f32::INFINITY, |hit| hit.distance),
        )
        .is_none()
    {
        return;
    }
    scratch.mesh_nodes.push(0);
    while let Some(index) = scratch.mesh_nodes.pop() {
        let Some(bvh_node) = mesh.bvh().get(index as usize).copied() else {
            continue;
        };
        let max_distance = nearest.map_or(f32::INFINITY, |hit| hit.distance);
        if bvh_node
            .bounds()
            .ray_entry(local_ray.origin, local_ray.direction, max_distance)
            .is_none()
        {
            continue;
        }
        match bvh_node {
            BvhNode::Leaf {
                first_triangle,
                triangle_count,
                ..
            } => {
                let first_triangle = first_triangle as usize;
                let end = first_triangle + triangle_count as usize;
                for triangle in &mesh.triangle_order()[first_triangle..end] {
                    if let Some(hit) = mesh_triangle_hit(
                        node,
                        mesh,
                        inverse_world,
                        local_ray,
                        world_ray,
                        *triangle,
                        nearest.map_or(f32::INFINITY, |hit| hit.distance),
                    ) && nearest.is_none_or(|nearest_hit| hit.distance <= nearest_hit.distance)
                    {
                        *nearest = Some(hit);
                    }
                }
            }
            BvhNode::Branch { left, right, .. } => {
                let left_node = mesh.bvh()[left as usize];
                let right_node = mesh.bvh()[right as usize];
                let left_distance = left_node.bounds().ray_entry(
                    local_ray.origin,
                    local_ray.direction,
                    max_distance,
                );
                let right_distance = right_node.bounds().ray_entry(
                    local_ray.origin,
                    local_ray.direction,
                    max_distance,
                );
                match (left_distance, right_distance) {
                    (Some(left_distance), Some(right_distance))
                        if left_distance <= right_distance =>
                    {
                        scratch.mesh_nodes.push(right);
                        scratch.mesh_nodes.push(left);
                    }
                    (Some(_), Some(_)) => {
                        scratch.mesh_nodes.push(left);
                        scratch.mesh_nodes.push(right);
                    }
                    (Some(_), None) => scratch.mesh_nodes.push(left),
                    (None, Some(_)) => scratch.mesh_nodes.push(right),
                    (None, None) => {}
                }
            }
        }
    }
}

fn mesh_triangle_hit(
    node: NodeId,
    mesh: &crate::Mesh,
    inverse_world: Mat4,
    local_ray: LocalRay,
    world_ray: Ray,
    triangle: u32,
    max_distance: f32,
) -> Option<RaycastHit> {
    let first_index =
        usize::try_from(triangle).expect("validated mesh triangle indices fit usize") * 3;
    let vertices = mesh.vertices();
    let vertex_a = vertices[mesh.indices()[first_index] as usize];
    let vertex_b = vertices[mesh.indices()[first_index + 1] as usize];
    let vertex_c = vertices[mesh.indices()[first_index + 2] as usize];
    let (distance, barycentric_b, barycentric_c) = intersect_triangle(
        local_ray.origin,
        local_ray.direction,
        [vertex_a.position, vertex_b.position, vertex_c.position],
    )?;
    if distance > max_distance {
        return None;
    }
    let part = mesh.part_for_triangle(triangle)?;
    let local_normal =
        (vertex_b.position - vertex_a.position).cross(vertex_c.position - vertex_a.position);
    let world_normal = inverse_transpose_vector(inverse_world, local_normal);

    Some(RaycastHit {
        node,
        mesh: mesh.id(),
        triangle,
        part,
        distance,
        position: world_ray.at(distance),
        normal: world_normal.normalized().unwrap_or(Vec3::Y),
        uv: Vec2::new(
            vertex_a.uv.x * (1.0 - barycentric_b - barycentric_c)
                + vertex_b.uv.x * barycentric_b
                + vertex_c.uv.x * barycentric_c,
            vertex_a.uv.y * (1.0 - barycentric_b - barycentric_c)
                + vertex_b.uv.y * barycentric_b
                + vertex_c.uv.y * barycentric_c,
        ),
    })
}

fn inverse_transpose_vector(inverse: Mat4, vector: Vec3) -> Vec3 {
    let columns = inverse.columns();
    Vec3::new(
        columns[0][0] * vector.x + columns[1][0] * vector.y + columns[2][0] * vector.z,
        columns[0][1] * vector.x + columns[1][1] * vector.y + columns[2][1] * vector.z,
        columns[0][2] * vector.x + columns[1][2] * vector.y + columns[2][2] * vector.z,
    )
}

fn intersect_triangle(
    ray_origin: Vec3,
    ray_direction: Vec3,
    triangle: [Vec3; 3],
) -> Option<(f32, f32, f32)> {
    let [vertex_a, vertex_b, vertex_c] = triangle;
    let edge_to_b = vertex_b - vertex_a;
    let edge_to_c = vertex_c - vertex_a;
    let ray_perpendicular = ray_direction.cross(edge_to_c);
    let determinant = edge_to_b.dot(ray_perpendicular);
    if determinant.abs() <= 1e-7 {
        return None;
    }
    let inverse_determinant = determinant.recip();
    let origin_to_a = ray_origin - vertex_a;
    let barycentric_b = origin_to_a.dot(ray_perpendicular) * inverse_determinant;
    if !(0.0..=1.0).contains(&barycentric_b) {
        return None;
    }
    let origin_edge_cross = origin_to_a.cross(edge_to_b);
    let barycentric_c = ray_direction.dot(origin_edge_cross) * inverse_determinant;
    if barycentric_c < 0.0 || barycentric_b + barycentric_c > 1.0 {
        return None;
    }
    let distance = edge_to_c.dot(origin_edge_cross) * inverse_determinant;
    (distance >= 0.0 && distance.is_finite()).then_some((distance, barycentric_b, barycentric_c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mesh, Node, Transform, Vertex};
    use std::sync::Arc;

    #[test]
    fn raycast_applies_parent_and_node_transforms() {
        let mut scene = Scene::new();
        let parent = scene
            .insert(
                None,
                Node::new().with_transform(Transform {
                    translation: Vec3::new(0.0, 0.0, -2.0),
                    ..Transform::IDENTITY
                }),
            )
            .unwrap();
        let mesh = Arc::new(
            Mesh::new(
                vec![vertex(-0.5, -0.5), vertex(0.5, -0.5), vertex(0.0, 0.5)],
                vec![0, 1, 2],
            )
            .unwrap(),
        );
        let node = scene
            .insert(Some(parent), Node::new().with_mesh(mesh))
            .unwrap();
        let ray = Ray::new(Vec3::ZERO, -Vec3::Z).unwrap();
        let hit = scene
            .raycast(ray, &mut RaycastScratch::new())
            .unwrap()
            .unwrap();
        assert_eq!(hit.node, node);
        assert!((hit.distance - 2.0).abs() < 1e-5);
        assert!((hit.position.z + 2.0).abs() < 1e-5);
        assert_eq!(hit.part.material_slot(), 0);
    }

    #[test]
    fn raycast_uses_bvh_and_reuses_query_scratch() {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for triangle in 0..32_u32 {
            let left = f32::from(u16::try_from(triangle).unwrap()) * 2.0;
            let first = u32::try_from(vertices.len()).unwrap();
            vertices.extend([
                vertex_at(left, 0.0, -2.0),
                vertex_at(left + 1.0, 0.0, -2.0),
                vertex_at(left + 0.5, 1.0, -2.0),
            ]);
            indices.extend([first, first + 1, first + 2]);
        }
        let mesh = Arc::new(
            Mesh::with_parts(
                vertices,
                indices,
                vec![MeshPart::new(0, 48, 0), MeshPart::new(48, 48, 1)],
            )
            .unwrap(),
        );
        let mut scene = Scene::new();
        scene.insert(None, Node::new().with_mesh(mesh)).unwrap();
        let ray = Ray::new(Vec3::new(38.5, 0.25, 0.0), -Vec3::Z).unwrap();
        let mut scratch = RaycastScratch::new();

        let hit = scene.raycast(ray, &mut scratch).unwrap().unwrap();
        assert_eq!(hit.triangle, 19);
        assert_eq!(hit.part.material_slot(), 1);
        let scene_capacity = scratch.scene_nodes.capacity();
        let mesh_capacity = scratch.mesh_nodes.capacity();
        assert!(scene_capacity > 0);
        assert!(mesh_capacity > 0);

        assert_eq!(scene.raycast(ray, &mut scratch).unwrap(), Some(hit));
        assert_eq!(scratch.scene_nodes.capacity(), scene_capacity);
        assert_eq!(scratch.mesh_nodes.capacity(), mesh_capacity);
    }

    #[test]
    fn raycast_transforms_face_normals_with_nonuniform_scale() {
        let mesh = Arc::new(
            Mesh::new(
                vec![
                    vertex_at(0.0, 0.0, 0.0),
                    vertex_at(1.0, 0.0, 0.0),
                    vertex_at(0.0, 1.0, 1.0),
                ],
                vec![0, 1, 2],
            )
            .unwrap(),
        );
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new().with_mesh(mesh).with_transform(Transform {
                    scale: Vec3::new(2.0, 1.0, 0.5),
                    ..Transform::IDENTITY
                }),
            )
            .unwrap();
        let ray = Ray::new(Vec3::new(0.5, 0.25, 2.0), -Vec3::Z).unwrap();

        let hit = scene
            .raycast(ray, &mut RaycastScratch::new())
            .unwrap()
            .unwrap();
        let expected = Vec3::new(0.0, -1.0, 2.0).normalized().unwrap();
        assert!((hit.normal - expected).length() < 1e-5);
    }

    fn vertex(x: f32, y: f32) -> Vertex {
        vertex_at(x, y, 0.0)
    }

    fn vertex_at(x: f32, y: f32, z: f32) -> Vertex {
        Vertex {
            position: Vec3::new(x, y, z),
            normal: Vec3::Z,
            uv: Vec2::new(x + 0.5, y + 0.5),
            color: [1.0; 4],
        }
    }
}
