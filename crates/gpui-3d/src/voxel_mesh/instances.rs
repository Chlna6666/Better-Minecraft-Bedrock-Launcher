//! Stable per-material jobs for reusable arbitrary models and height-scaled fluids.
use super::*;
use std::{collections::HashMap, ops::Range};
fn union(a: Aabb, b: Aabb) -> Aabb {
    Aabb {
        min: a.min.min(b.min),
        max: a.max.max(b.max),
    }
}

/// One instance of reusable face templates inside the padded cell grid.
#[derive(Clone, Debug, PartialEq)]
pub struct VoxelMeshInstance {
    /// Cell coordinates inside the grid, including its padding.
    pub position: [u32; 3],
    /// Face template range passed to `VoxelMeshSource::with_instances`.
    pub faces: Range<u32>,
    /// Vertical scale applied before translation; fluid surfaces use their decoded height.
    pub height: f32,
    /// Neighbor-class bits which hide this instance's faces. Zero disables neighbor culling.
    pub occlusion_mask: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_instances_cull_neighbors_and_scale_fluid_vertices() {
        let source = super::super::tests::source();
        let templates = source.templates.to_vec();
        let classes = source.cells.iter().map(|v| u32::from(v != 0)).collect();
        let source = source
            .with_instances(
                classes,
                templates,
                vec![VoxelMeshInstance {
                    position: [1, 1, 1],
                    faces: 0..6,
                    height: 0.4,
                    occlusion_mask: 1,
                }],
            )
            .unwrap();
        assert_eq!(super::super::tests::visible(&source), 15);
        let source = source.with_y_range(0.0, 0.5).unwrap();
        assert_eq!(super::super::tests::visible(&source), 7);
        assert!(
            source
                .fallback()
                .vertices()
                .iter()
                .any(|v| v.position.y == 0.4)
        );
    }
    #[test]
    fn enclosed_instances_never_reserve_gpu_output_or_compute_jobs() {
        let templates = super::super::tests::source().templates.to_vec();
        let source = VoxelMeshSource::new([3, 3, 3], Vec3::ZERO, vec![0; 27], vec![])
            .unwrap();
        let mut classes = vec![0u32; 27];
        for neighbor in [12, 14, 10, 16, 4, 22] {
            classes[neighbor] = 1;
        }
        let source = source
            .with_instances(
                classes,
                templates,
                vec![VoxelMeshInstance {
                    position: [1, 1, 1],
                    faces: 0..6,
                    height: 1.0,
                    occlusion_mask: 1,
                }],
            )
            .unwrap();
        assert_eq!(source.capacity(), 0);
        assert!(source.instances.as_ref().unwrap().jobs.is_empty());
        assert_eq!(super::super::tests::visible(&source), 0);
    }

