use super::{Mesh, MeshError, MeshId, NEXT_MESH_ID};
use crate::Vec3;
use bevy_mikktspace::Geometry;
use std::collections::HashMap;

/// A mesh with generated MikkTSpace tangent frames and an output-to-source vertex map.
#[derive(Clone, Debug)]
#[must_use]
pub struct GeneratedTangents {
    mesh: Mesh,
    source_vertices: Box<[u32]>,
}

impl GeneratedTangents {
    /// Returns the generated mesh.
    #[must_use]
    pub fn mesh(&self) -> &Mesh {
        &self.mesh
    }

    /// Returns the source vertex for each vertex in [`Self::mesh`].
    #[must_use]
    pub fn source_vertices(&self) -> &[u32] {
        &self.source_vertices
    }

    /// Returns the generated mesh and output-to-source vertex map.
    #[must_use]
    pub fn into_parts(self) -> (Mesh, Box<[u32]>) {
        (self.mesh, self.source_vertices)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TangentKey {
    source_vertex: u32,
    tangent: [u32; 4],
}

struct TangentGeometry<'a> {
    mesh: &'a Mesh,
    tangents: Vec<Option<[f32; 4]>>,
}

impl Geometry for TangentGeometry<'_> {
    fn num_faces(&self) -> usize {
        self.mesh.indices().len() / 3
    }

    fn num_vertices_of_face(&self, _face: usize) -> usize {
        3
    }

    fn position(&self, face: usize, vert: usize) -> [f32; 3] {
        let source = self.mesh.indices()[face * 3 + vert] as usize;
        let position = self.mesh.vertices()[source].position;
        [position.x, position.y, position.z]
    }

    fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
        let source = self.mesh.indices()[face * 3 + vert] as usize;
        let normal = self.mesh.vertices()[source]
            .normal
            .normalized()
            .unwrap_or(Vec3::ZERO);
        [normal.x, normal.y, normal.z]
    }

    fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
        let source = self.mesh.indices()[face * 3 + vert] as usize;
        let uv = self.mesh.vertices()[source].uv;
        [uv.x, uv.y]
    }

    fn set_tangent_encoded(&mut self, tangent: [f32; 4], face: usize, vert: usize) {
        self.tangents[face * 3 + vert] = Some(tangent);
    }
}

