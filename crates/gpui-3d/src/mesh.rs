use crate::{Aabb, Vec2, Vec3};
use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

mod tangents;
pub use tangents::GeneratedTangents;

static NEXT_MESH_ID: AtomicU64 = AtomicU64::new(1);

/// Stable identity for immutable mesh geometry.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MeshId(pub u64);

/// Vertex attributes consumed by scene renderers. Optional tangent frames are stored separately
/// on [`Mesh`] so geometry without normal maps does not need tangent data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    /// Position in mesh-local coordinates.
    pub position: Vec3,
    /// Normal in mesh-local coordinates.
    pub normal: Vec3,
    /// Texture coordinate.
    pub uv: Vec2,
    /// Linear RGBA vertex color.
    pub color: [f32; 4],
}

/// Selects triangle edges for one-pixel coverage smoothing.
///
/// Edge 0, 1, or 2 is the edge opposite the matching triangle vertex. Edges omitted from the
/// mask remain fully covered, which lets a mesh hide triangulation seams while smoothing its
/// silhouette.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TriangleEdgeMask(u8);

/// Whether any vertex carries a usable texture coordinate.
///
/// An all-zero UV stream cannot address a texture region; treating it as texture-mapped would make
/// every draw sample one arbitrary texel.
fn mesh_uses_uv_regions(vertices: &[Vertex]) -> bool {
    vertices
        .iter()
        .any(|vertex| vertex.uv.x != 0.0 || vertex.uv.y != 0.0)
}

impl TriangleEdgeMask {
    /// No edges receive coverage smoothing.
    pub const NONE: Self = Self(0);
    /// All three edges receive coverage smoothing.
    pub const ALL: Self = Self(0b111);

    /// Selects edges opposite vertices 0, 1, and 2.
    #[must_use]
    pub const fn new(opposite_vertices: [bool; 3]) -> Self {
        Self(
            (opposite_vertices[0] as u8)
                | ((opposite_vertices[1] as u8) << 1)
                | ((opposite_vertices[2] as u8) << 2),
        )
    }

    pub(crate) const fn bits(self) -> u8 {
        self.0
    }
}

/// Invalid mesh data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeshError {
    /// A vertex or index count exceeds the 32-bit draw range.
    TooLarge,
    /// A vertex update does not preserve the mesh's vertex count.
    VertexCountMismatch,
    /// The index count does not describe complete triangles.
    IncompleteTriangles,
    /// A vertex contains a non-finite attribute.
    NonFiniteVertex,
    /// An index addresses a missing vertex.
    IndexOutOfBounds,
    /// Parts do not form contiguous, triangle-aligned coverage of the index buffer.
    InvalidParts,
    /// Triangle edge masks do not match the mesh's triangle count.
    InvalidTriangleEdgeMasks,
    /// A triangle is degenerate or a vertex cannot receive a generated normal.
    InvalidGeneratedNormals,
    /// No usable tangent space exists for the mesh's geometry, UVs, or indexed normals.
    InvalidGeneratedTangents,
    /// Supplied tangent data does not match or define the mesh's vertex tangent frames.
    InvalidTangents,
    /// Primitive dimensions, radius, or segment counts are invalid.
    InvalidPrimitiveParameters,
}

impl fmt::Display for MeshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::TooLarge => "mesh exceeds the 32-bit draw range",
            Self::VertexCountMismatch => "updated mesh vertex count does not match the source mesh",
            Self::IncompleteTriangles => "index count is not a multiple of three",
            Self::NonFiniteVertex => "vertex contains a non-finite attribute",
            Self::IndexOutOfBounds => "mesh index refers to a missing vertex",
            Self::InvalidParts => "mesh parts do not cover the index buffer with triangle ranges",
            Self::InvalidTriangleEdgeMasks => {
                "triangle edge mask count does not match the mesh triangle count"
            }
            Self::InvalidGeneratedNormals => {
                "mesh normals cannot be generated for degenerate or unreferenced vertices"
            }
            Self::InvalidGeneratedTangents => "mesh has no valid tangent space",
            Self::InvalidTangents => "mesh tangent data is invalid",
            Self::InvalidPrimitiveParameters => {
                "primitive dimensions or segment counts are invalid"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for MeshError {}

/// Immutable indexed triangle geometry.
#[derive(Clone, Debug)]
#[must_use]
pub struct Mesh {
    id: MeshId,
    generation: u64,
    vertices: Box<[Vertex]>,
    tangents: Option<Box<[[f32; 4]]>>,
    indices: Box<[u32]>,
    edge_masks: Option<Box<[TriangleEdgeMask]>>,
    parts: Box<[MeshPart]>,
    part_bounds: Box<[Aabb]>,
    bounds: Option<Aabb>,
    bvh: Box<[BvhNode]>,
    triangle_order: Box<[u32]>,
    uses_uv: bool,
}

/// A contiguous triangle range using one scene-node material slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeshPart {
    first_index: u32,
    index_count: u32,
    material_slot: u32,
}

impl MeshPart {
    /// Creates a mesh part descriptor. [`Mesh::with_parts`] validates its range.
    #[must_use]
    pub const fn new(first_index: u32, index_count: u32, material_slot: u32) -> Self {
        Self {
            first_index,
            index_count,
            material_slot,
        }
    }

    /// First index in the mesh index buffer.
    #[must_use]
    pub const fn first_index(self) -> u32 {
        self.first_index
    }

    /// Number of indices in this part.
    #[must_use]
    pub const fn index_count(self) -> u32 {
        self.index_count
    }

