//! Indexed mesh snapshots for Bedrock selections.

use super::*;

pub(super) const GPU_BUFFER_BUDGET_BYTES: usize = 64 * 1024 * 1024;
pub(super) const GPU_VERTEX_BUDGET: usize = GPU_BUFFER_BUDGET_BYTES / std::mem::size_of::<Vertex>();
pub(super) const FACE_BUDGET: usize = GPU_VERTEX_BUDGET / 4;
pub(super) const TOTAL_FACE_BUDGET: usize = FACE_BUDGET;
pub(super) const TOTAL_VERTEX_BUDGET: usize = TOTAL_FACE_BUDGET * 4;
pub(super) const VERTICAL_SCALE: f32 = 1.0;
pub(super) const GLASS_FACE_BUDGET: usize = TOTAL_FACE_BUDGET / 5;
pub(super) const WATER_FACE_BUDGET: usize = TOTAL_FACE_BUDGET / 5;
pub(super) fn opaque_face_budget(
    glass_faces: usize,
    water_faces: usize,
    lava_faces: usize,
) -> usize {
    TOTAL_FACE_BUDGET
        .saturating_sub(glass_faces)
        .saturating_sub(water_faces)
        .saturating_sub(lava_faces)
}
#[derive(Clone, Debug)]
/// A selection snapshot with CPU-side mesh data and construction statistics.
pub struct SceneMesh {
    pub chunk_meshes: Vec<ChunkMesh>,
    pub min_y: i16,
    pub max_y: i16,
    pub min_x: i32,
    pub max_x: i32,
    pub min_z: i32,
    pub max_z: i32,
    pub missing_chunks: usize,
    pub chunk_count: usize,
    pub processed_chunk_count: usize,
    pub subchunk_count: usize,
    pub solid_block_count: usize,
    pub glass_block_count: usize,
    pub water_block_count: usize,
    pub lava_block_count: usize,
    pub face_count: usize,
    pub glass_face_count: usize,
    pub water_face_count: usize,
    pub lava_face_count: usize,
    pub culled_face_count: usize,
    pub omitted_face_count: usize,
    pub truncated_chunk_count: usize,
    pub vertex_budget: usize,
}

impl SceneMesh {
    pub fn vertex_count(&self) -> usize {
        self.chunk_meshes
            .iter()
            .map(|mesh| mesh.mesh.vertices().len())
            .sum()
    }

    pub fn chunk_mesh_count(&self) -> usize {
        self.chunk_meshes.len()
    }

    pub fn estimated_cpu_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(
                self.chunk_meshes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<ChunkMesh>()),
            )
            .saturating_add(
                self.chunk_meshes
                    .iter()
                    .map(ChunkMesh::estimated_cpu_bytes)
                    .sum::<usize>(),
            )
    }

    pub fn rendered_chunk_count(&self) -> usize {
        self.processed_chunk_count
            .saturating_sub(self.missing_chunks)
            .min(self.chunk_count)
    }

    pub fn surface_face_count(&self) -> usize {
        self.face_count
            .saturating_add(self.glass_face_count)
            .saturating_add(self.water_face_count)
            .saturating_add(self.lava_face_count)
    }

    pub fn horizontal_span(&self) -> f32 {
        if self.chunk_count == 0 {
            return 1.0;
        }
        let span_x = self
            .max_x
            .saturating_sub(self.min_x)
            .saturating_add(1)
            .max(1) as f32;
        let span_z = self
            .max_z
            .saturating_sub(self.min_z)
            .saturating_add(1)
            .max(1) as f32;
        span_x.max(span_z)
    }
}

#[derive(Clone, Debug)]
/// A mesh batch with its world origin and material metadata.
pub struct ChunkMesh {
    pub mesh: Arc<Mesh>,
    pub ranges: MaterialRanges,
    pub center: [f32; 3],
    pub fit_scale: f32,
    pub world_origin: [i32; 3],
    pub face_metadata: Arc<[FaceMetadata]>,
}

