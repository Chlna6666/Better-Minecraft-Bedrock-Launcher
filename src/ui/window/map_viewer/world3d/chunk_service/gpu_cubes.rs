//! Resolve reusable cube templates into compact static meshes or dynamically lit GPU sources.
use super::*;

impl Worker {
    pub(super) fn build_gpu_cube_mesh(
        &mut self,
        pos: ChunkPos,
        band: p3d::GpuCubeBand,
        faces: &[p3d::Preview3dStreamFace],
        lighting: Option<&super::lighting::ChunkLighting>,
    ) -> Result<Option<World3dChunkMesh>, String> {
        let lighting = lighting.filter(|_| self.lighting.is_lit());
        let levels = lighting.map(|light| light.gpu_levels(pos, &band));
        let mut order = Vec::new();
        let Some(mut output) = self.build_chunk_mesh_with_order(
            pos,
            band.section_y,
            faces,
            Some(&mut order),
            band.template_faces,
        )?
        else {
            return Ok(None);
        };
        let vertices = output.mesh.vertices();
        let mut templates = vec![None; faces.len()];
        // Packed quads and MeshPart ranges are both monotonic. A cursor avoids
        // scanning every material part for every face in a large chunk section.
        let mut part_cursor = 0usize;
        for (quad, &original) in order.iter().enumerate() {
            let material_slot = packed_quad_material_slot(
                output.mesh.parts(),
                quad,
                &mut part_cursor,
            )
            .ok_or_else(|| "GPU 方块模板缺少材质分段".to_string())?;
            templates[original] = Some(gpui_3d::VoxelFaceTemplate {
                vertices: std::array::from_fn(|corner| vertices[quad * 4 + corner]),
                material_slot,
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
        let compact = !self.lighting.is_lit();
        #[cfg(test)]
        let compact = compact && std::env::var("BMCBL_WORLD3D_TEST_GPU_MESH").as_deref() != Ok("1");
        let mesh = if compact {
            source
                .compact_mesh()
                .map_err(|error| format!("构造紧凑网格失败: {error}"))?
        } else {
            Mesh::from_gpu_voxels(source)
        }
        .with_identity(chunk_mesh_id(pos, band.section_y), self.mesh_generation);
        output.pos = pos;
        output.section_y = band.section_y;
        output.origin = [pos.x * 16, 0, pos.z * 16];
        // GPU sources report reserved capacity; compact meshes report their actual face count.
        output.faces = mesh.index_count() / 6;
        output.triangles = output.faces * 2;
        output.mesh = Arc::new(mesh);
        output.occluders = occluders;
        Ok(Some(output))
    }
}


/// Resolve monotonically packed quads to their material part in O(quads + parts).
fn packed_quad_material_slot(
    parts: &[MeshPart],
    quad: usize,
    cursor: &mut usize,
) -> Option<u32> {
    let first_index = quad.checked_mul(6)?;
    while let Some(part) = parts.get(*cursor) {
        let end = (part.first_index() as usize).checked_add(part.index_count() as usize)?;
        if first_index < end {
            break;
        }
        *cursor += 1;
    }
    parts.get(*cursor)
        .filter(|part| first_index >= part.first_index() as usize)
        .map(|part| part.material_slot())
}

#[cfg(test)]
mod material_cursor_tests {
    use super::{MeshPart, packed_quad_material_slot};

    #[test]
    fn packed_material_cursor_crosses_section_part_boundaries() {
        let parts = [
            MeshPart::new(0, 12, 4),
            MeshPart::new(12, 6, 9),
            MeshPart::new(18, 18, 2),
        ];
        let mut cursor = 0;
        let mapped = (0..6)
            .map(|quad| packed_quad_material_slot(&parts, quad, &mut cursor))
            .collect::<Vec<_>>();
        assert_eq!(mapped, vec![Some(4), Some(4), Some(9), Some(2), Some(2), Some(2)]);
        assert_eq!(cursor, 2);
        assert_eq!(packed_quad_material_slot(&parts, 6, &mut cursor), None);
    }

    #[test]
    fn packed_material_cursor_rejects_unmapped_or_gapped_quads() {
        let parts = [MeshPart::new(6, 6, 7)];
        let mut cursor = 0;
        assert_eq!(packed_quad_material_slot(&parts, 0, &mut cursor), None);
        assert_eq!(packed_quad_material_slot(&parts, 1, &mut cursor), Some(7));
    }
}
