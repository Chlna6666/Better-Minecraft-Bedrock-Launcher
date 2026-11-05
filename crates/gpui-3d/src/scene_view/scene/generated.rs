//! Materialize per-draw arguments from GPU-generated per-material face counts.
use super::*;
use gfx_core::{
    BufferBinding, ComputeBufferAccess, ComputeBufferBinding, ComputeDispatch, MemoryLocation,
};

pub(super) struct Mapping {
    pub buffer: BufferId,
    capacity: usize,
    entries: Vec<u32>,
    output: Option<BufferId>,
}
struct Update {
    source: BufferId,
    source_size: usize,
    input: BufferId,
    entries: Vec<u32>,
    bytes: Vec<u8>,
}

impl SceneResources {
    pub(super) fn sync_generated_draws(
        &mut self,
        device: &mut dyn ExtensionDevice,
        voxel_shader: bool,
    ) -> gpui::Result<()> {
        let mut groups = HashMap::<BufferId, (usize, Vec<u32>)>::new();
        for (index, batch) in self.batches.iter().enumerate() {
            let draw = &self.prepared.draws[batch.first_instance];
            let mesh = &self.meshes[&mesh_key(draw, voxel_shader)];
            let Some(generated) = &mesh.generated else {
                continue;
            };
            let part = draw
                .mesh
                .parts()
                .iter()
                .position(|part| *part == draw.part)
                .ok_or_else(|| anyhow!("missing generated material part"))?;
            let (_, mapping) = groups
                .entry(generated.arguments)
                .or_insert_with(|| (draw.mesh.parts().len() * 16, Vec::new()));
            mapping.extend([
                part as u32,
                index as u32,
                u32::try_from(batch.first_instance).context("generated draw index overflow")?,
                0,
            ]);
        }
        let stale = self
            .generated_mappings
            .keys()
            .filter(|source| !groups.contains_key(source))
            .copied()
            .collect::<Vec<_>>();
        for source in stale {
            if let Some(mapping) = self.generated_mappings.remove(&source) {
                device.destroy_buffer(mapping.buffer)?;
            }
        }
        if groups.is_empty() {
            return Ok(());
        }
        let needed = self.batches.len().next_power_of_two();
        if self.generated_arguments.is_none_or(|(_, cap)| cap < needed) {
            let buffer = device.create_buffer(&BufferDescriptor {
                label: Some("generated voxel draw arguments".into()),
                size: (needed * 16) as u64,
                usage: BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE | BufferUsage::INDIRECT,
                memory_location: MemoryLocation::GpuOnly,
            })?;
            if let Some((previous, _)) = self.generated_arguments.replace((buffer, needed)) {
                device.destroy_buffer(previous)?;
            }
        }
        if self.mesh_draw_shader.is_none() {
            self.mesh_draw_shader = Some(super::super::generated::create_shader(
                device,
                self.backend,
                "cs_mesh_draws",
            )?);
        }
        let (output, capacity) = self
            .generated_arguments
            .ok_or_else(|| anyhow!("missing generated draw output"))?;
        let mut updates = Vec::new();
        for (source, (source_size, entries)) in groups {
            if self
                .generated_mappings
                .get(&source)
                .is_some_and(|mapping| mapping.entries == entries && mapping.output == Some(output))
            {
                continue;
            }
            let needed = entries.len() * 4;
            if self
                .generated_mappings
                .get(&source)
                .is_none_or(|mapping| mapping.capacity < needed)
            {
                let capacity = needed.next_power_of_two();
                let buffer = device.create_buffer(&BufferDescriptor {
                    label: Some("generated draw mapping".into()),
                    size: capacity as u64,
                    usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
                    memory_location: MemoryLocation::GpuOnly,
                })?;
                if let Some(previous) = self.generated_mappings.insert(
                    source,
                    Mapping {
                        buffer,
                        capacity,
                        entries: Vec::new(),
                        output: None,
                    },
                ) {
                    device.destroy_buffer(previous.buffer)?;
                }
            }
            let input = self.generated_mappings[&source].buffer;
            let bytes = entries.iter().flat_map(|v| v.to_le_bytes()).collect();
            updates.push(Update {
                source,
                source_size,
                input,
                entries,
                bytes,
            });
        }
        if updates.is_empty() {
            return Ok(());
        }
        let writes = updates
            .iter()
            .map(|update| gfx_core::BufferWrite {
                descriptor: gfx_core::BufferWriteDescriptor {
                    buffer: update.input,
                    offset: 0,
                },
                data: &update.bytes,
            })
            .collect::<Vec<_>>();
        device.write_buffer_batch(&writes)?;
        let bindings = updates
            .iter()
            .map(|update| {
                [
                    (
                        update.source,
                        update.source_size,
                        ComputeBufferAccess::ReadOnly,
                    ),
                    (
                        update.input,
                        update.bytes.len(),
                        ComputeBufferAccess::ReadOnly,
                    ),
                    (output, capacity * 16, ComputeBufferAccess::ReadWrite),
                ]
                .into_iter()
                .enumerate()
                .map(|(slot, (buffer, size, access))| ComputeBufferBinding {
                    binding: slot as u32,
                    buffer: BufferBinding {
                        buffer,
                        offset: 0,
                        size: size as u64,
                        stride: None,
                    },
                    access,
                })
                .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let shader = self
            .mesh_draw_shader
            .ok_or_else(|| anyhow!("missing generated draw shader"))?;
        let compute_caps = device.compute_capabilities();
        let dispatches = updates
            .iter()
            .zip(&bindings)
            .map(|(update, bindings)| {
                let count = u32::try_from(update.entries.len().div_ceil(4 * 64))
                    .context("generated draw dispatch overflow")?;
                let grid = compute_caps.plan_linear_dispatch_2d(count)?;
                Ok(ComputeDispatch {
                    shader,
                    bindings,
                    workgroups: grid,
                })
            })
            .collect::<gpui::Result<Vec<_>>>()?;
        device.dispatch_compute_batch(&dispatches)?;
        for update in updates {
            let mapping = self
                .generated_mappings
                .get_mut(&update.source)
                .ok_or_else(|| anyhow!("generated draw mapping disappeared"))?;
            mapping.entries = update.entries;
            mapping.output = Some(output);
        }
        Ok(())
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use crate::{Material, Mesh, Node, Vec3};
    use gfx_core::DeviceDescriptor;
    use gfx_dx11::Dx11Device;

    #[test]
    #[ignore = "requires D3D11 hardware; verifies mesh lookup during voxel shader transitions"]
    fn generated_draws_use_current_voxel_shader_mode() {
        let mut device = Dx11Device::new(&DeviceDescriptor::default()).unwrap();
        let mesh = Arc::new(Mesh::cube());
        let mut material = Material::new();
        material.shading_model = ShadingModel::Unlit;
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        for current_mode in [true, false] {
            let mut resources = SceneResources::new(&mut device, BackendKind::Dx11).unwrap();
            resources.voxel_shader = !current_mode;
            resources.prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();
            assert_eq!(resources.prepared.draws.len(), 1);
            resources.batches.push(DrawBatchRange {
                first_instance: 0,
                instance_count: 1,
            });
            resources
                .ensure_meshes(&mut device, BackendKind::Dx11, current_mode)
                .unwrap();
            assert!(resources.meshes.contains_key(&(mesh.id(), current_mode)));
            assert!(!resources.meshes.contains_key(&(mesh.id(), !current_mode)));
            resources
                .sync_generated_draws(&mut device, current_mode)
                .unwrap();
            resources.destroy(&mut device).unwrap();
        }
    }

    #[test]
    #[ignore = "requires D3D11 hardware; verifies bounded mapping reuse and output invalidation"]
    fn generated_mappings_reuse_buffers_and_follow_output_replacement() {
        let mut device = Dx11Device::new(&DeviceDescriptor::default()).unwrap();
        let mesh = Arc::new(Mesh::from_gpu_voxels(crate::voxel_mesh::tests::source()));
        let mut material = Material::new();
        material.shading_model = ShadingModel::Unlit;
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh)
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            100.0,
        );
        let mut resources = SceneResources::new(&mut device, BackendKind::Dx11).unwrap();
        resources.prepared = PreparedScene::new(&scene, camera, 1.0).unwrap();
        resources
            .ensure_meshes(&mut device, BackendKind::Dx11, true)
            .unwrap();
        resources.batches.push(DrawBatchRange {
            first_instance: 0,
            instance_count: 1,
        });
        resources.sync_generated_draws(&mut device, true).unwrap();
        let mut culling = CullResources::create(
            &mut device,
            &resources.prepared.draws,
            &resources.batches,
            BackendKind::Dx11,
            Some(&resources.meshes),
        )
        .unwrap();
        let shader = super::super::super::generated::create_shader(
            &mut device,
            BackendKind::Dx11,
            "cs_cull",
        )
        .unwrap();
        for (matrix, expected) in [
            (crate::Mat4::IDENTITY, [60, 0, 1, 0]),
            (
                crate::Transform {
                    translation: Vec3::new(0.0, 200.0, 0.0),
                    ..crate::Transform::IDENTITY
                }
                .matrix(),
                [0, 0, 0, 0],
            ),
            (crate::Mat4::IDENTITY, [60, 0, 1, 0]),
        ] {
            device
                .write_buffer(
                    resources.frame_buffer,
                    0,
                    &super::super::super::culling::tests::frame(matrix),
                )
                .unwrap();
            culling
                .dispatch(
                    &mut device,
                    shader,
                    resources.frame_buffer,
                    None,
                    resources.generated_arguments,
                )
                .unwrap();
            assert_eq!(
                super::super::super::generated::tests::counts(&mut device, culling.arguments, 16),
                expected
            );
        }
        culling.destroy(&mut device).unwrap();
        device.destroy_shader_module(shader).unwrap();
        let source = *resources.generated_mappings.keys().next().unwrap();
        let input = resources.generated_mappings[&source].buffer;
        let output = resources.generated_arguments.unwrap().0;
        let before = gfx_core::DiagnosticsDevice::resource_stats(&device).buffers;
        resources.sync_generated_draws(&mut device, true).unwrap();
        assert_eq!(
            gfx_core::DiagnosticsDevice::resource_stats(&device).buffers,
            before
        );
        assert_eq!(resources.generated_mappings[&source].buffer, input);
        device.destroy_buffer(output).unwrap();
        resources.generated_arguments = None;
        resources.sync_generated_draws(&mut device, true).unwrap();
        assert_eq!(resources.generated_mappings[&source].buffer, input);
        assert_ne!(resources.generated_arguments.unwrap().0, output);
        assert_eq!(
            resources.generated_mappings[&source].output,
            resources.generated_arguments.map(|(buffer, _)| buffer)
        );
        resources.batches.clear();
        resources.sync_generated_draws(&mut device, true).unwrap();
        assert!(resources.generated_mappings.is_empty());
        resources.destroy(&mut device).unwrap();
    }
}
