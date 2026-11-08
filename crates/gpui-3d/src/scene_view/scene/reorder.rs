//! Reorders transparent bindings while keeping GPU tables and argument addresses resident.

use super::*;
use crate::scene_view::gpu::encode_instance_records;

#[cfg(all(test, target_os = "windows"))]
#[path = "reorder_native_tests.rs"]
mod native_tests;

impl SceneResources {
    /// Retained for native test coverage of the transparent-reorder helper.
    /// The production GPU-culling path keeps the blended tail in its
    /// camera-independent candidate order and renders it through
    /// `blend_order`, so tables are never permuted for transparency anymore.
    #[cfg(test)]
    pub(super) fn sync_reordered_blends(&mut self, start: usize) -> gpui::Result<()> {
        let first_batch = self
            .batches
            .partition_point(|batch| batch.first_instance < start);
        let order = self.prepared.blended_order();
        let end = first_batch + order.len();
        if self.draw_resources.len() != end
            || self.draw_resource_keys.len() != end
            || self.draw_addresses.len() != end
            || self.prepared.draws.len() - start != order.len()
        {
            return Err(anyhow!(
                "GPUI 3D transparent order does not match resident bindings"
            ));
        }
        crate::prepare::apply_order(
            &mut self.draw_resources[first_batch..],
            order,
            &mut self.reorder_positions,
        );
        crate::prepare::apply_order(
            &mut self.draw_resource_keys[first_batch..],
            order,
            &mut self.reorder_positions,
        );
        // Positional fast-path source IDs must follow precisely the same
        // permutation as the associated BindGroups and Page metadata.
        if self.draw_resource_sources.len() == end {
            crate::prepare::apply_order(
                &mut self.draw_resource_sources[first_batch..],
                order,
                &mut self.reorder_positions,
            );
        }
        if self.native_command_sources.len() == end {
            crate::prepare::apply_order(
                &mut self.native_command_sources[first_batch..],
                order,
                &mut self.reorder_positions,
            );
        }
        // The Generated Mapping source belongs to the same drawable as its
        // BindGroup. Keep them together during camera-only blended sorting.
        if self.generated_batch_sources.len() == end {
            crate::prepare::apply_order(
                &mut self.generated_batch_sources[first_batch..],
                order,
                &mut self.reorder_positions,
            );
        }
        // A pending Page mapping was assembled for the former logical order.
        // Any later direct sync must rebuild it after this permutation.
        self.generated_pending_groups = None;
        crate::prepare::apply_order(
            &mut self.draw_addresses[first_batch..],
            order,
            &mut self.reorder_positions,
        );
        Ok(())
    }