#[derive(Clone, Copy, Debug, Default)]
/// Index ranges by material pass.
pub struct MaterialRanges {
    pub opaque: IndexRange,
    pub glass: IndexRange,
    pub water: IndexRange,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// A contiguous range in the index buffer.
pub struct IndexRange {
    pub start: u32,
    pub count: u32,
}

impl ChunkMesh {
    pub fn estimated_cpu_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(std::mem::size_of::<Mesh>())
            .saturating_add(
                self.mesh
                    .vertices()
                    .len()
                    .saturating_mul(std::mem::size_of::<Vertex>()),
            )
            .saturating_add(
                self.mesh
                    .indices()
                    .len()
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
            .saturating_add(
                self.face_metadata
                    .len()
                    .saturating_mul(std::mem::size_of::<FaceMetadata>()),
            )
    }
}

#[derive(Clone, Debug)]
/// Material name and optional per-face texture coordinates.
pub struct FaceMetadata {
    pub material: MaterialName,
    pub uv: Option<[[f32; 2]; 4]>,
}
pub(super) fn fit_scale(horizontal_span: f32, vertical_span: f32) -> f32 {
    let fitted_span = horizontal_span.max(vertical_span * VERTICAL_SCALE * 1.25);
    1.48 / fitted_span.max(1.0)
}
#[cfg(test)]
pub(super) fn build_meshes(
    opaque_faces: &[Face],
    glass_faces: &[Face],
    water_faces: &[Face],
    lava_faces: &[Face],
    center: [f32; 3],
    horizontal_span: f32,
    vertical_span: f32,
    generation: u64,
) -> Result<Vec<ChunkMesh>, String> {
    let world_origin = [
        center[0].floor() as i32,
        center[1].floor() as i32,
        center[2].floor() as i32,
    ];
    let local_center = [
        center[0] - world_origin[0] as f32,
        center[1] - world_origin[1] as f32,
        center[2] - world_origin[2] as f32,
    ];
    build_meshes_at_world_origin(
        opaque_faces,
        glass_faces,
        water_faces,
        lava_faces,
        world_origin,
        local_center,
        horizontal_span,
        vertical_span,
        generation,
    )
}

pub(super) fn build_meshes_at_world_origin(
    opaque_faces: &[Face],
    glass_faces: &[Face],
    water_faces: &[Face],
    lava_faces: &[Face],
    world_origin: [i32; 3],
    local_center: [f32; 3],
    horizontal_span: f32,
    vertical_span: f32,
    generation: u64,
) -> Result<Vec<ChunkMesh>, String> {
    let fit_scale = fit_scale(horizontal_span, vertical_span);
    let mut meshes = Vec::new();
    for slice in face_slices(opaque_faces, glass_faces, water_faces, lava_faces) {
        meshes.push(build_mesh_from_slices(
            slice.opaque,
            slice.glass,
            slice.water,
            slice.lava,
            world_origin,
            local_center,
            fit_scale,
            generation,
        )?);
    }
    Ok(meshes)
}

pub(super) fn face_slices<'a>(
    opaque_faces: &'a [Face],
    glass_faces: &'a [Face],
    water_faces: &'a [Face],
    lava_faces: &'a [Face],
) -> Vec<FaceSlice<'a>> {
    let max_faces = FACE_BUDGET.max(1);
    let mut slices = Vec::new();
    let mut opaque_start = 0;
    let mut glass_start = 0;
    let mut water_start = 0;
    let mut lava_start = 0;
    while opaque_start < opaque_faces.len()
        || glass_start < glass_faces.len()
        || water_start < water_faces.len()
        || lava_start < lava_faces.len()
    {
        let opaque_remaining = opaque_faces.len() - opaque_start;
        let glass_remaining = glass_faces.len() - glass_start;
        let water_remaining = water_faces.len() - water_start;
        let lava_remaining = lava_faces.len() - lava_start;
        let opaque_count = opaque_remaining.min(max_faces);
        let remaining_faces = max_faces.saturating_sub(opaque_count);
        let glass_count = glass_remaining.min(remaining_faces);
        let remaining_faces = remaining_faces.saturating_sub(glass_count);
        let water_count = water_remaining.min(remaining_faces);
        let remaining_faces = remaining_faces.saturating_sub(water_count);
        let lava_count = lava_remaining.min(remaining_faces);

        slices.push(FaceSlice {
            opaque: &opaque_faces[opaque_start..opaque_start + opaque_count],
            glass: &glass_faces[glass_start..glass_start + glass_count],
            water: &water_faces[water_start..water_start + water_count],
            lava: &lava_faces[lava_start..lava_start + lava_count],
        });
        opaque_start += opaque_count;
        glass_start += glass_count;
        water_start += water_count;
        lava_start += lava_count;
    }
    slices
}

pub(super) struct FaceSlice<'a> {
    opaque: &'a [Face],
    glass: &'a [Face],
    water: &'a [Face],
    lava: &'a [Face],
}

#[cfg(test)]
pub(super) fn build_mesh(
    opaque_faces: &[Face],
    glass_faces: &[Face],
    water_faces: &[Face],
    center: [f32; 3],
    horizontal_span: f32,
    vertical_span: f32,
    generation: u64,
) -> Result<Mesh, String> {
    let fit_scale = fit_scale(horizontal_span, vertical_span);
    let world_origin = [
        center[0].floor() as i32,
        center[1].floor() as i32,
        center[2].floor() as i32,
    ];
    let local_center = [
        center[0] - world_origin[0] as f32,
        center[1] - world_origin[1] as f32,
        center[2] - world_origin[2] as f32,
    ];
    Ok(build_mesh_from_slices(
        opaque_faces,
        glass_faces,
        water_faces,
        &[],
        world_origin,
        local_center,
        fit_scale,
        generation,
    )
    .map(|chunk_mesh| chunk_mesh.mesh.as_ref().clone())?)
}

