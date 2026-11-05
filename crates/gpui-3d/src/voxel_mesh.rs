//! Raw padded voxel cells and reusable face templates for compute mesh generation.
use crate::{Aabb, Mesh, MeshError, MeshPart, Vec3, Vertex};
use std::{collections::BTreeMap, sync::OnceLock};
mod cells;
mod classes;
mod compact;
mod illumination;
mod instances;
mod lighting;
pub use instances::VoxelMeshInstance;
pub use lighting::voxel_ambient_light;

/// Occupied halo cell which occludes faces but does not emit geometry.
pub const VOXEL_OCCLUDER: u32 = u32::MAX;

/// One reusable face in a six-face voxel model. Vertices are relative to the cell origin.
#[derive(Clone, Debug, PartialEq)]
pub struct VoxelFaceTemplate {
    /// Relative positions, unorm colors and texture coordinates of the ordered quad.
    pub vertices: [Vertex; 4],
    /// Scene-node material slot used by this face.
    pub material_slot: u32,
    /// Unit axis direction of the neighboring cell which can occlude this face.
    pub neighbour: [i32; 3],
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn source() -> VoxelMeshSource {
        let cube = Mesh::cube();
        let templates = cube
            .vertices()
            .chunks_exact(4)
            .map(|vertices| VoxelFaceTemplate {
                vertices: std::array::from_fn(|index| {
                    let mut v = vertices[index];
                    v.position = v.position + Vec3::new(0.5, 0.5, 0.5);
                    v
                }),
                material_slot: 0,
                neighbour: [
                    vertices[0].normal.x as i32,
                    vertices[0].normal.y as i32,
                    vertices[0].normal.z as i32,
                ],
            })
            .collect();
        let mut cells = vec![0; 4 * 3 * 3];
        cells[17] = 1;
        cells[18] = 1;
        VoxelMeshSource::new([4, 3, 3], Vec3::new(-2.0, -1.0, -1.0), cells, templates).unwrap()
    }
    pub(crate) fn visible(source: &VoxelMeshSource) -> usize {
        source
            .fallback()
            .vertices()
            .chunks_exact(4)
            .filter(|quad| quad[0].color[3] > 0.0)
            .count()
    }
    #[test]
    fn neighbour_faces_and_vertical_bands_match_cube_semantics() {
        assert_eq!(visible(&source()), 10);
        assert_eq!(visible(&source().with_y_range(0.0, 0.5).unwrap()), 2);
        let mesh = Mesh::from_gpu_voxels(source());
        assert!(mesh.gpu_voxels().unwrap().fallback.get().is_none());
        assert_eq!(mesh.vertex_count(), 48);
        assert_eq!(mesh.vertices().len(), 48);
    }
    #[test]
    fn rejects_emitting_halo_and_invalid_cell_models() {
        let valid = source();
        let mut cells = valid.cells.to_vec();
        cells[0] = 1;
        assert!(
            VoxelMeshSource::new(valid.size, valid.origin, cells, valid.templates.to_vec())
                .is_err()
        );
        let mut cells = valid.cells.to_vec();
        cells[17] = 2;
        assert!(
            VoxelMeshSource::new(valid.size, valid.origin, cells, valid.templates.to_vec())
                .is_err()
        );
    }
    #[test]
    fn halo_hides_faces_and_static_faces_preserve_material_order() {
        let valid = source();
        let mut cells = valid.cells.to_vec();
        cells[16] = VOXEL_OCCLUDER;
        let source =
            VoxelMeshSource::new(valid.size, valid.origin, cells, valid.templates.to_vec())
                .unwrap();
        assert_eq!(visible(&source), 9);
        let mut first = source.templates[0].clone();
        first.material_slot = 3;
        first
            .vertices
            .iter_mut()
            .for_each(|v| v.color = [1.0, 0.0, 0.0, 0.5]);
        let mut second = first.clone();
        second
            .vertices
            .iter_mut()
            .for_each(|v| v.color = [0.0, 1.0, 0.0, 0.5]);
        let source = source.with_static_faces(vec![first, second]).unwrap();
        assert_eq!(visible(&source), 11);
        assert_eq!(source.parts.len(), 2);
        let vertices = source.fallback().vertices();
        let start = source.parts[1].first_index() as usize / 6 * 4;
        assert_eq!(vertices[start].color, [1.0, 0.0, 0.0, 128.0 / 255.0]);
        assert_eq!(vertices[start + 4].color, [0.0, 1.0, 0.0, 128.0 / 255.0]);
    }
}