    /// Material slot resolved against the owning scene node.
    #[must_use]
    pub const fn material_slot(self) -> u32 {
        self.material_slot
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum BvhNode {
    Leaf {
        bounds: Aabb,
        first_triangle: u32,
        triangle_count: u32,
    },
    Branch {
        bounds: Aabb,
        left: u32,
        right: u32,
    },
}

impl BvhNode {
    pub(crate) fn bounds(self) -> Aabb {
        match self {
            Self::Leaf { bounds, .. } | Self::Branch { bounds, .. } => bounds,
        }
    }
}

impl Mesh {
    /// Creates a unit cube centered at the origin with face normals and UVs.
    #[must_use]
    pub fn cube() -> Self {
        let faces = [
            (
                [
                    Vec3::new(-0.5, -0.5, 0.5),
                    Vec3::new(0.5, -0.5, 0.5),
                    Vec3::new(0.5, 0.5, 0.5),
                    Vec3::new(-0.5, 0.5, 0.5),
                ],
                Vec3::Z,
            ),
            (
                [
                    Vec3::new(0.5, -0.5, -0.5),
                    Vec3::new(-0.5, -0.5, -0.5),
                    Vec3::new(-0.5, 0.5, -0.5),
                    Vec3::new(0.5, 0.5, -0.5),
                ],
                -Vec3::Z,
            ),
            (
                [
                    Vec3::new(0.5, -0.5, 0.5),
                    Vec3::new(0.5, -0.5, -0.5),
                    Vec3::new(0.5, 0.5, -0.5),
                    Vec3::new(0.5, 0.5, 0.5),
                ],
                Vec3::X,
            ),
            (
                [
                    Vec3::new(-0.5, -0.5, -0.5),
                    Vec3::new(-0.5, -0.5, 0.5),
                    Vec3::new(-0.5, 0.5, 0.5),
                    Vec3::new(-0.5, 0.5, -0.5),
                ],
                -Vec3::X,
            ),
            (
                [
                    Vec3::new(-0.5, 0.5, 0.5),
                    Vec3::new(0.5, 0.5, 0.5),
                    Vec3::new(0.5, 0.5, -0.5),
                    Vec3::new(-0.5, 0.5, -0.5),
                ],
                Vec3::Y,
            ),
            (
                [
                    Vec3::new(-0.5, -0.5, -0.5),
                    Vec3::new(0.5, -0.5, -0.5),
                    Vec3::new(0.5, -0.5, 0.5),
                    Vec3::new(-0.5, -0.5, 0.5),
                ],
                -Vec3::Y,
            ),
        ];
        let mut vertices = Vec::with_capacity(faces.len() * 4);
        let mut indices = Vec::with_capacity(faces.len() * 6);
        for (face_index, (positions, normal)) in faces.into_iter().enumerate() {
            let first = u32::try_from(face_index * 4).expect("unit cube vertex index fits u32");
            for (position, uv) in positions.into_iter().zip([
                Vec2::new(0.0, 1.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(0.0, 0.0),
            ]) {
                vertices.push(Vertex {
                    position,
                    normal,
                    uv,
                    color: [1.0; 4],
                });
            }
            indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
        }
        Self::new(vertices, indices).expect("unit cube geometry is valid")
    }

    /// Creates an XZ plane centered at the origin.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError::InvalidPrimitiveParameters`] for non-finite or non-positive sizes.
    pub fn plane(width: f32, depth: f32) -> Result<Self, MeshError> {
        if !width.is_finite() || !depth.is_finite() || width <= 0.0 || depth <= 0.0 {
            return Err(MeshError::InvalidPrimitiveParameters);
        }
        Self::new(
            [
                Vertex {
                    position: Vec3::new(-width * 0.5, 0.0, -depth * 0.5),
                    normal: Vec3::Y,
                    uv: Vec2::new(0.0, 0.0),
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::new(width * 0.5, 0.0, -depth * 0.5),
                    normal: Vec3::Y,
                    uv: Vec2::new(1.0, 0.0),
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::new(width * 0.5, 0.0, depth * 0.5),
                    normal: Vec3::Y,
                    uv: Vec2::new(1.0, 1.0),
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::new(-width * 0.5, 0.0, depth * 0.5),
                    normal: Vec3::Y,
                    uv: Vec2::new(0.0, 1.0),
                    color: [1.0; 4],
                },
            ],
            [0, 2, 1, 0, 3, 2],
        )
    }

    /// Creates a UV sphere centered at the origin with analytic normals.
    ///
    /// `segments` is `[longitude, latitude]`; longitude requires at least 3 segments and
    /// latitude requires at least 2.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError::InvalidPrimitiveParameters`] for an invalid radius or segment count,
    /// and [`MeshError::TooLarge`] when generated indices exceed the 32-bit draw range.
    #[allow(
        clippy::cast_precision_loss,
        reason = "the vertex limit is memory-bound well below the point where f32 UV precision changes materially"
    )]
    pub fn uv_sphere(radius: f32, segments: [u32; 2]) -> Result<Self, MeshError> {
        let [longitude_segments, latitude_segments] = segments;
        if !radius.is_finite() || radius <= 0.0 || longitude_segments < 3 || latitude_segments < 2 {
            return Err(MeshError::InvalidPrimitiveParameters);
        }
        let longitude_segments =
            usize::try_from(longitude_segments).map_err(|_| MeshError::TooLarge)?;
        let latitude_segments =
            usize::try_from(latitude_segments).map_err(|_| MeshError::TooLarge)?;
        let vertex_count = longitude_segments
            .checked_add(1)
            .and_then(|longitude| {
                latitude_segments
                    .checked_add(1)
                    .and_then(|latitude| longitude.checked_mul(latitude))
            })
            .ok_or(MeshError::TooLarge)?;
        let index_count = longitude_segments
            .checked_mul(latitude_segments)
            .and_then(|quads| quads.checked_mul(6))
            .ok_or(MeshError::TooLarge)?;
        if u32::try_from(vertex_count).is_err() || u32::try_from(index_count).is_err() {
            return Err(MeshError::TooLarge);
        }
        let mut vertices = Vec::with_capacity(vertex_count);
        for latitude in 0..=latitude_segments {
            let v = latitude as f32 / latitude_segments as f32;
            let theta = v * std::f32::consts::PI;
            let (sin_theta, cos_theta) = theta.sin_cos();
            for longitude in 0..=longitude_segments {
                let u = longitude as f32 / longitude_segments as f32;
                let phi = u * std::f32::consts::TAU;
                let (sin_phi, cos_phi) = phi.sin_cos();
                let normal = Vec3::new(sin_theta * cos_phi, cos_theta, sin_theta * sin_phi);
                vertices.push(Vertex {
                    position: normal * radius,
                    normal,
                    uv: Vec2::new(u, 1.0 - v),
                    color: [1.0; 4],
                });
            }
        }
        let mut indices = Vec::with_capacity(index_count);
        let row = longitude_segments + 1;
        for latitude in 0..latitude_segments {
            for longitude in 0..longitude_segments {
                let first = latitude * row + longitude;
                let a = u32::try_from(first).map_err(|_| MeshError::TooLarge)?;
                let b = u32::try_from(first + 1).map_err(|_| MeshError::TooLarge)?;
                let c = u32::try_from(first + row).map_err(|_| MeshError::TooLarge)?;
                let d = u32::try_from(first + row + 1).map_err(|_| MeshError::TooLarge)?;
                indices.extend([a, b, c, b, d, c]);
            }
        }
        Self::new(vertices, indices)
    }

    /// Creates a cylinder centered at the origin with its axis along Y.
    ///
    /// The side has smooth radial normals; the caps have separate flat normals and UVs.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError::InvalidPrimitiveParameters`] for non-finite or non-positive radius
    /// or height, or fewer than three radial segments. Returns [`MeshError::TooLarge`] when the
    /// generated geometry exceeds the 32-bit draw range.
    #[allow(
        clippy::cast_precision_loss,
        reason = "primitive segment counts are memory-bound well below the point where f32 UV precision changes materially"
    )]
    pub fn cylinder(radius: f32, height: f32, segments: u32) -> Result<Self, MeshError> {
        radial_mesh(radius, radius, height, segments)
    }

    /// Creates a cone centered at the origin with its axis along Y and base below the origin.
    ///
    /// Side normals are smooth and sloped; the base has a separate flat normal and UVs.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError::InvalidPrimitiveParameters`] for non-finite or non-positive radius
    /// or height, or fewer than three radial segments. Returns [`MeshError::TooLarge`] when the
    /// generated geometry exceeds the 32-bit draw range.
    #[allow(
        clippy::cast_precision_loss,
        reason = "primitive segment counts are memory-bound well below the point where f32 UV precision changes materially"
    )]
    pub fn cone(radius: f32, height: f32, segments: u32) -> Result<Self, MeshError> {
        if !radius.is_finite()
            || radius <= 0.0
            || !height.is_finite()
            || height <= 0.0
            || segments < 3
        {
            return Err(MeshError::InvalidPrimitiveParameters);
        }
        let segments = usize::try_from(segments).map_err(|_| MeshError::TooLarge)?;
        let vertex_count = segments
            .checked_mul(4)
            .and_then(|count| count.checked_add(2))
            .ok_or(MeshError::TooLarge)?;
        let index_count = segments.checked_mul(6).ok_or(MeshError::TooLarge)?;
        if u32::try_from(vertex_count).is_err() || u32::try_from(index_count).is_err() {
            return Err(MeshError::TooLarge);
        }

        let mut vertices = Vec::with_capacity(vertex_count);
        let mut indices = Vec::with_capacity(index_count);
        let half_height = height * 0.5;
        let normal_scale = radius.max(height);
        let horizontal_normal = height / normal_scale;
        let vertical_normal = radius / normal_scale;
        for segment in 0..segments {
            let u0 = segment as f32 / segments as f32;
            let u1 = (segment + 1) as f32 / segments as f32;
            let theta0 = u0 * std::f32::consts::TAU;
            let theta1 = u1 * std::f32::consts::TAU;
            let (sin0, cos0) = theta0.sin_cos();
            let (sin1, cos1) = theta1.sin_cos();
            let middle = (theta0 + theta1) * 0.5;
            let (sin_middle, cos_middle) = middle.sin_cos();
            let normal0 = Vec3::new(
                horizontal_normal * cos0,
                vertical_normal,
                horizontal_normal * sin0,
            )
            .normalized()
            .ok_or(MeshError::InvalidPrimitiveParameters)?;
            let normal1 = Vec3::new(
                horizontal_normal * cos1,
                vertical_normal,
                horizontal_normal * sin1,
            )
            .normalized()
            .ok_or(MeshError::InvalidPrimitiveParameters)?;
            let apex_normal = Vec3::new(
                horizontal_normal * cos_middle,
                vertical_normal,
                horizontal_normal * sin_middle,
            )
            .normalized()
            .ok_or(MeshError::InvalidPrimitiveParameters)?;
            let first = u32::try_from(vertices.len()).map_err(|_| MeshError::TooLarge)?;
            vertices.extend([
                Vertex {
                    position: Vec3::new(radius * cos0, -half_height, radius * sin0),
                    normal: normal0,
                    uv: Vec2::new(u0, 1.0),
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::new(0.0, half_height, 0.0),
                    normal: apex_normal,
                    uv: Vec2::new((u0 + u1) * 0.5, 0.0),
                    color: [1.0; 4],
                },
                Vertex {
                    position: Vec3::new(radius * cos1, -half_height, radius * sin1),
                    normal: normal1,
                    uv: Vec2::new(u1, 1.0),
                    color: [1.0; 4],
                },
            ]);
            indices.extend([first, first + 1, first + 2]);
        }
        append_cap(
            &mut vertices,
            &mut indices,
            radius,
            -half_height,
            segments,
            false,
        )?;
        Self::new(vertices, indices)
    }

    /// Validates and creates immutable triangle geometry.
    ///
    /// Empty meshes are valid and produce no draw. Non-empty index buffers must contain complete
    /// triangles and address vertices directly. Positions, normals, UVs, and colors must be
    /// finite; normals are not normalized automatically.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError`] for oversized buffers, incomplete triangles, non-finite attributes,
    /// or out-of-range indices.
    pub fn new(
        vertices: impl Into<Box<[Vertex]>>,
        indices: impl Into<Box<[u32]>>,
    ) -> Result<Self, MeshError> {
        let vertices = vertices.into();
        let indices = indices.into();
        let parts = if indices.is_empty() {
            Vec::new()
        } else {
            vec![MeshPart::new(
                0,
                u32::try_from(indices.len()).map_err(|_| MeshError::TooLarge)?,
                0,
            )]
        };
        Self::with_parts(vertices, indices, parts)
    }

    /// Creates immutable geometry with contiguous material parts.
    ///
    /// Parts must be non-empty, triangle-aligned, ordered, non-overlapping, and cover the entire
    /// index buffer. An empty mesh must have no parts. Material slots are resolved against the
    /// materials attached to a scene node; an unbound slot uses the default material.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError::InvalidParts`] when the ranges do not form complete contiguous
    /// coverage. Other errors match [`Mesh::new`].
    pub fn with_parts(
        vertices: impl Into<Box<[Vertex]>>,
        indices: impl Into<Box<[u32]>>,
        parts: impl Into<Box<[MeshPart]>>,
    ) -> Result<Self, MeshError> {
        let vertices = vertices.into();
        let indices = indices.into();
        let parts = parts.into();
        if u32::try_from(vertices.len()).is_err() || u32::try_from(indices.len()).is_err() {
            return Err(MeshError::TooLarge);
        }
        if indices.len() % 3 != 0 {
            return Err(MeshError::IncompleteTriangles);
        }
        if vertices.iter().any(|vertex| {
            !vertex.position.is_finite()
                || !vertex.normal.is_finite()
                || !vertex.uv.x.is_finite()
                || !vertex.uv.y.is_finite()
                || vertex.color.iter().any(|component| !component.is_finite())
        }) {
            return Err(MeshError::NonFiniteVertex);
        }
        if indices
            .iter()
            .any(|index| usize::try_from(*index).map_or(true, |index| index >= vertices.len()))
        {
            return Err(MeshError::IndexOutOfBounds);
        }
        let mut expected_first_index = 0_u32;
        let mut part_bounds = Vec::with_capacity(parts.len());
        for part in &parts {
            let end = part
                .first_index
                .checked_add(part.index_count)
                .ok_or(MeshError::InvalidParts)?;
            if part.first_index != expected_first_index
                || part.index_count == 0
                || part.first_index % 3 != 0
                || part.index_count % 3 != 0
                || usize::try_from(end).map_or(true, |end| end > indices.len())
            {
                return Err(MeshError::InvalidParts);
            }
            part_bounds.push(range_bounds(
                &vertices,
                &indices,
                part.first_index,
                part.index_count,
            ));
            expected_first_index = end;
        }
        if usize::try_from(expected_first_index).ok() != Some(indices.len()) {
            return Err(MeshError::InvalidParts);
        }
        let bounds: Option<Aabb> =
            vertices
                .iter()
                .map(|vertex| vertex.position)
                .fold(None, |bounds, point| {
                    Some(match bounds {
                        Some(bounds) => Aabb {
                            min: bounds.min.min(point),
                            max: bounds.max.max(point),
                        },
                        None => Aabb {
                            min: point,
                            max: point,
                        },
                    })
                });
        let mut triangle_order = Vec::with_capacity(indices.len() / 3);
        for triangle in 0..indices.len() / 3 {
            triangle_order.push(u32::try_from(triangle).map_err(|_| MeshError::TooLarge)?);
        }
        let mut bvh = Vec::new();
        if !triangle_order.is_empty() {
            build_bvh(&vertices, &indices, &mut triangle_order, 0, &mut bvh)?;
        }
        let uses_uv = mesh_uses_uv_regions(&vertices);

        Ok(Self {
            id: MeshId(NEXT_MESH_ID.fetch_add(1, Ordering::Relaxed)),
            generation: 0,
            vertices,
            tangents: None,
            indices,
            edge_masks: None,
            parts,
            part_bounds: part_bounds.into_boxed_slice(),
            bounds,
            bvh: bvh.into_boxed_slice(),
            triangle_order: triangle_order.into_boxed_slice(),
            uses_uv,
        })
    }

    /// Returns a new immutable snapshot with updated vertex attributes and unchanged topology.
    ///
    /// The vertex count must remain fixed. Indices, material parts, and triangle edge masks are
    /// preserved; mesh and part bounds plus the ray-query BVH are rebuilt. Tangent frames are
    /// cleared because changed positions, normals, or texture coordinates can invalidate them.
    /// The returned mesh receives a fresh render-resource identity, so existing snapshots remain
    /// usable and the renderer uploads the new geometry as a whole.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError::VertexCountMismatch`] when the vertex count changes, or
    /// [`MeshError::NonFiniteVertex`] when an updated attribute is not finite.
    pub fn with_vertices(&self, vertices: impl Into<Box<[Vertex]>>) -> Result<Self, MeshError> {
        let vertices = vertices.into();
        if vertices.len() != self.vertices.len() {
            return Err(MeshError::VertexCountMismatch);
        }

        let mut mesh = Self::with_parts(vertices, self.indices.clone(), self.parts.clone())?;
        mesh.edge_masks.clone_from(&self.edge_masks);
        Ok(mesh)
    }

    /// Stable identity used to recognize this geometry across scene snapshots.
    pub fn id(&self) -> MeshId {
        self.id
    }

    /// Geometry revision associated with this mesh identity.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns a new mesh value with a caller-managed identity and revision.
    ///
    /// The id must be unique among live meshes submitted to one renderer. Increment the
    /// generation whenever the geometry changes.
    pub fn with_identity(mut self, id: MeshId, generation: u64) -> Self {
        self.id = id;
        self.generation = generation;
        self
    }

    /// Declares that this mesh's UVs address real texture regions.
    ///
    /// Scene views detect non-zero UVs automatically. An all-zero UV stream cannot address a
    /// texture region, so a draw with no UV coverage keeps its authored vertex colors even when the
    /// scene view binds an albedo asset.
    #[must_use]
    pub fn with_uv_regions(mut self, uses_uv: bool) -> Self {
        self.uses_uv = uses_uv && mesh_uses_uv_regions(&self.vertices);
        self
    }

    /// Whether this mesh's UVs address texture regions.
    #[must_use]
    pub const fn uses_uv_regions(&self) -> bool {
        self.uses_uv
    }

    /// Replaces vertex normals with area-weighted normals from the indexed triangles.
    ///
    /// Vertices shared by adjacent faces receive smoothed normals. Duplicate vertices preserve
    /// hard edges. Empty meshes are returned unchanged. Existing tangent frames are cleared
    /// because they no longer match the regenerated normals. A non-empty mesh receives a fresh
    /// identity because the uploaded vertex attributes changed.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError::InvalidGeneratedNormals`] when a triangle has zero, negligible, or
    /// non-finite area, or a vertex has no usable accumulated normal.
    pub fn generate_normals(mut self) -> Result<Self, MeshError> {
        if self.indices.is_empty() {
            return Ok(self);
        }

        let mut normals = vec![Vec3::ZERO; self.vertices.len()];
        for triangle in self.indices.chunks_exact(3) {
            let first = self.vertices[triangle[0] as usize].position;
            let second = self.vertices[triangle[1] as usize].position;
            let third = self.vertices[triangle[2] as usize].position;
            let face_normal = (second - first).cross(third - first);
            if !face_normal.is_finite() || face_normal.normalized().is_none() {
                return Err(MeshError::InvalidGeneratedNormals);
            }
            for index in triangle {
                normals[*index as usize] = normals[*index as usize] + face_normal;
            }
        }

        for (vertex, normal) in self.vertices.iter_mut().zip(normals) {
            vertex.normal = normal
                .normalized()
                .ok_or(MeshError::InvalidGeneratedNormals)?;
        }
        self.tangents = None;
        self.id = MeshId(NEXT_MESH_ID.fetch_add(1, Ordering::Relaxed));
        self.generation = 0;
        Ok(self)
    }

    /// Returns a mesh snapshot with optional per-triangle silhouette edges.
    ///
    /// Masks follow index-buffer triangle order, not the reordered BVH traversal order. During
    /// GPU upload, a mesh with masks expands indexed triangles into triangle-local vertices so
    /// each corner can carry barycentric coordinates. This increases its vertex upload to three
    /// vertices per triangle; meshes without masks retain indexed uploads.
    ///
    /// The returned mesh gets a fresh identity because edge masks change uploaded vertex data.
    /// An empty mask disables coverage smoothing for every triangle.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError::InvalidTriangleEdgeMasks`] unless there is exactly one mask per
    /// triangle in the index buffer.
    pub fn with_edge_masks(
        mut self,
        masks: impl Into<Box<[TriangleEdgeMask]>>,
    ) -> Result<Self, MeshError> {
        let masks = masks.into();
        if masks.len() != self.indices.len() / 3 {
            return Err(MeshError::InvalidTriangleEdgeMasks);
        }
        self.id = MeshId(NEXT_MESH_ID.fetch_add(1, Ordering::Relaxed));
        self.generation = 0;
        self.edge_masks = Some(masks);
        Ok(self)
    }

    /// Vertices in mesh-local coordinates.
    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    /// Returns tangent frames aligned with [`Mesh::vertices`], when present.
    ///
    /// Each entry is `[tangent_x, tangent_y, tangent_z, handedness]`. The xyz direction is
    /// orthogonal to the vertex normal and normalized; handedness is `-1` or `1` and determines
    /// the bitangent as `normal.cross(tangent) * handedness`. Tangents use texture coordinate set
    /// zero and are consumed by the scene-view normal-map shader.
    #[must_use]
    pub fn tangents(&self) -> Option<&[[f32; 4]]> {
        self.tangents.as_deref()
    }

    /// Triangle indices.
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// Per-triangle edge coverage masks in index-buffer order, when enabled.
    #[must_use]
    pub fn edge_masks(&self) -> Option<&[TriangleEdgeMask]> {
        self.edge_masks.as_deref()
    }

    /// Contiguous triangle parts in index-buffer draw order.
    #[must_use]
    pub fn parts(&self) -> &[MeshPart] {
        &self.parts
    }

    /// Returns the material part containing a triangle's first index.
    #[must_use]
    pub fn part_for_triangle(&self, triangle: u32) -> Option<MeshPart> {
        let first_index = triangle.checked_mul(3)?;
        let part_index = self
            .parts
            .partition_point(|part| part.first_index <= first_index)
            .checked_sub(1)?;
        self.parts
            .get(part_index)
            .filter(|part| first_index < part.first_index + part.index_count)
            .copied()
    }

    /// Local-space bounds, or `None` for an empty mesh.
    pub fn bounds(&self) -> Option<Aabb> {
        self.bounds
    }

    pub(crate) fn part_bounds(&self, index: usize) -> Option<Aabb> {
        self.part_bounds.get(index).copied()
    }

    pub(crate) fn bvh(&self) -> &[BvhNode] {
        &self.bvh
    }

    pub(crate) fn triangle_order(&self) -> &[u32] {
        &self.triangle_order
    }
}