    #[test]
    fn partial_occlusion_reserves_only_exposed_quads() {
        let templates = super::super::tests::source().templates.to_vec();
        let source = VoxelMeshSource::new([3, 3, 3], Vec3::ZERO, vec![0; 27], vec![])
            .unwrap();
        let mut classes = vec![0u32; 27];
        classes[12] = 1; // One adjacent opaque cell suppresses exactly one face.
        let source = source
            .with_instances(
                classes,
                templates,
                vec![VoxelMeshInstance {
                    position: [1, 1, 1],
                    faces: 0..6,
                    height: 1.0,
                    occlusion_mask: 1,
                }],
            )
            .unwrap();
        assert_eq!(source.capacity(), 5);
        assert_eq!(source.instances.as_ref().unwrap().jobs.len(), 1);
        assert_eq!(super::super::tests::visible(&source), 5);
    }
    #[test]
    fn models_can_fill_an_otherwise_empty_cell_source() {
        let templates = super::super::tests::source().templates.to_vec();
        let source = VoxelMeshSource::new([3, 3, 3], Vec3::ZERO, vec![0; 27], vec![]).unwrap();
        let source = source
            .with_instances(
                vec![0; 27],
                templates,
                vec![VoxelMeshInstance {
                    position: [1, 1, 1],
                    faces: 0..6,
                    height: 1.0,
                    occlusion_mask: 1,
                }],
            )
            .unwrap();
        assert_eq!(super::super::tests::visible(&source), 6);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InstanceJob {
    pub position: [u32; 3],
    pub height: f32,
    pub first_face: u32,
    pub face_count: u32,
    pub material: u32,
    pub occlusion_mask: u32,
}

#[derive(Debug, PartialEq)]
pub(crate) struct InstanceData {
    pub classes: super::classes::Classes,
    pub jobs: Box<[InstanceJob]>,
    pub bounds: BTreeMap<u32, Aabb>,
}

impl VoxelMeshSource {
    /// Adds model instances without expanding their faces on the CPU.
    /// Templates are relative to the cell origin. Their `neighbour` stores a relative cell
    /// offset; zero disables occlusion for that face. Class bits have application-defined meaning.
    /// Instance order is retained within each material, including blended materials.
    /// # Errors
    /// Rejects invalid ranges, colors, scales, halo accesses, repeated calls or output overflow.
    pub fn with_instances(
        mut self,
        classes: Vec<u32>,
        faces: Vec<VoxelFaceTemplate>,
        instances: Vec<VoxelMeshInstance>,
    ) -> Result<Self, MeshError> {
        if self.instances.is_some()
            || classes.len() != self.size.into_iter().map(|n| n as usize).product::<usize>()
        {
            return Err(MeshError::InvalidParts);
        }
        if instances.is_empty() {
            return Ok(self);
        }
        validate_voxel_templates(&faces)?;
        let mut capacities = self
            .parts
            .iter()
            .map(|part| (part.material_slot(), part.index_count()))
            .collect::<BTreeMap<_, _>>();
        let mut models =
            HashMap::<(u32, u32), Vec<(u32, u32, u32, Aabb, [i32; 3], [i32; 3])>>::new();
        let mut templates = Vec::new();
        let mut jobs = BTreeMap::<u32, Vec<InstanceJob>>::new();
        let mut bounds = BTreeMap::<u32, Aabb>::new();
        for instance in instances {
            if !instance.height.is_finite()
                || instance.height <= 0.0
                || instance.height > 1.0
                || instance.faces.start >= instance.faces.end
                || instance.faces.end as usize > faces.len()
                || instance
                    .position
                    .iter()
                    .zip(self.size)
                    .any(|(&v, n)| v >= n)
            {
                return Err(MeshError::InvalidParts);
            }
            let key = (instance.faces.start, instance.faces.end);
            if !models.contains_key(&key) {
                let mut groups = BTreeMap::<u32, Vec<&VoxelFaceTemplate>>::new();
                for face in &faces[instance.faces.start as usize..instance.faces.end as usize] {
                    groups.entry(face.material_slot).or_default().push(face);
                }
                let mut model = Vec::new();
                for (material, group) in groups {
                    let first = u32::try_from(self.model_faces + templates.len())
                        .map_err(|_| MeshError::TooLarge)?;
                    let mut min = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
                    let mut max = -min;
                    let mut offset_min = [0; 3];
                    let mut offset_max = [0; 3];
                    for face in &group {
                        for axis in 0..3 {
                            offset_min[axis] = offset_min[axis].min(face.neighbour[axis]);
                            offset_max[axis] = offset_max[axis].max(face.neighbour[axis]);
                        }
                        for v in face.vertices {
                            min = Vec3::new(
                                min.x.min(v.position.x),
                                min.y.min(v.position.y),
                                min.z.min(v.position.z),
                            );
                            max = Vec3::new(
                                max.x.max(v.position.x),
                                max.y.max(v.position.y),
                                max.z.max(v.position.z),
                            );
                        }
                        templates.push((*face).clone());
                    }
                    model.push((
                        material,
                        first,
                        group.len() as u32,
                        Aabb::new(min, max).ok_or(MeshError::NonFiniteVertex)?,
                        offset_min,
                        offset_max,
                    ));
                }
                models.insert(key, model);
            }
            for &(material, first, count, model_bounds, offset_min, offset_max) in &models[&key] {
                for axis in 0..3 {
                    let value = i64::from(instance.position[axis]);
                    if value + i64::from(offset_min[axis]) < 0
                        || value + i64::from(offset_max[axis]) >= i64::from(self.size[axis])
                    {
                        return Err(MeshError::InvalidParts);
                    }
                }
                // The padded class grid already contains every adjacent Section's
                // occlusion halo. Count exposed template faces before reserving
                // GPU output: hidden faces never need a DrawIndirect slot, and
                // completely enclosed instances do not need a compute job.
                // Only integer grid lookups are performed; no CPU vertices are
                // expanded or copied into an intermediate mesh.
                let cell = (i64::from(instance.position[2]) * i64::from(self.size[1])
                    + i64::from(instance.position[1]))
                    * i64::from(self.size[0])
                    + i64::from(instance.position[0]);
                let begin = first as usize - self.model_faces;
                let exposed = templates[begin..begin + count as usize]
                    .iter()
                    .filter(|face| {
                        let [dx, dy, dz] = face.neighbour;
                        if [dx, dy, dz] == [0, 0, 0] {
                            return true;
                        }
                        let delta = i64::from(dx)
                            + i64::from(dy) * i64::from(self.size[0])
                            + i64::from(dz)
                                * i64::from(self.size[0])
                                * i64::from(self.size[1]);
                        classes[(cell + delta) as usize] & instance.occlusion_mask == 0
                    })
                    .count();
                if exposed == 0 {
                    continue;
                }
                let exposed_indices = u32::try_from(exposed)
                    .ok()
                    .and_then(|faces| faces.checked_mul(6))
                    .ok_or(MeshError::TooLarge)?;
                let capacity = capacities.entry(material).or_default();
                *capacity = capacity
                    .checked_add(exposed_indices)
                    .ok_or(MeshError::TooLarge)?;
                let position = self.origin
                    + Vec3::new(
                        instance.position[0] as f32,
                        instance.position[1] as f32,
                        instance.position[2] as f32,
                    );
                let scale = |p: Vec3| position + Vec3::new(p.x, p.y * instance.height, p.z);
                let instance_bounds = Aabb::new(scale(model_bounds.min), scale(model_bounds.max))
                    .ok_or(MeshError::NonFiniteVertex)?;
                bounds
                    .entry(material)
                    .and_modify(|b| *b = union(*b, instance_bounds))
                    .or_insert(instance_bounds);
                self.bounds = union(self.bounds, instance_bounds);
                jobs.entry(material).or_default().push(InstanceJob {
                    position: instance.position,
                    height: instance.height,
                    first_face: first,
                    face_count: count,
                    material,
                    occlusion_mask: instance.occlusion_mask,
                });
            }
        }
        let mut first = 0u32;
        let mut parts = Vec::new();
        for (material, count) in capacities {
            parts.push(MeshPart::new(first, count, material));
            first = first
                .checked_add(count)
                .filter(|n| *n < 0x80000000)
                .ok_or(MeshError::TooLarge)?;
        }
        let mut all = self.templates.into_vec();
        let old_static = all.split_off(self.model_faces);
        all.extend(templates);
        self.static_start = all.len();
        all.extend(old_static);
        self.templates = all.into_boxed_slice();
        self.parts = parts.into_boxed_slice();
        self.instances = Some(InstanceData {
            classes: super::classes::Classes::new(classes),
            jobs: jobs.into_values().flatten().collect(),
            bounds,
        });
        self.fallback = OnceLock::new();
        Ok(self)
    }

    pub(super) fn visit_instance_quads(&self, mut emit: impl FnMut(u32, [Vertex; 4])) {
        let Some(instances) = &self.instances else {
            return;
        };
        for job in &instances.jobs {
            let position = self.origin
                + Vec3::new(
                    job.position[0] as f32,
                    job.position[1] as f32,
                    job.position[2] as f32,
                );
            let cell =
                (job.position[2] * self.size[1] + job.position[1]) * self.size[0] + job.position[0];
            for face in
                &self.templates[job.first_face as usize..(job.first_face + job.face_count) as usize]
            {
                let y = position.y
                    + face.vertices.iter().map(|v| v.position.y).sum::<f32>() * 0.25 * job.height;
                let delta = self.neighbour_delta(face);
                if y < self.clip_y[0]
                    || y >= self.clip_y[1]
                    || (delta != 0
                        && instances
                            .classes
                            .cell((i64::from(cell) + i64::from(delta)) as usize)
                            & job.occlusion_mask
                            != 0)
                {
                    continue;
                }
                let light = if job.height == 1.0 {
                    self.face_ambient_light(face, job.position, true)
                } else {
                    [1.0; 4]
                };
                let illumination = self.face_light(face, job.position, job.height);
                emit(
                    job.material,
                    std::array::from_fn(|corner| {
                        let mut v = face.vertices[corner];
                        for c in &mut v.color[..3] {
                            *c *= light[corner] * illumination[corner];
                        }
                        v.position = position
                            + Vec3::new(v.position.x, v.position.y * job.height, v.position.z);
                        v
                    }),
                );
            }
        }
    }
}