/// Padded cell grid. Zero is air, `VOXEL_OCCLUDER` is a non-emitting solid,
/// and other values select a one-based model (six consecutive face templates).
/// The outermost cell layer must contain only air or non-emitting solids.
/// Dynamic faces must use opaque or masked materials: their parallel output order is unspecified.
/// Static faces preserve their order and may use blended materials.
#[derive(Debug)]
pub struct VoxelMeshSource {
    pub(crate) size: [u32; 3],
    pub(crate) origin: Vec3,
    pub(crate) cells: cells::Cells,
    pub(crate) templates: Box<[VoxelFaceTemplate]>,
    pub(crate) model_faces: usize,
    pub(crate) static_start: usize,
    pub(crate) instances: Option<instances::InstanceData>,
    pub(crate) parts: Box<[MeshPart]>,
    pub(crate) bounds: Aabb,
    pub(crate) clip_y: [f32; 2],
    pub(crate) ambient_occlusion: u32,
    pub(crate) lighting: Option<illumination::Lighting>,
    fallback: OnceLock<Mesh>,
}

/// Validate the immutable GPU input data without allocating or encoding a
/// temporary full CPU triangle Mesh. Both compute and lazy CPU fallback need
/// exactly the same finite-position/normal/UV and UNORM-color invariants.
fn validate_voxel_templates(faces: &[VoxelFaceTemplate]) -> Result<(), MeshError> {
    if faces.is_empty() {
        return Err(MeshError::InvalidParts);
    }
    let index_count = faces.len().checked_mul(6).ok_or(MeshError::TooLarge)?;
    u32::try_from(index_count).map_err(|_| MeshError::TooLarge)?;
    let vertices = faces.len().checked_mul(4).ok_or(MeshError::TooLarge)?;
    u32::try_from(vertices).map_err(|_| MeshError::TooLarge)?;
    for face in faces {
        for vertex in &face.vertices {
            if !vertex.position.is_finite()
                || !vertex.normal.is_finite()
                || !vertex.uv.x.is_finite()
                || !vertex.uv.y.is_finite()
                || vertex.color.iter().any(|value| !value.is_finite())
            {
                return Err(MeshError::NonFiniteVertex);
            }
            if vertex.color.iter().any(|value| !(0.0..=1.0).contains(value)) {
                return Err(MeshError::InvalidVoxelColor);
            }
        }
    }
    Ok(())
}

impl VoxelMeshSource {
    /// Validates a padded grid and computes conservative per-material output capacities.
    /// No faces or triangle vertices are generated here.
    ///
    /// # Errors
    /// Returns a mesh error for invalid cells, templates, coordinates or excessive capacity.
    pub fn new(
        size: [u32; 3],
        origin: Vec3,
        cells: Vec<u32>,
        templates: Vec<VoxelFaceTemplate>,
    ) -> Result<Self, MeshError> {
        let total = size
            .into_iter()
            .try_fold(1usize, |n, v| n.checked_mul(v as usize))
            .ok_or(MeshError::TooLarge)?;
        if total > i32::MAX as usize
            || size.iter().any(|&n| n < 3)
            || total != cells.len()
            || templates.len() % 6 != 0
            || ![origin.x, origin.y, origin.z].iter().all(|v| v.is_finite())
        {
            return Err(MeshError::InvalidParts);
        }
        let mut frequencies = vec![0usize; templates.len() / 6];
        for (index, &kind) in cells.iter().enumerate() {
            if kind == 0 || kind == VOXEL_OCCLUDER {
                continue;
            }
            let x = index as u32 % size[0];
            let y = index as u32 / size[0] % size[1];
            let z = index as u32 / size[0] / size[1];
            if x == 0
                || x + 1 == size[0]
                || y == 0
                || y + 1 == size[1]
                || z == 0
                || z + 1 == size[2]
            {
                return Err(MeshError::InvalidParts);
            }
            let Some(frequency) = frequencies.get_mut((kind - 1) as usize) else {
                return Err(MeshError::InvalidParts);
            };
            *frequency += 1;
        }
        let mut capacities = BTreeMap::<u32, usize>::new();
        for (index, face) in templates.iter().enumerate() {
            if ![
                [1, 0, 0],
                [-1, 0, 0],
                [0, 1, 0],
                [0, -1, 0],
                [0, 0, 1],
                [0, 0, -1],
            ]
            .contains(&face.neighbour)
            {
                return Err(MeshError::InvalidParts);
            }
            *capacities.entry(face.material_slot).or_default() += frequencies[index / 6];
        }
        // A Section can be model/instance-only with no six-face cube
        // templates. Validate non-empty template records directly; do not
        // construct a temporary CPU Mesh, index array and packed vertex table
        // just to verify GPU compute input.
        if !templates.is_empty() {
            validate_voxel_templates(&templates)?;
        }
        let mut parts = Vec::new();
        let mut first = 0u32;
        for (slot, count) in capacities {
            if count == 0 {
                continue;
            }
            let count = u32::try_from(count.checked_mul(6).ok_or(MeshError::TooLarge)?)
                .map_err(|_| MeshError::TooLarge)?;
            parts.push(MeshPart::new(first, count, slot));
            first = first
                .checked_add(count)
                .filter(|n| *n < 0x8000_0000)
                .ok_or(MeshError::TooLarge)?;
        }
        let mut min = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut max = -min;
        for vertex in templates.iter().flat_map(|face| face.vertices.iter()) {
            min = Vec3::new(
                min.x.min(vertex.position.x),
                min.y.min(vertex.position.y),
                min.z.min(vertex.position.z),
            );
            max = Vec3::new(
                max.x.max(vertex.position.x),
                max.y.max(vertex.position.y),
                max.z.max(vertex.position.z),
            );
        }
        if templates.is_empty() {
            min = Vec3::ZERO;
            max = Vec3::ZERO;
        }
        let bounds = Aabb::new(
            origin + min,
            origin
                + Vec3::new(
                    (size[0] - 1) as f32,
                    (size[1] - 1) as f32,
                    (size[2] - 1) as f32,
                )
                + max,
        )
        .ok_or(MeshError::NonFiniteVertex)?;
        let model_faces = templates.len();
        Ok(Self {
            size,
            origin,
            cells: cells::Cells::new(if model_faces == 0 { Vec::new() } else { cells }),
            templates: templates.into_boxed_slice(),
            model_faces,
            static_start: model_faces,
            instances: None,
            parts: parts.into_boxed_slice(),
            bounds,
            clip_y: [-f32::MAX, f32::MAX],
            ambient_occlusion: 0,
            lighting: None,
            fallback: OnceLock::new(),
        })
    }