const BVH_LEAF_SIZE: usize = 8;

#[allow(
    clippy::cast_precision_loss,
    reason = "primitive segment counts are memory-bound well below the point where f32 UV precision changes materially"
)]
fn radial_mesh(
    bottom_radius: f32,
    top_radius: f32,
    height: f32,
    segments: u32,
) -> Result<Mesh, MeshError> {
    if !bottom_radius.is_finite()
        || bottom_radius <= 0.0
        || !top_radius.is_finite()
        || top_radius <= 0.0
        || !height.is_finite()
        || height <= 0.0
        || segments < 3
    {
        return Err(MeshError::InvalidPrimitiveParameters);
    }
    let segments = usize::try_from(segments).map_err(|_| MeshError::TooLarge)?;
    let vertex_count = segments
        .checked_mul(4)
        .and_then(|count| count.checked_add(6))
        .ok_or(MeshError::TooLarge)?;
    let index_count = segments.checked_mul(12).ok_or(MeshError::TooLarge)?;
    if u32::try_from(vertex_count).is_err() || u32::try_from(index_count).is_err() {
        return Err(MeshError::TooLarge);
    }

    let mut vertices = Vec::with_capacity(vertex_count);
    let mut indices = Vec::with_capacity(index_count);
    let half_height = height * 0.5;
    let vertical_slope = bottom_radius - top_radius;
    let normal_scale = height.max(vertical_slope.abs());
    let horizontal_normal = height / normal_scale;
    let vertical_normal = vertical_slope / normal_scale;
    for row in 0..=1 {
        let (radius, y) = if row == 0 {
            (bottom_radius, -half_height)
        } else {
            (top_radius, half_height)
        };
        for segment in 0..=segments {
            let u = segment as f32 / segments as f32;
            let theta = u * std::f32::consts::TAU;
            let (sin_theta, cos_theta) = theta.sin_cos();
            let normal = Vec3::new(
                horizontal_normal * cos_theta,
                vertical_normal,
                horizontal_normal * sin_theta,
            )
            .normalized()
            .ok_or(MeshError::InvalidPrimitiveParameters)?;
            vertices.push(Vertex {
                position: Vec3::new(radius * cos_theta, y, radius * sin_theta),
                normal,
                uv: Vec2::new(u, 1.0 - row as f32),
                color: [1.0; 4],
            });
        }
    }
    let row_stride = u32::try_from(segments + 1).map_err(|_| MeshError::TooLarge)?;
    for segment in 0..segments {
        let bottom = u32::try_from(segment).map_err(|_| MeshError::TooLarge)?;
        let top = bottom + row_stride;
        let next = bottom + 1;
        indices.extend([bottom, top, next, next, top, top + 1]);
    }
    append_cap(
        &mut vertices,
        &mut indices,
        bottom_radius,
        -half_height,
        segments,
        false,
    )?;
    append_cap(
        &mut vertices,
        &mut indices,
        top_radius,
        half_height,
        segments,
        true,
    )?;
    Mesh::new(vertices, indices)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "primitive segment counts are memory-bound well below the point where f32 UV precision changes materially"
)]
fn append_cap(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    radius: f32,
    y: f32,
    segments: usize,
    top: bool,
) -> Result<(), MeshError> {
    let first = u32::try_from(vertices.len()).map_err(|_| MeshError::TooLarge)?;
    let normal = if top { Vec3::Y } else { -Vec3::Y };
    vertices.push(Vertex {
        position: Vec3::new(0.0, y, 0.0),
        normal,
        uv: Vec2::new(0.5, 0.5),
        color: [1.0; 4],
    });
    for segment in 0..=segments {
        let angle = segment as f32 / segments as f32 * std::f32::consts::TAU;
        let (sin_angle, cos_angle) = angle.sin_cos();
        vertices.push(Vertex {
            position: Vec3::new(radius * cos_angle, y, radius * sin_angle),
            normal,
            uv: Vec2::new(cos_angle * 0.5 + 0.5, sin_angle * 0.5 + 0.5),
            color: [1.0; 4],
        });
    }
    for segment in 0..segments {
        let center = first;
        let current = first + 1 + u32::try_from(segment).map_err(|_| MeshError::TooLarge)?;
        let next = current + 1;
        if top {
            indices.extend([center, next, current]);
        } else {
            indices.extend([center, current, next]);
        }
    }
    Ok(())
}

