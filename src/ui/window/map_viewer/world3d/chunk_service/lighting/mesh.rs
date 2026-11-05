//! Attach raw static model faces to the same GPU light field as generated cubes.
use super::*;

impl ChunkLighting {
    pub(in crate::ui::window::map_viewer::world3d::chunk_service) fn static_source_faces(
        &self,
        faces: Vec<gpui_3d::VoxelFaceTemplate>,
    ) -> Result<gpui_3d::VoxelMeshSource, String> {
        // Compute light-field bounds from the immutable GPU face templates,
        // without building a temporary CPU vertex/index Mesh first.
        let mut min = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut max = -min;
        for face in &faces {
            for vertex in &face.vertices {
                min = min.min(vertex.position);
                max = max.max(vertex.position);
            }
        }
        if faces.is_empty() {
            return Err("模型缺少光照范围".to_string());
        }
        let origin = Vec3::new(
            min.x.floor() - 1.,
            min.y.floor() - 1.,
            min.z.floor() - 1.,
        );
        let size = [
            (max.x.ceil() - origin.x + 1.) as u32,
            (max.y.ceil() - origin.y + 1.) as u32,
            (max.z.ceil() - origin.z + 1.) as u32,
        ];
        // A planar model still needs one interior cell and a halo on both sides.
        let size = size.map(|extent| extent.max(3));
        let count = size
            .into_iter()
            .try_fold(1usize, |count, n| count.checked_mul(n as usize))
            .filter(|count| *count <= 4_194_304)
            .ok_or_else(|| "模型光照范围过大".to_string())?;
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
