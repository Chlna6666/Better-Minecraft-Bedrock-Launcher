//! Attach raw static model faces to the same GPU light field as generated cubes.
use super::*;

impl ChunkLighting {
    pub(in crate::ui::window::map_viewer::world3d::chunk_service) fn static_source(
        &self,
        mesh: &Mesh,
    ) -> Result<gpui_3d::VoxelMeshSource, String> {
        let bounds = mesh
            .bounds()
            .ok_or_else(|| "模型缺少光照范围".to_string())?;
        let origin = Vec3::new(
            bounds.min.x.floor() - 1.,
            bounds.min.y.floor() - 1.,
            bounds.min.z.floor() - 1.,
        );
        let size = [
            (bounds.max.x.ceil() - origin.x + 1.) as u32,
            (bounds.max.y.ceil() - origin.y + 1.) as u32,
            (bounds.max.z.ceil() - origin.z + 1.) as u32,
        ];
        // A planar model still needs one interior cell and a halo on both sides.
        let size = size.map(|extent| extent.max(3));
        let count = size
            .into_iter()
            .try_fold(1usize, |count, n| count.checked_mul(n as usize))
            .filter(|count| *count <= 4_194_304)
            .ok_or_else(|| "模型光照范围过大".to_string())?;
        let vertices = mesh.vertices();
        let mut faces = Vec::with_capacity(vertices.len() / 4);
        for part in mesh.parts() {
            let first = part.first_index() as usize / 6 * 4;
            let end = first + part.index_count() as usize / 6 * 4;
            for quad in vertices[first..end].chunks_exact(4) {
                faces.push(gpui_3d::VoxelFaceTemplate {
                    vertices: std::array::from_fn(|i| quad[i]),
                    material_slot: part.material_slot(),
                    neighbour: [
                        quad[0].normal.x as i32,
                        quad[0].normal.y as i32,
                        quad[0].normal.z as i32,
                    ],
                });
            }
        }
        let levels = (0..count)
            .map(|i| {
                self.packed_at([
                    self.origin[0] + 1 + origin.x as i32 + (i % size[0] as usize) as i32,
                    origin.y as i32 + (i / size[0] as usize % size[1] as usize) as i32,
                    self.origin[2]
                        + 1
                        + origin.z as i32
                        + (i / size[0] as usize / size[1] as usize) as i32,
                ])
            })
            .collect();
        gpui_3d::VoxelMeshSource::new(size, origin, vec![0; count], vec![])
            .and_then(|source| source.with_static_faces(faces))
            .and_then(|source| source.with_lighting(levels, self.ambient))
            .map_err(|error| format!("模型光照输入无效：{error}"))
    }
}
