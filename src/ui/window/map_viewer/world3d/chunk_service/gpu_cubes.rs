//! Resolve small cube template tables while leaving cell faces for GPU generation.
use super::*;

impl Worker {
    pub(super) fn build_gpu_cube_mesh(
        &mut self,
        pos: ChunkPos,
        band: p3d::GpuCubeBand,
        faces: &[p3d::Preview3dStreamFace],
        lighting: Option<&super::lighting::ChunkLighting>,
    ) -> Result<Option<World3dChunkMesh>, String> {
        let levels = lighting.map(|light| light.gpu_levels(pos, &band));
        let mut faces = faces.to_vec();
        for face in &mut faces[..band.template_faces] {
            for corner in &mut face.corners {
                corner[0] += f64::from(pos.x) * 16.0;
                corner[2] += f64::from(pos.z) * 16.0;
            }
        }
        let mut order = Vec::new();
        let Some(mut output) =
            self.build_chunk_mesh_with_order(pos, band.section_y, &faces, Some(&mut order))?
        else {
            return Ok(None);
        };
        let vertices = output.mesh.vertices();
        let mut templates = vec![None; faces.len()];
        for (quad, &original) in order.iter().enumerate() {
            let part = output
                .mesh
                .parts()
                .iter()
                .find(|part| {
                    part.first_index() as usize <= quad * 6
                        && quad * 6 < (part.first_index() + part.index_count()) as usize
                })
                .ok_or_else(|| "GPU 方块模板缺少材质分段".to_string())?;
            templates[original] = Some(gpui_3d::VoxelFaceTemplate {
                vertices: std::array::from_fn(|corner| vertices[quad * 4 + corner]),
                material_slot: part.material_slot(),
                neighbour: if original < band.template_faces {
                    band.neighbours[original]
                } else {
                    faces[original].normal
                },
            });
        }
        let mut templates = templates
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| "GPU 方块模板不完整".to_string())?;
        let occluders = super::occluders::gpu_boxes(&band, &templates, &output.materials);
        let extra = templates.split_off(band.template_faces);
        let instance_templates = templates.split_off(band.model_faces);
        let source = gpui_3d::VoxelMeshSource::new(band.size, band.origin, band.cells, templates)
            .and_then(|source| {
                source.with_instances(band.classes, instance_templates, band.instances)
            })
            .and_then(|source| source.with_static_faces(extra))
            .map(|source| source.with_ambient_occlusion(if lighting.is_some() { 16 } else { 0 }))
            .and_then(|source| match (levels, lighting) {
                (Some(levels), Some(light)) => source.with_lighting(levels, light.ambient),
                _ => Ok(source),
            })
            .and_then(|source| {
                source.with_y_range(
                    (band.section_y * 64) as f32,
                    ((band.section_y + 1) * 64) as f32,
                )
            })
            .map_err(|error| format!("GPU 构网输入无效: {error}"))?;
        let mesh = Mesh::from_gpu_voxels(source)
            .with_identity(chunk_mesh_id(pos, band.section_y), self.mesh_generation);
        output.pos = pos;
        output.section_y = band.section_y;
        output.origin = [pos.x * 16, 0, pos.z * 16];
        // GPU output count is consumed directly by rendering; this diagnostic is capacity.
        output.faces = mesh.index_count() / 6;
        output.triangles = output.faces * 2;
        output.mesh = Arc::new(mesh);
        output.occluders = occluders;
        Ok(Some(output))
    }
}