    /// Adds CPU fallback faces (for example connected or fluid models) to the same draw regions.
    /// These faces are already in mesh-local coordinates and are not neighbor-culled again.
    /// # Errors
    /// Returns a mesh error for invalid vertices or excessive combined capacity.
    pub fn with_static_faces(mut self, faces: Vec<VoxelFaceTemplate>) -> Result<Self, MeshError> {
        if faces.is_empty() {
            return Ok(self);
        }
        validate_voxel_templates(&faces)?;
        let mut capacities = self
            .parts
            .iter()
            .map(|part| (part.material_slot(), part.index_count()))
            .collect::<BTreeMap<_, _>>();
        for face in &faces {
            let count = capacities.entry(face.material_slot).or_default();
            *count = count.checked_add(6).ok_or(MeshError::TooLarge)?;
            for vertex in face.vertices {
                let p = vertex.position;
                self.bounds.min = Vec3::new(
                    self.bounds.min.x.min(p.x),
                    self.bounds.min.y.min(p.y),
                    self.bounds.min.z.min(p.z),
                );
                self.bounds.max = Vec3::new(
                    self.bounds.max.x.max(p.x),
                    self.bounds.max.y.max(p.y),
                    self.bounds.max.z.max(p.z),
                );
            }
        }
        let mut first = 0u32;
        let mut parts = Vec::new();
        for (material, count) in capacities {
            parts.push(MeshPart::new(first, count, material));
            first = first
                .checked_add(count)
                .filter(|v| *v < 0x80000000)
                .ok_or(MeshError::TooLarge)?;
        }
        self.parts = parts.into_boxed_slice();
        let mut templates = self.templates.into_vec();
        templates.extend(faces);
        self.templates = templates.into_boxed_slice();
        self.fallback = OnceLock::new();
        Ok(self)
    }

    pub(crate) fn capacity(&self) -> usize {
        self.parts.last().map_or(0, |part| {
            (part.first_index() + part.index_count()) as usize / 6
        })
    }

