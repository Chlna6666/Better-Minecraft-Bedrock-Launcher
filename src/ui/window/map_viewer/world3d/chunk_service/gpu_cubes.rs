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
        let mut templates = Vec::with_capacity(faces.len());
        let Some(mut output) = self.build_chunk_mesh_with_templates(
            pos,
            band.section_y,
            faces,
            Some(&mut templates),
            band.template_faces,
        )?
        else {
            return Ok(None);
        };
        // Neighbour offsets for canonical cube faces come from the decoded
        // compact grid, not from the visual face normal. Extra/static faces
        // keep their original normal; GPU source validation handles each type.
        for (template, &neighbour) in templates
            .iter_mut()
            .take(band.template_faces)
            .zip(band.neighbours.iter())
        {
            template.neighbour = neighbour;
        }
        if templates.len() != faces.len() || band.neighbours.len() != band.template_faces {
            return Err("GPU 方块模板与区块面数不匹配".to_string());
        }
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

#[cfg(test)]
mod direct_template_tests {
    use super::*;

    #[test]
    fn direct_gpu_template_matches_cpu_packed_quad_geometry() {
        let face = p3d::Preview3dStreamFace {
            ambient_light: [1.0; 4],
            corners: [
                [0.0, 4.0, 0.0], [1.0, 4.0, 0.0],
                [1.0, 4.0, 1.0], [0.0, 4.0, 1.0],
            ],
            color: [1.0; 4],
            shade: 1.0,
            normal: [0, 1, 0],
            material: Arc::from("unit_test"),
            uv: Some([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
            pass: p3d::Preview3dStreamPass::Opaque,
        };
        for local in [false, true] {
            let quad = world3d_face_vertices(
                &face, [128.0, 0.0, 256.0], local, [1.0; 4]
            );
            let mesh = Mesh::from_voxel_quads_without_raycast(
                [quad], vec![MeshPart::new(0, 6, 0)],
            ).unwrap();
            let template = gpui_3d::VoxelFaceTemplate {
                vertices: quad, material_slot: 0, neighbour: face.normal,
            };
            assert_eq!(mesh.vertices(), template.vertices.as_slice());
            assert_eq!(mesh.parts()[0].material_slot(), template.material_slot);
            assert_eq!(mesh.index_count(), 6);
        }
    }
}