fn build_bvh(
    vertices: &[Vertex],
    indices: &[u32],
    triangles: &mut [u32],
    first_triangle: usize,
    nodes: &mut Vec<BvhNode>,
) -> Result<u32, MeshError> {
    let bounds = triangles
        .iter()
        .map(|triangle| triangle_bounds(vertices, indices, *triangle))
        .reduce(union_bounds)
        .expect("BVH nodes contain at least one triangle");
    let node_index = u32::try_from(nodes.len()).map_err(|_| MeshError::TooLarge)?;
    nodes.push(BvhNode::Leaf {
        bounds,
        first_triangle: 0,
        triangle_count: 0,
    });
    if triangles.len() <= BVH_LEAF_SIZE {
        nodes[node_index as usize] = BvhNode::Leaf {
            bounds,
            first_triangle: u32::try_from(first_triangle).map_err(|_| MeshError::TooLarge)?,
            triangle_count: u32::try_from(triangles.len()).map_err(|_| MeshError::TooLarge)?,
        };
        return Ok(node_index);
    }

    let centroid_bounds = triangles
        .iter()
        .map(|triangle| triangle_bounds(vertices, indices, *triangle))
        .map(|bounds| (bounds.min + bounds.max) * 0.5)
        .map(|center| Aabb {
            min: center,
            max: center,
        })
        .reduce(|left, right| Aabb {
            min: left.min.min(right.min),
            max: left.max.max(right.max),
        })
        .expect("BVH nodes contain at least one triangle");
    let extent = centroid_bounds.max - centroid_bounds.min;
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    triangles.sort_unstable_by(|left, right| {
        triangle_centroid(vertices, indices, *left)[axis]
            .total_cmp(&triangle_centroid(vertices, indices, *right)[axis])
    });
    let midpoint = triangles.len() / 2;
    let (left_triangles, right_triangles) = triangles.split_at_mut(midpoint);
    let left = build_bvh(vertices, indices, left_triangles, first_triangle, nodes)?;
    let right = build_bvh(
        vertices,
        indices,
        right_triangles,
        first_triangle + midpoint,
        nodes,
    )?;
    nodes[node_index as usize] = BvhNode::Branch {
        bounds,
        left,
        right,
    };
    Ok(node_index)
}

