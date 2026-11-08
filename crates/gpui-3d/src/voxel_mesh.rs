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
        // Adjacent solid faces are not reserved in the GPU output. 2 cubes
        // share one internal plane: 12 theoretical faces -> 10 actual faces.
        assert_eq!(mesh.vertex_count(), 40);
        assert_eq!(mesh.vertices().len(), 40);
    }

    #[test]
    fn dense_voxel_sections_reserve_only_surface_output_capacity() {
        // A padded 4x4x4 solid cube emits 6*4*4 exterior quads, not the old
        // 6*4*4*4 worst-case inner+hidden face allocation.
        let size = [6, 6, 6];
        let mut cells = vec![0u32; 6 * 6 * 6];
        for z in 1..5 {
            for y in 1..5 {
                for x in 1..5 {
                    cells[(z * 6 + y) * 6 + x] = 1;
                }
            }
        }
        let templates = source().templates.to_vec();
        let packed = VoxelMeshSource::new(size, Vec3::ZERO, cells, templates).unwrap();
        assert_eq!(packed.capacity(), 96);
        assert_eq!(visible(&packed), 96);
        assert!(packed.part_bounds(0).max.x.is_finite());
    }

    #[test]
    fn malformed_neighbour_offsets_are_rejected_without_overflow() {
        let mut templates = source().templates.to_vec();
        templates[0].neighbour = [i32::MIN, i32::MIN, i32::MIN];
        let result = VoxelMeshSource::new(
            [4, 3, 3], Vec3::ZERO,
            {
                let mut cells = vec![0; 4 * 3 * 3];
                cells[17] = 1;
                cells
            },
            templates,
        );
        assert!(matches!(result, Err(MeshError::InvalidParts)));
    }

    #[test]
    fn fully_occluded_single_voxel_reserves_no_gpu_faces() {
        let size = [5, 5, 5];
        let mut cells = vec![0u32; 125];
        let center = (2 * 5 + 2) * 5 + 2;
        cells[center] = 1;
        // Six halo occluders suppress all faces without emitting geometry.
        for neighbor in [center - 1, center + 1, center - 5, center + 5, center - 25, center + 25] {
            cells[neighbor] = VOXEL_OCCLUDER;
        }
        let packed = VoxelMeshSource::new(
            size, Vec3::ZERO, cells, source().templates.to_vec()
        ).unwrap();
        assert_eq!(packed.capacity(), 0);
        assert!(packed.parts.is_empty());
        assert_eq!(visible(&packed), 0);
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

/// One retained GPU-generated Section to be fused with neighbouring Sections.
///
/// `offset` is measured in padded-grid cells relative to the first Section,
/// not world-space coordinates. `material_slots` maps local material slots to
/// the fused node's material array.
pub struct VoxelFusionInput<'a> {
    /// An immutable GPU voxel source; its original Scene node is not mutated.
    pub source: &'a VoxelMeshSource,
    /// Source-grid displacement within the fused padded grid.
    pub offset: [u32; 3],
    /// Source Scene-node translation relative to the fused Scene node, in
    /// whole world/mesh units. Static face vertices use this displacement;
    /// cell-relative cube/model templates use `offset` instead.
    pub translation: [i32; 3],
    /// Local material slot -> fused material slot mapping.
    pub material_slots: &'a [u32],
}