impl Mesh {
    /// Returns a mesh snapshot with caller-supplied tangent frames for UV set zero.
    ///
    /// Supply one `[x, y, z, handedness]` frame per vertex. The method normalizes and
    /// orthogonalizes xyz against each vertex normal and preserves handedness `-1` or `1`.
    /// Tangent seams must already use split vertices, and all three vertices of a triangle must
    /// have the same handedness. Existing snapshots remain unchanged; the returned mesh receives
    /// a fresh mesh identity.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError::InvalidTangents`] when the count differs from the vertex count, a
    /// frame is non-finite or undefined, or a triangle mixes handedness signs.
    pub fn with_tangents(&self, tangents: impl Into<Box<[[f32; 4]]>>) -> Result<Self, MeshError> {
        let tangents = tangents.into();
        if tangents.len() != self.vertices().len() {
            return Err(MeshError::InvalidTangents);
        }
        let tangents = tangents
            .iter()
            .zip(self.vertices())
            .map(|(tangent, vertex)| {
                if tangent[3] != -1.0 && tangent[3] != 1.0 {
                    return Err(MeshError::InvalidTangents);
                }
                normalize_tangent(*tangent, vertex.normal).map_err(|_| MeshError::InvalidTangents)
            })
            .collect::<Result<Vec<_>, _>>()?;
        for triangle in self.indices().chunks_exact(3) {
            let handedness = tangents[triangle[0] as usize][3];
            if triangle
                .iter()
                .any(|&index| tangents[index as usize][3] != handedness)
            {
                return Err(MeshError::InvalidTangents);
            }
        }

        let mut mesh = self.clone();
        mesh.tangents = Some(tangents.into_boxed_slice());
        mesh.id = MeshId(NEXT_MESH_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        mesh.generation = 0;
        Ok(mesh)
    }

    /// Generates MikkTSpace tangent frames for texture coordinate set zero.
    ///
    /// The returned immutable mesh preserves triangle order, material parts, edge masks, and
    /// vertex attributes. It splits vertices when triangle corners require different tangent
    /// frames, including mirrored UV seams. Unreferenced vertices remain in the output and receive
    /// an unused fallback frame. The source mesh is unchanged.
    ///
    /// Use [`GeneratedTangents::source_vertices`] to remap attributes stored outside
    /// [`super::Vertex`].
    /// Tangent generation is synchronous CPU work. Regenerate tangents after changing positions,
    /// normals, or UVs.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError::InvalidGeneratedTangents`] when an indexed vertex has no usable
    /// normal, no triangle has both nonzero position and UV area, or MikkTSpace cannot produce a
    /// finite, nonzero tangent frame for a triangle corner.
    pub fn generate_tangents(&self) -> Result<GeneratedTangents, MeshError> {
        if !has_usable_tangent_face(self) {
            return Err(MeshError::InvalidGeneratedTangents);
        }

        let mut geometry = TangentGeometry {
            mesh: self,
            tangents: vec![None; self.indices().len()],
        };
        if !bevy_mikktspace::generate_tangents(&mut geometry) {
            return Err(MeshError::InvalidGeneratedTangents);
        }
        let corner_tangents = std::mem::take(&mut geometry.tangents);
        drop(geometry);

        let mut source_vertices = Vec::with_capacity(self.vertices().len());
        let mut vertices = Vec::with_capacity(self.vertices().len());
        let mut tangents = Vec::with_capacity(self.vertices().len());
        let mut indices = Vec::with_capacity(self.indices().len());
        let mut vertex_by_tangent = HashMap::with_capacity(self.vertices().len());

        for (corner, &source_vertex) in self.indices().iter().enumerate() {
            let vertex = self.vertices()[source_vertex as usize];
            let tangent = normalize_tangent(
                corner_tangents[corner].ok_or(MeshError::InvalidGeneratedTangents)?,
                vertex.normal,
            )?;
            let key = TangentKey {
                source_vertex,
                tangent: tangent.map(f32::to_bits),
            };
            let index = if let Some(&index) = vertex_by_tangent.get(&key) {
                index
            } else {
                let index = u32::try_from(vertices.len()).map_err(|_| MeshError::TooLarge)?;
                vertices.push(vertex);
                tangents.push(tangent);
                source_vertices.push(source_vertex);
                vertex_by_tangent.insert(key, index);
                index
            };
            indices.push(index);
        }

        let mut referenced = vec![false; self.vertices().len()];
        for &index in self.indices() {
            referenced[index as usize] = true;
        }
        for (source_vertex, (&vertex, &is_referenced)) in
            self.vertices().iter().zip(&referenced).enumerate()
        {
            if !is_referenced {
                vertices.push(vertex);
                tangents.push(fallback_tangent(vertex.normal));
                source_vertices
                    .push(u32::try_from(source_vertex).map_err(|_| MeshError::TooLarge)?);
            }
        }

        let mut mesh = Mesh::with_parts(vertices, indices, self.parts.clone())?;
        mesh.edge_masks = self.edge_masks.clone();
        mesh.tangents = Some(tangents.into_boxed_slice());

        Ok(GeneratedTangents {
            mesh,
            source_vertices: source_vertices.into_boxed_slice(),
        })
    }
}

fn has_usable_tangent_face(mesh: &Mesh) -> bool {
    mesh.indices().chunks_exact(3).any(|triangle| {
        let vertices = [
            mesh.vertices()[triangle[0] as usize],
            mesh.vertices()[triangle[1] as usize],
            mesh.vertices()[triangle[2] as usize],
        ];
        let [p0, p1, p2] = vertices.map(|vertex| {
            [
                f64::from(vertex.position.x),
                f64::from(vertex.position.y),
                f64::from(vertex.position.z),
            ]
        });
        let edge_a = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let edge_b = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
        let area = [
            edge_a[1] * edge_b[2] - edge_a[2] * edge_b[1],
            edge_a[2] * edge_b[0] - edge_a[0] * edge_b[2],
            edge_a[0] * edge_b[1] - edge_a[1] * edge_b[0],
        ];
        if area == [0.0; 3] {
            return false;
        }

        let uv = vertices.map(|vertex| [f64::from(vertex.uv.x), f64::from(vertex.uv.y)]);
        let delta_a = [uv[1][0] - uv[0][0], uv[1][1] - uv[0][1]];
        let delta_b = [uv[2][0] - uv[0][0], uv[2][1] - uv[0][1]];
        delta_a[0] * delta_b[1] - delta_a[1] * delta_b[0] != 0.0
    })
}

fn normalize_tangent(tangent: [f32; 4], normal: Vec3) -> Result<[f32; 4], MeshError> {
    if tangent.iter().any(|component| !component.is_finite()) {
        return Err(MeshError::InvalidGeneratedTangents);
    }
    let normal = normal
        .normalized()
        .ok_or(MeshError::InvalidGeneratedTangents)?;
    let tangent_direction = Vec3::new(tangent[0], tangent[1], tangent[2]);
    let tangent_direction = (tangent_direction - normal * normal.dot(tangent_direction))
        .normalized()
        .ok_or(MeshError::InvalidGeneratedTangents)?;
    let handedness = if tangent[3] < 0.0 {
        -1.0
    } else if tangent[3] > 0.0 {
        1.0
    } else {
        return Err(MeshError::InvalidGeneratedTangents);
    };
    Ok([
        tangent_direction.x,
        tangent_direction.y,
        tangent_direction.z,
        handedness,
    ])
}

fn fallback_tangent(normal: Vec3) -> [f32; 4] {
    let Some(normal) = normal.normalized() else {
        return [1.0, 0.0, 0.0, 1.0];
    };
    let axis = if normal.x.abs() < 0.9 {
        Vec3::X
    } else {
        Vec3::Y
    };
    let tangent = normal.cross(axis).normalized().unwrap_or(Vec3::X);
    [tangent.x, tangent.y, tangent.z, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MeshPart, TriangleEdgeMask, Vec2, Vertex};

    fn plane() -> Mesh {
        Mesh::new(
            [
                Vertex {
                    position: Vec3::ZERO,
                    normal: Vec3::Z,
                    uv: Vec2::ZERO,
                    color: [0.1, 0.2, 0.3, 1.0],
                },
                Vertex {
                    position: Vec3::X,
                    normal: Vec3::Z,
                    uv: Vec2::new(1.0, 0.0),
                    color: [0.4, 0.5, 0.6, 1.0],
                },
                Vertex {
                    position: Vec3::Y,
                    normal: Vec3::Z,
                    uv: Vec2::new(0.0, 1.0),
                    color: [0.7, 0.8, 0.9, 1.0],
                },
            ],
            [0, 1, 2],
        )
        .unwrap()
    }

    #[test]
    fn generates_orthonormal_tangents_and_preserves_source_mesh() {
        let source = plane();
        let source_id = source.id();
        let generated = source.generate_tangents().unwrap();
        let mesh = generated.mesh();

        assert_ne!(mesh.id(), source_id);
        assert_eq!(source.tangents(), None);
        assert_eq!(mesh.tangents().unwrap().len(), mesh.vertices().len());
        assert_eq!(mesh.indices(), [0, 1, 2]);
        assert_eq!(generated.source_vertices(), [0, 1, 2]);
        for (vertex, tangent) in mesh.vertices().iter().zip(mesh.tangents().unwrap()) {
            let direction = Vec3::new(tangent[0], tangent[1], tangent[2]);
            assert!((direction.length() - 1.0).abs() < 1.0e-5);
            assert!(vertex.normal.dot(direction).abs() < 1.0e-5);
            assert_eq!(tangent[3], 1.0);
        }
    }

    #[test]
    fn supplied_tangents_are_normalized_and_orthogonalized() {
        let source = plane();
        let source_id = source.id();
        let mesh = source.with_tangents([[1.0, 0.0, 1.0, 1.0]; 3]).unwrap();

        assert_ne!(mesh.id(), source_id);
        assert_eq!(source.tangents(), None);
        for tangent in mesh.tangents().unwrap() {
            assert!((tangent[0] - 1.0).abs() < 1.0e-5);
            assert!(tangent[1].abs() < 1.0e-5);
            assert!(tangent[2].abs() < 1.0e-5);
            assert_eq!(tangent[3], 1.0);
        }
    }

    #[test]
    fn supplied_tangents_reject_invalid_count_and_mixed_triangle_handedness() {
        let source = plane();
        assert_eq!(
            source.with_tangents([[1.0, 0.0, 0.0, 1.0]; 2]).unwrap_err(),
            MeshError::InvalidTangents
        );
        assert_eq!(
            source
                .with_tangents([
                    [1.0, 0.0, 0.0, 1.0],
                    [1.0, 0.0, 0.0, -1.0],
                    [1.0, 0.0, 0.0, 1.0],
                ])
                .unwrap_err(),
            MeshError::InvalidTangents
        );
    }

    #[test]
    fn preserves_parts_edge_masks_and_unused_vertices() {
        let source = Mesh::with_parts(
            [
                Vertex {
                    position: Vec3::ZERO,
                    normal: Vec3::Z,
                    uv: Vec2::ZERO,
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::X,
                    normal: Vec3::Z,
                    uv: Vec2::new(1.0, 0.0),
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::Y,
                    normal: Vec3::Z,
                    uv: Vec2::new(0.0, 1.0),
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::new(9.0, 9.0, 9.0),
                    normal: Vec3::ZERO,
                    uv: Vec2::ZERO,
                    color: [1.0; 4],
                },
            ],
            [0, 1, 2],
            [MeshPart::new(0, 3, 2)],
        )
        .unwrap()
        .with_edge_masks([TriangleEdgeMask::ALL])
        .unwrap();
        let generated = source.generate_tangents().unwrap();
        let mesh = generated.mesh();

        assert_eq!(mesh.parts(), source.parts());
        assert_eq!(mesh.edge_masks(), source.edge_masks());
        assert_eq!(mesh.vertices().len(), 4);
        assert_eq!(generated.source_vertices(), [0, 1, 2, 3]);
        assert_eq!(mesh.vertices()[3].position, Vec3::new(9.0, 9.0, 9.0));
    }

    #[test]
    fn splits_vertices_at_mirrored_uv_handedness_seams() {
        let source = Mesh::new(
            [
                Vertex {
                    position: Vec3::ZERO,
                    normal: Vec3::Z,
                    uv: Vec2::ZERO,
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::X,
                    normal: Vec3::Z,
                    uv: Vec2::new(1.0, 0.0),
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::Y,
                    normal: Vec3::Z,
                    uv: Vec2::new(0.0, 1.0),
                    color: [1.0; 4],
                },
                Vertex {
                    position: -Vec3::X,
                    normal: Vec3::Z,
                    uv: Vec2::new(1.0, 0.0),
                    color: [1.0; 4],
                },
            ],
            [0, 1, 2, 0, 2, 3],
        )
        .unwrap();
        let mesh = source.generate_tangents().unwrap().into_parts().0;

        assert_eq!(mesh.indices().len(), source.indices().len());
        assert!(mesh.vertices().len() > source.vertices().len());
        assert_ne!(mesh.indices()[0], mesh.indices()[3]);
        let tangents = mesh.tangents().unwrap();
        assert_ne!(
            tangents[mesh.indices()[0] as usize][3],
            tangents[mesh.indices()[3] as usize][3]
        );
    }

    #[test]
    fn rejects_degenerate_uv_triangles() {
        let source = Mesh::new(
            [
                Vertex {
                    position: Vec3::ZERO,
                    normal: Vec3::Z,
                    uv: Vec2::ZERO,
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::X,
                    normal: Vec3::Z,
                    uv: Vec2::ZERO,
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::Y,
                    normal: Vec3::Z,
                    uv: Vec2::ZERO,
                    color: [1.0; 4],
                },
            ],
            [0, 1, 2],
        )
        .unwrap();

        assert_eq!(
            source.generate_tangents().unwrap_err(),
            MeshError::InvalidGeneratedTangents
        );
    }

    #[test]
    fn rejects_degenerate_position_triangles() {
        let source = Mesh::new(
            [
                Vertex {
                    position: Vec3::ZERO,
                    normal: Vec3::Z,
                    uv: Vec2::ZERO,
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::X,
                    normal: Vec3::Z,
                    uv: Vec2::new(1.0, 0.0),
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::new(2.0, 0.0, 0.0),
                    normal: Vec3::Z,
                    uv: Vec2::new(0.0, 1.0),
                    color: [1.0; 4],
                },
            ],
            [0, 1, 2],
        )
        .unwrap();

        assert_eq!(
            source.generate_tangents().unwrap_err(),
            MeshError::InvalidGeneratedTangents
        );
    }

    #[test]
    fn rejects_meshes_without_triangles() {
        let source = Mesh::new(Vec::<Vertex>::new(), Vec::<u32>::new()).unwrap();

        assert_eq!(
            source.generate_tangents().unwrap_err(),
            MeshError::InvalidGeneratedTangents
        );
    }

    #[test]
    fn generates_tangents_for_uv_sphere_with_degenerate_pole_faces() {
        let source = Mesh::uv_sphere(1.0, [16, 8]).unwrap();
        let generated = source.generate_tangents().unwrap();

        assert_eq!(generated.mesh().indices().len(), source.indices().len());
        assert_eq!(
            generated.mesh().tangents().unwrap().len(),
            generated.mesh().vertices().len()
        );
    }
}
