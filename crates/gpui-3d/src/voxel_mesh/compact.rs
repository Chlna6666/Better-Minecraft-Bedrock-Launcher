//! Shared visible-quad emission for compact static meshes and the padded CPU fallback.
use super::*;
use std::collections::HashMap;

impl VoxelMeshSource {
    /// Materializes only visible faces into the resident quad format, with no capacity padding.
    /// Cell lighting and AO are baked as in the CPU fallback; dynamic cell-light fields are not
    /// retained. Part material slots and translucent face order are preserved. No BVH is built.
    ///
    /// # Errors
    /// Returns a mesh error if emitted geometry exceeds the supported draw range.
    pub fn compact_mesh(&self) -> Result<Mesh, MeshError> {
        let slots = self
            .parts
            .iter()
            .enumerate()
            .map(|(i, p)| (p.material_slot(), i))
            .collect::<HashMap<_, _>>();
        let mut buckets = vec![Vec::<[Vertex; 4]>::new(); self.parts.len()];
        self.visit_quads(|material, quad| {
            buckets[slots[&material]].push(quad);
        });
        let mut parts = Vec::new();
        let mut first = 0u32;
        for (source, quads) in self.parts.iter().zip(&buckets) {
            let count = u32::try_from(quads.len())
                .ok()
                .and_then(|v| v.checked_mul(6))
                .ok_or(MeshError::TooLarge)?;
            if count == 0 {
                continue;
            }
            parts.push(MeshPart::new(first, count, source.material_slot()));
            first = first.checked_add(count).ok_or(MeshError::TooLarge)?;
        }
        Mesh::from_voxel_quads_without_raycast(buckets.into_iter().flatten(), parts)
    }

    pub(super) fn visit_quads(&self, mut emit: impl FnMut(u32, [Vertex; 4])) {
        self.visit_cell_quads(&mut emit);
        for face in &self.templates[self.static_start..] {
            let y = face.vertices.iter().map(|v| v.position.y).sum::<f32>() * 0.25;
            if y < self.clip_y[0] || y >= self.clip_y[1] {
                continue;
            }
            let illumination = self.static_light(face);
            emit(
                face.material_slot,
                face.vertices.map(|mut vertex| {
                    for color in &mut vertex.color[..3] {
                        *color *= illumination;
                    }
                    vertex
                }),
            );
        }
        self.visit_instance_quads(emit);
    }

    fn visit_cell_quads(&self, emit: &mut impl FnMut(u32, [Vertex; 4])) {
        for (index, kind) in self.cells.iter().enumerate() {
            if kind == 0 || kind == VOXEL_OCCLUDER {
                continue;
            }
            let cell = [
                index as u32 % self.size[0],
                index as u32 / self.size[0] % self.size[1],
                index as u32 / self.size[0] / self.size[1],
            ];
            let position = self.origin + Vec3::new(cell[0] as f32, cell[1] as f32, cell[2] as f32);
            for face in &self.templates[(kind - 1) as usize * 6..kind as usize * 6] {
                let y = position.y + face.vertices.iter().map(|v| v.position.y).sum::<f32>() * 0.25;
                if y < self.clip_y[0]
                    || y >= self.clip_y[1]
                    || self
                        .cells
                        .cell((index as i64 + i64::from(self.neighbour_delta(face))) as usize)
                        != 0
                {
                    continue;
                }
                let light = self.face_ambient_light(face, cell, false);
                let illumination = self.face_light(face, cell, 1.0);
                emit(
                    face.material_slot,
                    std::array::from_fn(|corner| {
                        let mut vertex = face.vertices[corner];
                        for c in &mut vertex.color[..3] {
                            *c *= light[corner] * illumination[corner];
                        }
                        vertex.position = vertex.position + position;
                        vertex
                    }),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn compact_faces_match_padded_fallback_without_unused_capacity() {
        let source = || {
            let base = super::super::tests::source();
            let templates = base.templates.to_vec();
            let classes = base.cells.iter().map(|v| u32::from(v != 0)).collect();
            base.with_instances(
                classes,
                templates,
                vec![super::super::VoxelMeshInstance {
                    position: [1, 1, 1],
                    faces: 0..6,
                    height: 0.4,
                    occlusion_mask: 1,
                }],
            )
            .unwrap()
            .with_lighting(vec![0x73; 36], 0.2)
            .unwrap()
            .with_ambient_occlusion(24)
        };
        for source in [source(), source().with_y_range(0., 0.5).unwrap()] {
            let compact = source.compact_mesh().unwrap();
            assert!(
                source.fallback.get().is_none(),
                "compaction must not allocate the padded fallback"
            );
            let expected = source
                .fallback()
                .vertices()
                .chunks_exact(4)
                .filter(|q| q[0].color[3] > 0.)
                .flat_map(|q| q.iter().copied())
                .collect::<Vec<_>>();
            assert_eq!(compact.vertices(), expected.as_slice());
            assert_eq!(compact.index_count(), expected.len() / 4 * 6);
            assert!(compact.vertex_count() < source.capacity() * 4);
        }
    }
}