    pub(crate) fn part_bounds(&self, material: u32) -> Aabb {
        if self.templates[..self.model_faces]
            .iter()
            .any(|face| face.material_slot == material)
        {
            return self.bounds;
        }
        let bounds = self
            .instances
            .as_ref()
            .and_then(|instances| instances.bounds.get(&material));
        let mut min = bounds.map_or(Vec3::new(f32::MAX, f32::MAX, f32::MAX), |b| b.min);
        let mut max = bounds.map_or(-min, |b| b.max);
        for vertex in self.templates[self.static_start..]
            .iter()
            .filter(|face| face.material_slot == material)
            .flat_map(|face| &face.vertices)
        {
            let p = vertex.position;
            min = Vec3::new(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z));
            max = Vec3::new(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z));
        }
        Aabb::new(min, max).unwrap_or(self.bounds)
    }
    /// Restricts emitted face centers to a half-open vertical band.
    /// # Errors
    /// Returns a mesh error for a non-finite or empty range.
    pub fn with_y_range(mut self, min: f32, max: f32) -> Result<Self, MeshError> {
        if !min.is_finite() || !max.is_finite() || min >= max {
            return Err(MeshError::InvalidParts);
        }
        self.clip_y = [min, max];
        self.fallback = OnceLock::new();
        Ok(self)
    }
    pub(crate) fn storage_bytes(&self) -> usize {
        self.cells.storage_bytes()
            + self.lighting.as_ref().map_or(0, |light| light.levels.len())
            + self.templates.len() * std::mem::size_of::<VoxelFaceTemplate>()
            + self.instances.as_ref().map_or(0, |instances| {
                instances.classes.storage_bytes()
                    + instances.jobs.len() * std::mem::size_of::<instances::InstanceJob>()
            })
            + self.fallback.get().map_or(0, Mesh::geometry_storage_bytes)
    }
    pub(crate) fn neighbour_delta(&self, face: &VoxelFaceTemplate) -> i32 {
        face.neighbour[0]
            + face.neighbour[1] * self.size[0] as i32
            + face.neighbour[2] * (self.size[0] * self.size[1]) as i32
    }

    pub(crate) fn fallback(&self) -> &Mesh {
        self.fallback.get_or_init(|| {
            let empty = [Vertex {
                position: Vec3::ZERO,
                normal: Vec3::Y,
                uv: crate::Vec2::ZERO,
                color: [0.0; 4],
            }; 4];
            let mut quads = vec![empty; self.capacity()];
            let mut counts = vec![0usize; self.parts.len()];
            self.visit_quads(|material, quad| {
                let part = self
                    .parts
                    .iter()
                    .position(|p| p.material_slot() == material)
                    .expect("validated face material has output capacity");
                let slot = self.parts[part].first_index() as usize / 6 + counts[part];
                counts[part] += 1;
                quads[slot] = quad;
            });
            Mesh::from_voxel_quads_without_raycast(quads, self.parts.to_vec())
                .expect("validated voxel templates and bounded material regions")
        })
    }
}

#[cfg(test)]
mod template_validation_tests {
    use super::*;

    #[test]
    fn validating_gpu_faces_does_not_materialize_cpu_geometry() {
        let source = tests::source();
        let templates = source.templates.to_vec();
        assert!(source.fallback.get().is_none());
        validate_voxel_templates(&templates).unwrap();
        assert!(source.fallback.get().is_none());
    }

    #[test]
    fn gpu_face_validator_preserves_compact_mesh_input_errors() {
        let original = tests::source().templates[0].clone();
        let mut broken = original.clone();
        broken.vertices[0].position.x = f32::NAN;
        assert!(matches!(
            validate_voxel_templates(&[broken]), Err(MeshError::NonFiniteVertex)
        ));
        let mut broken = original.clone();
        broken.vertices[0].uv.y = f32::INFINITY;
        assert!(matches!(
            validate_voxel_templates(&[broken]), Err(MeshError::NonFiniteVertex)
        ));
        let mut broken = original.clone();
        broken.vertices[0].color[3] = 1.2;
        assert!(matches!(
            validate_voxel_templates(&[broken]), Err(MeshError::InvalidVoxelColor)
        ));
        assert!(matches!(
            validate_voxel_templates(&[]), Err(MeshError::InvalidParts)
        ));
        validate_voxel_templates(&[original]).unwrap();
    }
    #[test]
    fn empty_cube_template_table_is_valid_for_gpu_instance_only_source() {
        // Instance-only source Section data does not require standard cube
        // templates. GPU model faces may be added by with_instances later.
        let size = [3, 3, 3];
        let cells = vec![0u32; (size[0] * size[1] * size[2]) as usize];
        let created = VoxelMeshSource::new(size, Vec3::ZERO, cells, Vec::new());
        assert!(created.is_ok());
        let source = created.unwrap();
        assert_eq!(source.templates.len(), 0);
        assert!(source.fallback.get().is_none());
    }

}