    pub(super) fn encode_draw_instances(
        &self,
        draws: &[PreparedDraw],
        voxel_shader: bool,
        bytes: &mut Vec<u8>,
    ) -> gpui::Result<()> {
        bytes.clear();
        bytes.reserve(draws.len().max(1).saturating_mul(INSTANCE_STRIDE as usize));
        encode_instance_records(draws, bytes)?;
        if !voxel_shader {
            return Ok(());
        }
        for (index, draw) in draws.iter().enumerate() {
            let mesh = self
                .meshes
                .get(&mesh_key(draw, true))
                .ok_or_else(|| anyhow!("GPUI 3D mesh was not uploaded"))?;
            if self.culling.is_some() && mesh.procedural_quads {
                let offset = index * INSTANCE_STRIDE as usize + 64;
                bytes[offset..offset + 4].copy_from_slice(&draw.part.index_count().to_le_bytes());
            }
            let vertex_base = if mesh.procedural_quads {
                mesh.vertex_base
                    .checked_add(
                        draw.part.first_index() / 6 * if mesh.generated.is_some() { 1 } else { 4 },
                    )
                    .filter(|base| *base < 0x8000_0000)
                    .ok_or_else(|| anyhow!("GPUI 3D voxel quad address exceeds shader range"))?
                    | 0x8000_0000
            } else {
                mesh.vertex_base
            };
            let offset = index * INSTANCE_STRIDE as usize + 140;
            bytes[offset..offset + 4].copy_from_slice(&vertex_base.to_ne_bytes());
            if let Some(generated) = &mesh.generated {
                // The voxel shader does not use normal_matrix for normals.
                // Column 0.yz carry the 32-bit template record index as two
                // numerically exact f32 16-bit halves. Never bitcast a u32
                // into f32 here: DX11/GL may flush denormal values to zero.
                // Column 0.x remains the indirect vertex count.
                let offset = index * INSTANCE_STRIDE as usize + 68;
                let lo = (generated.template_base & 0xffff) as f32;
                let hi = (generated.template_base >> 16) as f32;
                bytes[offset..offset + 4].copy_from_slice(&lo.to_ne_bytes());
                bytes[offset + 4..offset + 8].copy_from_slice(&hi.to_ne_bytes());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_view::gpu::encode_draw_records;
    use crate::{Material, Mesh, Node, Transform, Vec3};

    #[test]
    fn transparent_reordering_keeps_resident_records_across_camera_changes() {
        let mut scene = Scene::new();
        let mesh = Arc::new(Mesh::cube());
        for index in 0..3 {
            let mut material = Material::new();
            material.alpha_mode = if index == 0 {
                AlphaMode::Opaque
            } else {
                AlphaMode::Blend
            };
            material.base_color = [index as f32 * 0.3, 0.4, 0.8, 0.5];
            scene
                .insert(
                    None,
                    Node::new()
                        .with_mesh(mesh.clone())
                        .with_materials([Arc::new(material)])
                        .with_transform(Transform {
                            translation: Vec3::new(0.0, 0.0, -(index as f32)),
                            ..Transform::IDENTITY
                        }),
                )
                .unwrap();
        }
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            100.0,
        );
        let mut prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();
        assert_eq!(prepared.draws.len(), 3);
        let original_draws = encode_draw_records(&prepared.draws, 0).unwrap();
        let mut original_instances = Vec::new();
        encode_instance_records(&prepared.draws, &mut original_instances).unwrap();
        let resident = prepared.draws[1..]
            .iter()
            .enumerate()
            .map(|(index, draw)| (draw_identity(draw), resident_binding(draw, index + 1)))
            .collect::<Vec<_>>();
        prepared.draws.swap(1, 2);
        let full_draws = encode_draw_records(&prepared.draws, 0).unwrap();
        let suffix_draws = encode_draw_records(&prepared.draws[1..], 0).unwrap();
        assert_eq!(
            &full_draws[..DRAW_PARAMS_STRIDE as usize],
            &original_draws[..DRAW_PARAMS_STRIDE as usize]
        );
        assert_eq!(&full_draws[DRAW_PARAMS_STRIDE as usize..], suffix_draws);
        assert_ne!(
            &full_draws[DRAW_PARAMS_STRIDE as usize..],
            &original_draws[DRAW_PARAMS_STRIDE as usize..]
        );
        let mut full_instances = Vec::new();
        let mut suffix_instances = Vec::new();
        encode_instance_records(&prepared.draws, &mut full_instances).unwrap();
        encode_instance_records(&prepared.draws[1..], &mut suffix_instances).unwrap();
        assert_eq!(
            &full_instances[..INSTANCE_STRIDE as usize],
            &original_instances[..INSTANCE_STRIDE as usize]
        );
        assert_eq!(
            &full_instances[INSTANCE_STRIDE as usize..],
            suffix_instances
        );
        let mut reordered = resident
            .iter()
            .map(|(_, binding)| *binding)
            .collect::<Vec<_>>();
        let mut positions = Vec::new();
        crate::prepare::apply_order(&mut reordered, &[1, 0], &mut positions);
        assert_eq!(
            reordered
                .iter()
                .map(|binding| binding.2.first_instance)
                .collect::<Vec<_>>(),
            [2, 1]
        );
        for (draw, binding) in prepared.draws[1..].iter().zip(&reordered) {
            let offset = binding.2.first_instance * DRAW_PARAMS_STRIDE as usize;
            assert_eq!(
                &original_draws[offset..offset + DRAW_PARAMS_STRIDE as usize],
                encode_draw_records(std::slice::from_ref(draw), 0).unwrap()
            );
            assert_eq!(
                binding.2.arguments_offset,
                binding.2.first_instance as u64 * ARGUMENT_STRIDE
            );
        }
        crate::prepare::apply_order(&mut reordered, &[1, 0], &mut positions);
        assert_eq!(
            reordered
                .iter()
                .map(|binding| binding.2.first_instance)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        let mut values = [0, 1, 2, 3];
        crate::prepare::apply_order(&mut values, &[2, 0, 3, 1], &mut positions);
        assert_eq!(values, [2, 0, 3, 1]);
    }

    fn resident_binding(
        draw: &PreparedDraw,
        index: usize,
    ) -> (BindGroupId, DrawResourceKey, DrawAddress) {
        (
            BindGroupId::from_parts(index as u32, 1),
            DrawResourceKey {
                shadow: None,
                mesh: DrawMeshKey::Mesh {
                    id: draw.mesh.id(),
                    generation: draw.mesh.generation(),
                },
                visible_instances: None,
                albedo_id: None,
                normal_id: None,
                occlusion_id: None,
                metallic_roughness_id: None,
            },
            DrawAddress {
                first_instance: index,
                arguments_offset: index as u64 * ARGUMENT_STRIDE,
            },
        )
    }

    #[test]
    #[ignore = "CPU workload timing; does not create a window or GPU device"]
    fn profile_retained_transparent_order() {
        use std::{hint::black_box, time::Instant};
        let mut scene = Scene::new();
        let mesh = Arc::new(Mesh::cube());
        let mut material = Material::new();
        material.alpha_mode = AlphaMode::Blend;
        let material = Arc::new(material);
        for index in 0..20_000 {
            scene
                .insert(
                    None,
                    Node::new()
                        .with_mesh(mesh.clone())
                        .with_materials([material.clone()])
                        .with_transform(Transform {
                            translation: Vec3::new(
                                (index % 200) as f32 * 0.01,
                                0.0,
                                -(index / 200) as f32 * 0.01,
                            ),
                            ..Transform::IDENTITY
                        }),
                )
                .unwrap();
        }
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            100.0,
        );
        let prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();
        assert_eq!(prepared.draws.len(), 20_000);
        let resident = prepared
            .draws
            .iter()
            .enumerate()
            .map(|(index, draw)| (draw_identity(draw), resident_binding(draw, index)))
            .collect::<Vec<_>>();
        let start = Instant::now();
        for _ in 0..20 {
            black_box(encode_draw_records(&prepared.draws, 0).unwrap());
            let mut bytes = Vec::new();
            encode_instance_records(&prepared.draws, &mut bytes).unwrap();
            black_box(bytes);
        }
        let encoding = start.elapsed();
        let mut groups = resident
            .iter()
            .map(|(_, binding)| binding.0)
            .collect::<Vec<_>>();
        let mut keys = resident
            .iter()
            .map(|(_, binding)| binding.1)
            .collect::<Vec<_>>();
        let mut addresses = resident
            .iter()
            .map(|(_, binding)| binding.2)
            .collect::<Vec<_>>();
        let mut order = (0..prepared.draws.len()).collect::<Vec<_>>();
        order.rotate_left(1);
        let mut positions = Vec::new();
        let start = Instant::now();
        for _ in 0..20 {
            crate::prepare::apply_order(&mut groups, &order, &mut positions);
            crate::prepare::apply_order(&mut keys, &order, &mut positions);
            crate::prepare::apply_order(&mut addresses, &order, &mut positions);
            black_box(&addresses);
        }
        eprintln!(
            "transparent CPU records={} frames=20 removed_encoding_ms={:.2} integer_remap_ms={:.2} bytes_no_longer_encoded_per_frame={}",
            prepared.draws.len(),
            encoding.as_secs_f64() * 1000.0,
            start.elapsed().as_secs_f64() * 1000.0,
            prepared.draws.len() * (DRAW_PARAMS_STRIDE + INSTANCE_STRIDE) as usize
        );
    }
}