pub(super) fn build_mesh_from_slices(
    opaque_faces: &[Face],
    glass_faces: &[Face],
    water_faces: &[Face],
    lava_faces: &[Face],
    world_origin: [i32; 3],
    local_center: [f32; 3],
    fit_scale: f32,
    generation: u64,
) -> Result<ChunkMesh, String> {
    let face_count = opaque_faces
        .len()
        .saturating_add(glass_faces.len())
        .saturating_add(water_faces.len())
        .saturating_add(lava_faces.len());
    let vertex_count = face_count.saturating_mul(4);
    let index_count = face_count.saturating_mul(6);
    if vertex_count > GPU_VERTEX_BUDGET {
        return Err(format!(
            "3D 预览网格过大: 顶点 {vertex_count}，预算 {GPU_VERTEX_BUDGET}"
        ));
    }
    let mut vertices = Vec::with_capacity(vertex_count);
    let mut indices = Vec::with_capacity(index_count);
    let mut face_metadata = Vec::with_capacity(face_count);
    let opaque = push_preview_faces(
        &mut vertices,
        &mut indices,
        &mut face_metadata,
        opaque_faces,
        world_origin,
    );
    let glass = push_preview_faces(
        &mut vertices,
        &mut indices,
        &mut face_metadata,
        glass_faces,
        world_origin,
    );
    let water = push_preview_fluid_faces(
        &mut vertices,
        &mut indices,
        &mut face_metadata,
        water_faces,
        lava_faces,
        world_origin,
    );
    let mesh = Mesh::new(vertices, indices)
        .map_err(|error| format!("invalid 3D preview mesh: {error}"))?;
    let id = mesh.id();
    let mesh = mesh.with_identity(id, generation);
    Ok(ChunkMesh {
        mesh: Arc::new(mesh),
        ranges: MaterialRanges {
            opaque,
            glass,
            water,
        },
        center: local_center,
        fit_scale,
        world_origin,
        face_metadata: Arc::from(face_metadata.into_boxed_slice()),
    })
}

pub(super) fn push_preview_faces(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    face_metadata: &mut Vec<FaceMetadata>,
    faces: &[Face],
    world_origin: [i32; 3],
) -> IndexRange {
    let start = u32::try_from(indices.len()).unwrap_or(u32::MAX);
    for face in faces {
        push_preview_face(vertices, indices, face_metadata, face, world_origin);
    }
    let count = u32::try_from(indices.len().saturating_sub(start as usize)).unwrap_or(u32::MAX);
    IndexRange { start, count }
}

pub(super) fn push_preview_fluid_faces(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    face_metadata: &mut Vec<FaceMetadata>,
    water_faces: &[Face],
    lava_faces: &[Face],
    world_origin: [i32; 3],
) -> IndexRange {
    let start = u32::try_from(indices.len()).unwrap_or(u32::MAX);
    push_preview_faces_matching(
        vertices,
        indices,
        face_metadata,
        water_faces,
        |face| face.normal != [0, 1, 0],
        world_origin,
    );
    push_preview_faces_matching(
        vertices,
        indices,
        face_metadata,
        lava_faces,
        |face| face.normal != [0, 1, 0],
        world_origin,
    );
    push_preview_faces_matching(
        vertices,
        indices,
        face_metadata,
        water_faces,
        |face| face.normal == [0, 1, 0],
        world_origin,
    );
    push_preview_faces_matching(
        vertices,
        indices,
        face_metadata,
        lava_faces,
        |face| face.normal == [0, 1, 0],
        world_origin,
    );
    let count = u32::try_from(indices.len().saturating_sub(start as usize)).unwrap_or(u32::MAX);
    IndexRange { start, count }
}

pub(super) fn push_preview_faces_matching(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    face_metadata: &mut Vec<FaceMetadata>,
    faces: &[Face],
    mut predicate: impl FnMut(&Face) -> bool,
    world_origin: [i32; 3],
) {
    for face in faces {
        if predicate(face) {
            push_preview_face(vertices, indices, face_metadata, face, world_origin);
        }
    }
}

pub(super) fn push_preview_face(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    face_metadata: &mut Vec<FaceMetadata>,
    face: &Face,
    world_origin: [i32; 3],
) {
    let color = shade_preview_color(face.color, face.shade);
    let base_vertex = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
    let normal = Vec3::new(
        face.normal[0] as f32,
        face.normal[1] as f32,
        face.normal[2] as f32,
    )
    .normalized()
    .unwrap_or(Vec3::Y);
    let vertex = |index: usize| Vertex {
        position: Vec3::new(
            (face.corners[index][0] - f64::from(world_origin[0])) as f32,
            (face.corners[index][1] - f64::from(world_origin[1])) as f32,
            (face.corners[index][2] - f64::from(world_origin[2])) as f32,
        ),
        normal,
        uv: face
            .uv
            .map_or(Vec2::ZERO, |uv| Vec2::new(uv[index][0], uv[index][1])),
        color,
    };
    vertices.extend([vertex(0), vertex(1), vertex(2), vertex(3)]);
    indices.extend([
        base_vertex,
        base_vertex.saturating_add(1),
        base_vertex.saturating_add(2),
        base_vertex,
        base_vertex.saturating_add(2),
        base_vertex.saturating_add(3),
    ]);
    face_metadata.push(FaceMetadata {
        material: face.material.clone(),
        uv: face.uv,
    });
}
