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
    patches: Vec<MappingPatch>,
}

/// One record-aligned dirty region in a persistent generated-draw mapping.
/// The output Argument Buffer may change without touching this source mapping.
struct MappingPatch {
    offset: u64,
    bytes: Vec<u8>,
}

/// Build bounded, record-aligned uploads without materializing the entire
/// logical mapping as a byte vector. Four u32 words describe each draw; a
/// changed output buffer does not invalidate unchanged GPU input mapping data.
fn mapping_patches(previous: &[u32], current: &[u32]) -> Vec<MappingPatch> {
    debug_assert!(current.len().is_multiple_of(4));
    // Compare full 16-byte mapping records, not their individual words.
    // Exact sequence equality is checked by the common dirty-range planner.
    // Coalesce short gaps; cap scatter uploads to avoid excessive driver calls.
    let previous_records = previous.chunks_exact(4).map(|r| [r[0], r[1], r[2], r[3]])
        .collect::<Vec<_>>();
    let current_records = current.chunks_exact(4).map(|r| [r[0], r[1], r[2], r[3]])
        .collect::<Vec<_>>();
    super::super::uploads::changed_ranges_for_keys(
        &previous_records,
        &current_records,
        previous.len() * 4,
        16,
        12,
        2,
    ).into_iter().map(|range| {
        let mut bytes = Vec::with_capacity(range.len());
        for &word in &current[range.start / 4..range.end / 4] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        MappingPatch { offset: range.start as u64, bytes }
    }).collect()
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
            // `output` can change on capacity growth even when the source
            // mapping is bit-for-bit identical. Re-dispatch the kernel but do
            // not re-upload the already resident input buffer in that case.
            let patches = mapping_patches(
                &self.generated_mappings[&source].entries,
                &entries,
            );
            updates.push(Update {
                source,
                source_size,
                input,
                entries,
                patches,
            });
        }
        if updates.is_empty() {
            return Ok(());
        }
        let writes = updates
            .iter()
            .flat_map(|update| {
                update.patches.iter().map(move |patch| gfx_core::BufferWrite {
                    descriptor: gfx_core::BufferWriteDescriptor {
                        buffer: update.input,
                        offset: patch.offset,
                    },
                    data: patch.bytes.as_slice(),
                })
            })
            .collect::<Vec<_>>();
        if !writes.is_empty() {
            device.write_buffer_batch(&writes)?;
        }
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
                        update.entries.len() * 4,
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

#[cfg(test)]
mod mapping_patch_tests {
    use super::mapping_patches;

    fn materialize(previous: &[u32], next: &[u32]) -> (Vec<u32>, usize) {
        let patches = mapping_patches(previous, next);
        let mut bytes = previous.iter().flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        bytes.resize(next.len() * 4, 0);
        for patch in &patches {
            let from = patch.offset as usize;
            bytes[from..from + patch.bytes.len()].copy_from_slice(&patch.bytes);
        }
        (
            bytes.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                .collect::<Vec<_>>(),
            patches.len(),
        )
    }

    #[test]
    fn output_reallocation_does_not_reupload_unchanged_input_mapping() {
        let map = [0, 3, 8, 0, 1, 9, 11, 0];
        let (bytes, writes) = materialize(&map, &map);
        assert_eq!(bytes, map);
        assert_eq!(writes, 0);
    }

    #[test]
    fn sparse_mapping_changes_upload_only_affected_draw_records() {
        let previous = (0..512u32).collect::<Vec<_>>();
        let mut updated = previous.clone();
        updated[40] ^= 0x20;
        updated[360] ^= 0x40;
        let patches = mapping_patches(&previous, &updated);
        assert_eq!(patches.len(), 2);
        assert_eq!(patches.iter().map(|p| p.bytes.len()).sum::<usize>(), 32);
        assert_eq!(materialize(&previous, &updated).0, updated);
    }

    #[test]
    fn appended_and_removed_mapping_entries_keep_exact_live_prefix() {
        let old = (0..48u32).collect::<Vec<_>>();
        let appended = (0..80u32).collect::<Vec<_>>();
        assert_eq!(materialize(&old, &appended).0, appended);
        let (truncated, writes) = materialize(&old, &old[..28]);
        assert_eq!(truncated, old[..28]);
        assert_eq!(writes, 0, "retired suffix does not need a GPU upload");
    }
}