fn triangle_bounds(vertices: &[Vertex], indices: &[u32], triangle: u32) -> Aabb {
    let index = usize::try_from(triangle).expect("triangle index fits usize") * 3;
    let a = vertices[indices[index] as usize].position;
    let b = vertices[indices[index + 1] as usize].position;
    let c = vertices[indices[index + 2] as usize].position;
    Aabb {
        min: a.min(b).min(c),
        max: a.max(b).max(c),
    }
}

fn range_bounds(vertices: &[Vertex], indices: &[u32], first_index: u32, index_count: u32) -> Aabb {
    let first = usize::try_from(first_index).expect("validated mesh part start fits usize");
    let end = first + usize::try_from(index_count).expect("validated mesh part count fits usize");
    let mut points = indices[first..end]
        .iter()
        .map(|index| vertices[*index as usize].position);
    let first_point = points.next().expect("validated mesh part is non-empty");
    let (min, max) = points.fold((first_point, first_point), |(min, max), point| {
        (min.min(point), max.max(point))
    });
    Aabb { min, max }
}

fn triangle_centroid(vertices: &[Vertex], indices: &[u32], triangle: u32) -> [f32; 3] {
    let index = triangle as usize * 3;
    let a = vertices[indices[index] as usize].position;
    let b = vertices[indices[index + 1] as usize].position;
    let c = vertices[indices[index + 2] as usize].position;
    let center = (a + b + c) / 3.0;
    [center.x, center.y, center.z]
}