impl VoxelMeshSource {
    /// Fuse adjacent GPU-generated Sections before GPU meshing. Rebase cube
    /// classes, arbitrary-model jobs, static quads and light fields without
    /// expanding any source into CPU vertices. The resulting mesh uses one
    /// material part per distinct fused material, instead of one per Section.
    ///
    /// Adjacent halo overlaps are resolved by emitting interior cells first;
    /// a source's halo must never replace an adjacent source's real cell.
    ///
    /// The caller must restrict fusion to opaque/masked materials and keep the
    /// original Section nodes for occluders, editing and streaming identity.
    pub fn fuse_sections(inputs: &[VoxelFusionInput<'_>]) -> Result<Self, MeshError> {
        let Some(first) = inputs.first() else { return Err(MeshError::InvalidParts) };
        if inputs.len() < 2 { return Err(MeshError::InvalidParts); }
        let base = first.source;
        let base_origin = Vec3::new(
            base.origin.x + first.translation[0] as f32 - first.offset[0] as f32,
            base.origin.y + first.translation[1] as f32 - first.offset[1] as f32,
            base.origin.z + first.translation[2] as f32 - first.offset[2] as f32,
        );
        let mut size = [0u32; 3];
        let mut total_classes = 0u32;
        let mut total_instance_templates = 0u32;
        let mut template_count = 0usize;
        for input in inputs {
            let source = input.source;
            let expected_origin = Vec3::new(
                base_origin.x + input.offset[0] as f32,
                base_origin.y + input.offset[1] as f32,
                base_origin.z + input.offset[2] as f32,
            );
            let placed_origin = Vec3::new(
                source.origin.x + input.translation[0] as f32,
                source.origin.y + input.translation[1] as f32,
                source.origin.z + input.translation[2] as f32,
            );
            if (placed_origin - expected_origin).length() > 0.001
                || source.clip_y != base.clip_y
                || source.ambient_occlusion != base.ambient_occlusion
                || source.lighting.as_ref().map(|light| (light.ambient.to_bits(), light.levels.len()))
                    .map(|(ambient, _)| ambient)
                    != base.lighting.as_ref().map(|light| light.ambient.to_bits())
                || source.model_faces % 6 != 0
            {
                return Err(MeshError::InvalidParts);
            }
            for axis in 0..3 {
                size[axis] = size[axis].max(
                    input.offset[axis].checked_add(source.size[axis]).ok_or(MeshError::TooLarge)?
                );
            }
            total_classes = total_classes.checked_add(
                u32::try_from(source.model_faces / 6).map_err(|_| MeshError::TooLarge)?
            ).ok_or(MeshError::TooLarge)?;
            total_instance_templates = total_instance_templates.checked_add(
                u32::try_from(source.static_start - source.model_faces)
                    .map_err(|_| MeshError::TooLarge)?
            ).ok_or(MeshError::TooLarge)?;
            template_count = template_count.checked_add(source.templates.len())
                .ok_or(MeshError::TooLarge)?;
        }
        let count = size.into_iter()
            .try_fold(1usize, |acc, dim| acc.checked_mul(dim as usize))
            .ok_or(MeshError::TooLarge)?;
        if count > i32::MAX as usize { return Err(MeshError::TooLarge); }

        // Mark authoritative emitting volumes before importing any halos.
        // Otherwise a later neighbour's occluder halo could accidentally
        // replace an earlier Section's air cell on a shared seam.
        let mut interior_mask = vec![false; count];
        for input in inputs {
            let src = input.source.size;
            let dst_xy = size[0] as usize * size[1] as usize;
            for z in 1..src[2] - 1 {
                for y in 1..src[1] - 1 {
                    for x in 1..src[0] - 1 {
                        let target = (z + input.offset[2]) as usize * dst_xy
                            + (y + input.offset[1]) as usize * size[0] as usize
                            + (x + input.offset[0]) as usize;
                        if interior_mask[target] {
                            return Err(MeshError::InvalidParts);
                        }
                        interior_mask[target] = true;
                    }
                }
            }
        }
        let mut cells = vec![0u32; count];
        // The common cube-only GPU Section has no model-instance neighbour
        // classes; allocating an additional full u32 grid would be wasted.
        let mut classes = inputs.iter()
            .any(|input| input.source.instances.is_some())
            .then(|| vec![0u32; count]);
        let mut templates = Vec::with_capacity(total_classes as usize * 6);
        let mut model_faces = Vec::with_capacity(total_instance_templates as usize);
        let mut instances = Vec::new();
        let mut statics = Vec::with_capacity(template_count);
        let mut levels = base.lighting.as_ref().map(|_| vec![0u8; count]);
        let mut class_offset = 0u32;
        let mut instance_face_offset = 0u32;
        for input in inputs {
            let source = input.source;
            let remap = |slot: u32| -> Result<u32, MeshError> {
                input.material_slots.get(slot as usize).copied().ok_or(MeshError::InvalidParts)
            };
            let models = &source.templates[..source.model_faces];
            for face in models {
                let mut face = face.clone();
                face.material_slot = remap(face.material_slot)?;
                templates.push(face);
            }
            let src_instances = &source.templates[source.model_faces..source.static_start];
            for face in src_instances {
                let mut face = face.clone();
                face.material_slot = remap(face.material_slot)?;
                model_faces.push(face);
            }
            let delta = Vec3::new(
                input.translation[0] as f32,
                input.translation[1] as f32,
                input.translation[2] as f32,
            );
            for face in &source.templates[source.static_start..] {
                let mut face = face.clone();
                face.material_slot = remap(face.material_slot)?;
                for vertex in &mut face.vertices {
                    vertex.position = vertex.position + delta;
                }
                statics.push(face);
            }
            if let Some(data) = &source.instances {
                for job in data.jobs.iter() {
                    let first = instance_face_offset.checked_add(
                        job.first_face.checked_sub(source.model_faces as u32).ok_or(MeshError::InvalidParts)?
                    ).ok_or(MeshError::TooLarge)?;
                    let end = first.checked_add(job.face_count).ok_or(MeshError::TooLarge)?;
                    instances.push(VoxelMeshInstance {
                        position: std::array::from_fn(|axis| job.position[axis] + input.offset[axis]),
                        faces: first..end,
                        height: job.height,
                        occlusion_mask: job.occlusion_mask,
                    });
                }
            }
            let source_xy = (source.size[0] as usize) * (source.size[1] as usize);
            let merged_xy = (size[0] as usize) * (size[1] as usize);
            // The ownership mask is already complete for all source
            // interiors. One linear pass per Section is sufficient: halos
            // can never override authoritative voxels, even if those are air.
            // This halves the hot path's source-grid traversal.
            let source_volume = source.size.into_iter()
                .map(|extent| extent as usize).product::<usize>();
            for index in 0..source_volume {
                // Model-only Sections omit their cube-grid storage but may
                // still have neighbour classes and immutable light levels.
                let source_kind = if source.cells.len() == 0 {
                    0
                } else {
                    source.cells.cell(index)
                };
                let x = index % source.size[0] as usize;
                let y = index / source.size[0] as usize % source.size[1] as usize;
                let z = index / source_xy;
                let interior = x > 0 && y > 0 && z > 0
                    && x + 1 < source.size[0] as usize
                    && y + 1 < source.size[1] as usize
                    && z + 1 < source.size[2] as usize;
                let gx = x + input.offset[0] as usize;
                let gy = y + input.offset[1] as usize;
                let gz = z + input.offset[2] as usize;
                let destination = gz * merged_xy + gy * size[0] as usize + gx;
                if !interior && interior_mask[destination] { continue }
                let kind = if source_kind == 0 || source_kind == VOXEL_OCCLUDER {
                    source_kind
                } else {
                    class_offset.checked_add(source_kind).ok_or(MeshError::TooLarge)?
                };
                if interior || cells[destination] == 0 { cells[destination] = kind; }
                if let (Some(data), Some(classes)) = (&source.instances, &mut classes) {
                    let value = data.classes.cell(index);
                    if interior || classes[destination] == 0 { classes[destination] = value; }
                }
                if let (Some(light), Some(levels)) = (&source.lighting, &mut levels) {
                    if interior || levels[destination] == 0 {
                        levels[destination] = light.levels[index];
                    }
                }
            }
            class_offset = class_offset.checked_add((source.model_faces / 6) as u32)
                .ok_or(MeshError::TooLarge)?;
            instance_face_offset = instance_face_offset.checked_add(src_instances.len() as u32)
                .ok_or(MeshError::TooLarge)?;
        }
        let mut combined = Self::new(size, base_origin, cells, templates)?;
        if !instances.is_empty() {
            combined = combined.with_instances(
                classes.ok_or(MeshError::InvalidParts)?, model_faces, instances,
            )?;
        }
        combined = combined.with_static_faces(statics)?;
        combined = combined.with_ambient_occlusion(base.ambient_occlusion);
        if let (Some(light), Some(levels)) = (&base.lighting, levels) {
            combined = combined.with_lighting(levels, light.ambient)?;
        }
        combined.with_y_range(base.clip_y[0], base.clip_y[1])
    }
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
    /// Dimensions of this Section's padded voxel grid.
    #[must_use]
    pub const fn size(&self) -> [u32; 3] {
        self.size
    }

    /// Origin of this padded grid in its owning Scene-node coordinate system.
    #[must_use]
    pub const fn origin(&self) -> Vec3 {
        self.origin
    }

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
        // CPU only bounds GPU storage; it does not generate faces or issue a
        // visibility decision. The old estimate reserved all six faces of
        // every occupied voxel, even if its neighbours fully hid them. On
        // solid terrain this could reserve 4-6x the required GPU output.
        // Count the same conservative neighbour-empty condition as cs_mesh
        // and reserve only those possible faces, per material.
        if !templates.is_empty() {
            validate_voxel_templates(&templates)?;
        }
        let mut capacities = BTreeMap::<u32, usize>::new();
        let sx = size[0] as usize;
        let sy = size[1] as usize;
        let xy = sx.checked_mul(sy).ok_or(MeshError::TooLarge)?;
        for (index, &kind) in cells.iter().enumerate() {
            if kind == 0 || kind == VOXEL_OCCLUDER {
                continue;
            }
            let x = index % sx;
            let y = index / sx % sy;
            let z = index / xy;
            if x == 0 || x + 1 == sx
                || y == 0 || y + 1 == sy
                || z == 0 || z + 1 == size[2] as usize
            {
                return Err(MeshError::InvalidParts);
            }
            let offset = (kind as usize - 1)
                .checked_mul(6)
                .ok_or(MeshError::TooLarge)?;
            let faces = templates.get(offset..offset + 6)
                .ok_or(MeshError::InvalidParts)?;
            for face in faces {
                let [dx, dy, dz] = face.neighbour;
                // Check canonical offsets explicitly: unsigned_abs sums may
                // overflow for malformed i32::MIN neighbours in debug builds.
                if !matches!(face.neighbour,
                    [1, 0, 0] | [-1, 0, 0] | [0, 1, 0] |
                    [0, -1, 0] | [0, 0, 1] | [0, 0, -1]
                ) {
                    return Err(MeshError::InvalidParts);
                }
                let nx = (x as i32 + dx) as usize;
                let ny = (y as i32 + dy) as usize;
                let nz = (z as i32 + dz) as usize;
                let neighbour = (nz * sy + ny) * sx + nx;
                if cells[neighbour] == 0 {
                    let capacity = capacities.entry(face.material_slot).or_default();
                    *capacity = capacity.checked_add(1).ok_or(MeshError::TooLarge)?;
                }
            }
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

    /// Stable content fingerprint of the GPU voxel plan.
    ///
    /// Two sources with equal fingerprints produce equal fused plans for equal
    /// placements, which lets callers reuse cached fusion results instead of
    /// re-running [`Self::fuse_sections`]. Derived CPU geometry (`parts`,
    /// `bounds`, the lazily built CPU fallback) is excluded; inputs that change
    /// GPU output — cells, classes, instance jobs, templates, clip band,
    /// ambient occlusion and the light field — are covered.
    #[must_use]
    pub fn content_fingerprint(&self) -> u64 {
        use crate::fingerprint::FingerprintHasher;
        let mut hasher = FingerprintHasher::new();
        for value in self.size {
            hasher.write_u32(value);
        }
        for value in [self.origin.x, self.origin.y, self.origin.z] {
            hasher.write_u32(value.to_bits());
        }
        match &self.cells {
            cells::Cells::Bytes(values) => {
                hasher.write_u32(1);
                hasher.write_bytes(values.as_ref());
            }
            cells::Cells::Words(values) => {
                hasher.write_u32(2);
                for value in values.iter() {
                    hasher.write_u32(*value);
                }
            }
        }
        hasher.write_usize(self.templates.len());
        for template in &self.templates {
            hasher.write_u32(template.material_slot);
            for value in template.neighbour {
                hasher.write_u32(value as u32);
            }
            for vertex in &template.vertices {
                for value in [vertex.position.x, vertex.position.y, vertex.position.z] {
                    hasher.write_u32(value.to_bits());
                }
                for value in [vertex.normal.x, vertex.normal.y, vertex.normal.z] {
                    hasher.write_u32(value.to_bits());
                }
                for value in [vertex.uv.x, vertex.uv.y] {
                    hasher.write_u32(value.to_bits());
                }
                for value in vertex.color {
                    hasher.write_u32(value.to_bits());
                }
            }
        }
        hasher.write_usize(self.model_faces);
        hasher.write_usize(self.static_start);
        for value in self.clip_y {
            hasher.write_u32(value.to_bits());
        }
        hasher.write_u32(self.ambient_occlusion);
        match &self.instances {
            Some(instances) => {
                hasher.write_u32(1);
                hasher.write_usize(instances.jobs.len());
                for job in &instances.jobs {
                    for value in job.position {
                        hasher.write_u32(value);
                    }
                    hasher.write_u32(job.height.to_bits());
                    hasher.write_u32(job.first_face);
                    hasher.write_u32(job.face_count);
                    hasher.write_u32(job.material);
                    hasher.write_u32(job.occlusion_mask);
                }
            }
            None => hasher.write_u32(0),
        }
        match &self.lighting {
            Some(lighting) => {
                hasher.write_u32(1);
                hasher.write_u32(lighting.ambient.to_bits());
                hasher.write_bytes(lighting.levels.as_ref());
            }
            None => hasher.write_u32(0),
        }
        hasher.finish()
    }
}

#[cfg(test)]
mod fingerprint_tests {
    use super::tests::source;
    use super::*;

    #[test]
    fn equal_content_shares_fingerprint_and_edits_change_it() {
        let first = source();
        let second = source();
        assert_eq!(first.content_fingerprint(), second.content_fingerprint());
        let mut cells = first.cells.to_vec();
        cells[17] = 0;
        let changed = VoxelMeshSource::new(first.size, first.origin, cells, first.templates.to_vec())
            .unwrap();
        assert_ne!(first.content_fingerprint(), changed.content_fingerprint());
    }
}

#[cfg(test)]
mod fusion_tests {
    use super::*;

    #[test]
    fn adjacent_generated_cube_sections_share_one_material_part_and_cull_seams() {
        let left = tests::source();
        let right = tests::source();
        let merged = VoxelMeshSource::fuse_sections(&[
            VoxelFusionInput { source: &left, offset: [0, 0, 0], translation: [0, 0, 0], material_slots: &[0] },
            VoxelFusionInput { source: &right, offset: [2, 0, 0], translation: [2, 0, 0], material_slots: &[0] },
        ]).unwrap();
        assert_eq!(merged.size(), [6, 3, 3]);
        assert_eq!(merged.parts.len(), 1, "same material across Sections becomes one draw");
        assert_eq!(tests::visible(&left), 10);
        assert_eq!(tests::visible(&right), 10);
        assert_eq!(tests::visible(&merged), 18, "inter-Section hidden faces must disappear");
    }

    #[test]
    fn fused_sections_match_separate_cpu_geometry_when_not_touching() {
        // A differential oracle independent of the GPU compute implementation:
        // before/after fusion must preserve every quad's world coordinates.
        // Leave an empty grid cell between the two sources so no face should
        // be culled by the new inter-Section neighbour check.
        let left = tests::source();
        let right = tests::source();
        let merged = VoxelMeshSource::fuse_sections(&[
            VoxelFusionInput {
                source: &left, offset: [0, 0, 0],
                translation: [0, 0, 0], material_slots: &[0],
            },
            VoxelFusionInput {
                source: &right, offset: [5, 0, 0],
                translation: [5, 0, 0], material_slots: &[0],
            },
        ]).unwrap();
        let quads = |source: &VoxelMeshSource, x: f32| {
            let mut faces = source.fallback().vertices()
                .chunks_exact(4)
                .filter(|quad| quad[0].color[3] > 0.0)
                .map(|quad| std::array::from_fn::<_, 4, _>(|corner| {
                    let pos = quad[corner].position;
                    [(pos.x + x).to_bits(), pos.y.to_bits(), pos.z.to_bits()]
                }))
                .collect::<Vec<[[u32; 3]; 4]>>();
            faces.sort_unstable();
            faces
        };
        let mut separate = quads(&left, 0.0);
        separate.extend(quads(&right, 5.0));
        separate.sort_unstable();
        assert_eq!(quads(&merged, 0.0), separate,
            "cross-Section fusion must never change unrelated geometry");
    }

    #[test]
    fn fused_model_only_sections_keep_instance_jobs_and_neighbor_classes() {
        let face_templates = tests::source().templates.to_vec();
        let make = || VoxelMeshSource::new([4, 3, 3], Vec3::ZERO, vec![0; 36], vec![])
            .unwrap().with_instances(
                vec![16; 36], face_templates.clone(),
                vec![VoxelMeshInstance {
                    position: [1, 1, 1], faces: 0..6, height: 1.0,
                    occlusion_mask: 0,
                }]
            ).unwrap();
        let left = make();
        let right = make();
        assert_eq!(left.cells.len(), 0, "model-only sources skip the cube cell buffer");
        let merged = VoxelMeshSource::fuse_sections(&[
            VoxelFusionInput { source: &left, offset: [0, 0, 0], translation: [0, 0, 0], material_slots: &[0] },
            VoxelFusionInput { source: &right, offset: [2, 0, 0], translation: [2, 0, 0], material_slots: &[0] },
        ]).unwrap();
        assert_eq!(merged.parts.len(), 1);
        assert_eq!(merged.instances.as_ref().unwrap().jobs.len(), 2);
        assert_eq!(tests::visible(&merged), 12);
        let classes = &merged.instances.as_ref().unwrap().classes;
        assert_eq!(classes.cell(1 + 6 + 6 * 3), 16);
        assert_eq!(classes.cell(3 + 6 + 6 * 3), 16);
    }

    #[test]
    fn fused_static_sections_rebase_quads_and_preserve_lighting() {
        let face = tests::source().templates[0].clone();
        let make = || VoxelMeshSource::new([4, 3, 3], Vec3::ZERO, vec![0; 36], vec![])
            .unwrap()
            .with_static_faces(vec![face.clone()]).unwrap()
            .with_lighting(vec![7; 36], 0.25).unwrap();
        let left = make();
        let right = make();
        let merged = VoxelMeshSource::fuse_sections(&[
            VoxelFusionInput { source: &left, offset: [0, 0, 0], translation: [0, 0, 0], material_slots: &[0] },
            VoxelFusionInput { source: &right, offset: [2, 0, 0], translation: [2, 0, 0], material_slots: &[0] },
        ]).unwrap();
        assert_eq!(merged.parts.len(), 1);
        assert_eq!(merged.capacity(), 2);
        assert_eq!(merged.lighting.as_ref().unwrap().levels.len(), 54);
        assert!(merged.lighting.as_ref().unwrap().levels.iter().all(|&x| x == 7));
        assert_eq!(merged.fallback().vertices().len(), 8);
        let mut x = merged.fallback().vertices().iter().map(|v| v.position.x).collect::<Vec<_>>();
        x.sort_by(f32::total_cmp);
        assert!(x[7] - x[0] > 2.0, "right static faces must move by two grid cells");
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