fn union_bounds(left: Aabb, right: Aabb) -> Aabb {
    Aabb {
        min: left.min.min(right.min),
        max: left.max.max(right.max),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, Ray, RaycastScratch, Scene};
    use std::sync::Arc;

    fn vertex(position: Vec3) -> Vertex {
        Vertex {
            position,
            normal: Vec3::Y,
            uv: Vec2::new(0.0, 0.0),
            color: [1.0; 4],
        }
    }

    #[test]
    fn validates_indices_and_computes_bounds() {
        let mesh = Mesh::new(
            vec![
                vertex(Vec3::new(-1.0, 2.0, 0.0)),
                vertex(Vec3::new(3.0, -2.0, 1.0)),
                vertex(Vec3::ZERO),
            ],
            vec![0, 1, 2],
        )
        .unwrap();
        assert_eq!(
            mesh.bounds(),
            Aabb::new(Vec3::new(-1.0, -2.0, 0.0), Vec3::new(3.0, 2.0, 1.0))
        );
        assert_eq!(
            Mesh::new(vec![vertex(Vec3::ZERO)], vec![1, 0, 0]).unwrap_err(),
            MeshError::IndexOutOfBounds
        );
    }

    #[test]
    fn vertex_update_preserves_topology_and_rebuilds_query_data() {
        let vertices = [
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
                position: Vec3::new(2.0, 0.0, 0.0),
                normal: Vec3::Z,
                uv: Vec2::ZERO,
                color: [1.0; 4],
            },
            Vertex {
                position: Vec3::new(3.0, 0.0, 0.0),
                normal: Vec3::Z,
                uv: Vec2::new(1.0, 0.0),
                color: [1.0; 4],
            },
            Vertex {
                position: Vec3::new(2.0, 1.0, 0.0),
                normal: Vec3::Z,
                uv: Vec2::new(0.0, 1.0),
                color: [1.0; 4],
            },
        ];
        let original = Mesh::with_parts(
            vertices,
            [0, 1, 2, 3, 4, 5],
            [MeshPart::new(0, 3, 0), MeshPart::new(3, 3, 1)],
        )
        .unwrap()
        .with_edge_masks([TriangleEdgeMask::ALL, TriangleEdgeMask::NONE])
        .unwrap()
        .with_tangents([[1.0, 0.0, 0.0, 1.0]; 6])
        .unwrap();
        let old_id = original.id();
        let mut updated_vertices = original.vertices().to_vec();
        for vertex in &mut updated_vertices[..3] {
            vertex.position.x += 5.0;
        }

        let updated = original.with_vertices(updated_vertices).unwrap();

        assert_ne!(updated.id(), old_id);
        assert_eq!(
            original.bounds(),
            Aabb::new(Vec3::ZERO, Vec3::new(3.0, 1.0, 0.0))
        );
        assert_eq!(
            updated.bounds(),
            Aabb::new(Vec3::new(2.0, 0.0, 0.0), Vec3::new(6.0, 1.0, 0.0))
        );
        assert_eq!(updated.indices(), original.indices());
        assert_eq!(updated.parts(), original.parts());
        assert_eq!(
            updated.part_bounds(0),
            Aabb::new(Vec3::new(5.0, 0.0, 0.0), Vec3::new(6.0, 1.0, 0.0))
        );
        assert_eq!(updated.edge_masks(), original.edge_masks());
        assert!(updated.tangents().is_none());

        let mut scene = Scene::new();
        scene
            .insert(None, Node::new().with_mesh(Arc::new(updated)))
            .unwrap();
        let mut scratch = RaycastScratch::new();
        let old_ray = Ray::new(Vec3::new(0.25, 0.25, 1.0), -Vec3::Z).unwrap();
        let new_ray = Ray::new(Vec3::new(5.25, 0.25, 1.0), -Vec3::Z).unwrap();
        assert!(scene.raycast(old_ray, &mut scratch).unwrap().is_none());
        assert!(scene.raycast(new_ray, &mut scratch).unwrap().is_some());
    }

    #[test]
    fn vertex_update_rejects_count_changes_and_non_finite_attributes() {
        let mesh = Mesh::cube();
        assert_eq!(
            mesh.with_vertices(mesh.vertices()[..mesh.vertices().len() - 1].to_vec())
                .unwrap_err(),
            MeshError::VertexCountMismatch
        );

        let mut vertices = mesh.vertices().to_vec();
        vertices[0].position.x = f32::NAN;
        assert_eq!(
            mesh.with_vertices(vertices).unwrap_err(),
            MeshError::NonFiniteVertex
        );
    }

    #[test]
    fn edge_masks_follow_index_buffer_triangle_order_and_get_a_fresh_cache_identity() {
        let mesh = Mesh::new(
            vec![
                vertex(Vec3::ZERO),
                vertex(Vec3::X),
                vertex(Vec3::Y),
                vertex(Vec3::new(1.0, 1.0, 0.0)),
            ],
            vec![0, 1, 2, 0, 2, 3],
        )
        .unwrap();
        let id = mesh.id();
        let mesh = mesh
            .with_edge_masks(vec![
                TriangleEdgeMask::new([true, false, true]),
                TriangleEdgeMask::NONE,
            ])
            .unwrap();

        assert_ne!(mesh.id(), id);
        assert_eq!(mesh.edge_masks().unwrap().len(), 2);
        assert_eq!(mesh.edge_masks().unwrap()[0].bits(), 0b101);
        assert_eq!(mesh.edge_masks().unwrap()[1], TriangleEdgeMask::NONE);
    }

    #[test]
    fn edge_masks_must_cover_every_triangle() {
        let mesh = Mesh::new(
            vec![vertex(Vec3::ZERO), vertex(Vec3::X), vertex(Vec3::Y)],
            vec![0, 1, 2],
        )
        .unwrap();

        assert_eq!(
            mesh.with_edge_masks(Vec::<TriangleEdgeMask>::new())
                .unwrap_err(),
            MeshError::InvalidTriangleEdgeMasks
        );
    }

    #[test]
    fn generated_normals_are_area_weighted_and_refresh_cache_identity() {
        let mut vertices = [
            vertex(Vec3::ZERO),
            vertex(Vec3::X),
            vertex(Vec3::Y),
            vertex(Vec3::Z),
        ];
        for vertex in &mut vertices {
            vertex.normal = Vec3::ZERO;
        }
        let mesh = Mesh::new(vertices, [0, 1, 2, 0, 2, 3])
            .unwrap()
            .with_edge_masks([TriangleEdgeMask::ALL, TriangleEdgeMask::NONE])
            .unwrap();
        let old_id = mesh.id();
        let mesh = mesh.generate_normals().unwrap();

        assert_ne!(mesh.id(), old_id);
        assert_eq!(mesh.vertices()[1].normal, Vec3::Z);
        assert_eq!(mesh.vertices()[3].normal, Vec3::X);
        let smoothed = Vec3::new(1.0, 0.0, 1.0) / 2.0_f32.sqrt();
        assert!((mesh.vertices()[0].normal - smoothed).length() < 1.0e-5);
        assert!((mesh.vertices()[2].normal - smoothed).length() < 1.0e-5);
        assert_eq!(mesh.edge_masks().unwrap().len(), 2);
    }

    #[test]
    fn normal_generation_rejects_degenerate_faces_and_unreferenced_vertices() {
        let degenerate = Mesh::new(
            [
                vertex(Vec3::ZERO),
                vertex(Vec3::X),
                vertex(Vec3::new(2.0, 0.0, 0.0)),
            ],
            [0, 1, 2],
        )
        .unwrap();
        assert_eq!(
            degenerate.generate_normals().unwrap_err(),
            MeshError::InvalidGeneratedNormals
        );

        let unreferenced = Mesh::new(
            [
                vertex(Vec3::ZERO),
                vertex(Vec3::X),
                vertex(Vec3::Y),
                vertex(Vec3::Z),
            ],
            [0, 1, 2],
        )
        .unwrap();
        assert_eq!(
            unreferenced.generate_normals().unwrap_err(),
            MeshError::InvalidGeneratedNormals
        );
    }

    #[test]
    fn validates_contiguous_material_parts() {
        let vertices = vec![
            vertex(Vec3::ZERO),
            vertex(Vec3::X),
            vertex(Vec3::Y),
            vertex(Vec3::new(2.0, 0.0, 0.0)),
            vertex(Vec3::new(3.0, 0.0, 0.0)),
            vertex(Vec3::new(2.0, 1.0, 0.0)),
        ];
        let mesh = Mesh::with_parts(
            vertices.clone(),
            vec![0, 1, 2, 3, 4, 5],
            vec![MeshPart::new(0, 3, 0), MeshPart::new(3, 3, 1)],
        )
        .unwrap();
        assert_eq!(mesh.parts()[1].material_slot(), 1);
        assert_eq!(
            mesh.part_bounds(1),
            Aabb::new(Vec3::new(2.0, 0.0, 0.0), Vec3::new(3.0, 1.0, 0.0))
        );
        assert_eq!(
            Mesh::with_parts(
                vertices,
                vec![0, 1, 2, 3, 4, 5],
                vec![MeshPart::new(0, 3, 0), MeshPart::new(4, 2, 1)],
            )
            .unwrap_err(),
            MeshError::InvalidParts
        );
    }

    #[test]
    fn primitive_meshes_have_finite_outward_normals_and_valid_parts() {
        let cube = Mesh::cube();
        assert_eq!(cube.vertices().len(), 24);
        assert_eq!(cube.indices().len(), 36);
        assert!(
            cube.vertices()
                .iter()
                .all(|vertex| (vertex.normal.length() - 1.0).abs() < f32::EPSILON)
        );
        assert_outward_triangles(&cube);

        let plane = Mesh::plane(4.0, 2.0).unwrap();
        assert_eq!(plane.indices(), [0, 2, 1, 0, 3, 2]);
        assert!(
            plane
                .vertices()
                .iter()
                .all(|vertex| vertex.normal == Vec3::Y)
        );
        assert_outward_triangles(&plane);

        let sphere = Mesh::uv_sphere(2.0, [16, 8]).unwrap();
        assert_eq!(sphere.vertices().len(), 17 * 9);
        assert_eq!(sphere.indices().len(), 16 * 8 * 6);
        assert!(sphere.vertices().iter().all(|vertex| {
            (vertex.normal.length() - 1.0).abs() < 1.0e-5
                && vertex.normal.dot(vertex.position) > 0.0
        }));
        assert_outward_triangles(&sphere);

        let cylinder = Mesh::cylinder(1.0, 2.0, 12).unwrap();
        assert_eq!(cylinder.vertices().len(), 4 * 12 + 6);
        assert_eq!(cylinder.indices().len(), 12 * 12);
        assert_outward_triangles(&cylinder);

        let cone = Mesh::cone(1.0, 2.0, 12).unwrap();
        assert_eq!(cone.vertices().len(), 4 * 12 + 2);
        assert_eq!(cone.indices().len(), 6 * 12);
        assert_outward_triangles(&cone);
    }

    #[test]
    fn primitive_meshes_reject_invalid_dimensions_and_segment_counts() {
        assert_eq!(
            Mesh::plane(f32::NAN, 1.0).unwrap_err(),
            MeshError::InvalidPrimitiveParameters
        );
        assert_eq!(
            Mesh::uv_sphere(0.0, [16, 8]).unwrap_err(),
            MeshError::InvalidPrimitiveParameters
        );
        assert_eq!(
            Mesh::uv_sphere(1.0, [2, 8]).unwrap_err(),
            MeshError::InvalidPrimitiveParameters
        );
        assert_eq!(
            Mesh::cylinder(1.0, 1.0, 2).unwrap_err(),
            MeshError::InvalidPrimitiveParameters
        );
        assert_eq!(
            Mesh::cone(f32::INFINITY, 1.0, 8).unwrap_err(),
            MeshError::InvalidPrimitiveParameters
        );
    }

    fn assert_outward_triangles(mesh: &Mesh) {
        for triangle in mesh.indices().chunks_exact(3) {
            let a = mesh.vertices()[triangle[0] as usize];
            let b = mesh.vertices()[triangle[1] as usize];
            let c = mesh.vertices()[triangle[2] as usize];
            let face_normal = (b.position - a.position).cross(c.position - a.position);
            if face_normal.length_squared() > 1.0e-10 {
                let vertex_normal = a.normal + b.normal + c.normal;
                assert!(face_normal.dot(vertex_normal) > 0.0);
            }
        }
    }
}
