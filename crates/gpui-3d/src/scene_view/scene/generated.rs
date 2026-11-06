//! Materialize per-draw arguments from GPU-generated per-material face counts.
use super::*;
use gfx_core::{
    BufferBinding, ComputeBufferAccess, ComputeBufferBinding, ComputeDispatch, MemoryLocation,
};

pub(super) struct Mapping {
    pub buffer: BufferId,
    capacity: usize,
    entries: Vec<u32>,
    // Recycled staging vector for the next Scene/Section publication.
    // Keep the last committed entries immutable for dirty-range comparison.
    scratch_entries: Vec<u32>,
    output: Option<BufferId>,
}
struct Update {
    source: BufferId,
    source_size: usize,
    input: BufferId,
    entries: Vec<u32>,
    patches: Vec<MappingPatch>,
    /// Aligned read-only mapping windows; only these records are recomputed
    /// into the resident GPU indirect argument table.
    dispatch_ranges: Vec<MappingDispatchRange>,
}

/// One record-aligned dirty region in a persistent generated-draw mapping.
/// The output Argument Buffer may change without touching this source mapping.
struct MappingPatch {
    offset: u64,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MappingDispatchRange {
    offset: u64,
    size: u64,
}

/// Bind only the dirty portion of the resident mapping to the compute shader.
/// All four backends report their actual storage-buffer binding alignment; a
/// shader invocation starts at index 0 of the bound view, and map.y remains
/// the absolute output Draw index. Pages bigger than a single storage binding
/// are split into legal windows, including when the output buffer is rebuilt.
fn mapping_dispatch_ranges(
    patches: &[MappingPatch],
    mapping_bytes: usize,
    force_full: bool,
    limits: gfx_core::BufferBindingLimits,
) -> gpui::Result<Vec<MappingDispatchRange>> {
    if mapping_bytes == 0 || (!force_full && patches.is_empty()) {
        return Ok(Vec::new());
    }
    if !mapping_bytes.is_multiple_of(16)
        || limits.storage_offset_alignment == 0
        || limits.max_storage_binding_bytes == 0
    {
        return Err(anyhow!("generated mapping storage limits or record alignment are invalid").into());
    }
    let align = limits.storage_offset_alignment;
    let mut a = align;
    let mut b = 16u64;
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let offset_alignment = align.checked_div(a)
        .and_then(|value| value.checked_mul(16))
        .ok_or_else(|| anyhow!("generated mapping offset alignment overflow"))?;
    // A new binding must begin at a device-aligned, 16-byte record boundary.
    let max_window = limits.max_storage_binding_bytes
        / offset_alignment * offset_alignment;
    if max_window == 0 {
        return Err(anyhow!("generated mapping cannot bind a complete aligned record window").into());
    }
    let total = u64::try_from(mapping_bytes)
        .context("generated mapping byte length exceeds address range")?;
    let mut windows = Vec::<MappingDispatchRange>::new();
    let full = force_full || patches.len() > 16;
    if full {
        let mut start = 0u64;
        while start < total {
            let end = total.min(start + max_window);
            windows.push(MappingDispatchRange { offset: start, size: end - start });
            start = end;
        }
        return Ok(windows);
    }
    // Compute each sparse region at the nearest valid binding offset. A few
    // unchanged prefix records in the same alignment unit may be recomputed,
    // but no read crosses outside the live mapping.
    for patch in patches {
        let end = patch.offset.checked_add(patch.bytes.len() as u64)
            .ok_or_else(|| anyhow!("generated mapping dirty range overflow"))?;
        if end > total || !patch.offset.is_multiple_of(16) || !end.is_multiple_of(16) {
            return Err(anyhow!("generated mapping dirty range is not record aligned").into());
        }
        let mut start = patch.offset / offset_alignment * offset_alignment;
        while start < end {
            let stop = end.min(start + max_window);
            windows.push(MappingDispatchRange { offset: start, size: stop - start });
            start = stop;
        }
    }
    windows.sort_unstable_by_key(|range| range.offset);
    let mut merged = Vec::<MappingDispatchRange>::with_capacity(windows.len());
    for range in windows {
        if let Some(last) = merged.last_mut() {
            let end = last.offset + last.size;
            let other_end = range.offset + range.size;
            if range.offset <= end
                && other_end.max(end) - last.offset <= max_window
            {
                last.size = other_end.max(end) - last.offset;
                continue;
            }
        }
        merged.push(range);
    }
    // Dispatch setup and descriptor creation can cost more than useful GPU
    // work for highly fragmented deltas; fall back to a legal full sweep.
    let covered = merged.iter().map(|range| range.size).sum::<u64>();
    if merged.len() > 12 || covered.saturating_mul(2) >= total {
        merged.clear();
        let mut start = 0u64;
        while start < total {
            let end = total.min(start + max_window);
            merged.push(MappingDispatchRange { offset: start, size: end - start });
            start = end;
        }
    }
    Ok(merged)
}

/// Build bounded, record-aligned uploads without materializing the entire
/// logical mapping as a byte vector. Four u32 words describe each draw; a
/// changed output buffer does not invalidate unchanged GPU input mapping data.
fn mapping_patches(previous: &[u32], current: &[u32]) -> Vec<MappingPatch> {
    debug_assert!(current.len().is_multiple_of(4));
    // Directly compare groups of four u32 words, with no temporary comparison
    // tables. A bounded number of 16-byte aligned uploads is produced.
    super::super::uploads::changed_ranges_for_u32_records(
        previous, current, 4, 12, 2,
    ).into_iter().map(|range| {
        let mut bytes = Vec::with_capacity(range.len());
        for &word in &current[range.start / 4..range.end / 4] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        MappingPatch { offset: range.start as u64, bytes }
    }).collect()
}

/// The GPU count output is keyed by a stable physical slot under culling.
/// Reordering 16-byte mapping records still requires an input-buffer upload,
/// but must NOT recompute an unchanged output count. Resolve the previous
/// per-slot source once and mark only newly assigned/changed output targets.
///
/// Without GPU culling, output records are densely indexed and include the
/// logical first_instance. In that mode retain ordinary positional patches.
fn generated_output_dirty_patches(
    previous: &[u32],
    current: &[u32],
    physical_slots: bool,
) -> Vec<MappingPatch> {
    if !physical_slots {
        return mapping_patches(previous, current);
    }
    debug_assert!(previous.len().is_multiple_of(4));
    debug_assert!(current.len().is_multiple_of(4));
    let mut old = HashMap::<u32, [u32; 4]>::with_capacity(previous.len() / 4);
    for record in previous.chunks_exact(4) {
        let value = [record[0], record[1], record[2], record[3]];
        if old.insert(value[1], value).is_some() {
            // A corrupted non-unique output mapping cannot be safely
            // deduplicated by destination. Fall back to positional dispatch.
            return mapping_patches(previous, current);
        }
    }
    let mut seen = std::collections::HashSet::with_capacity(current.len() / 4);
    let mut patches = Vec::<MappingPatch>::new();
    for (index, record) in current.chunks_exact(4).enumerate() {
        let key = record[1];
        if !seen.insert(key) {
            return mapping_patches(previous, current);
        }
        if old.get(&key).is_some_and(|prior| prior.as_slice() == record) {
            continue;
        }
        let offset = index as u64 * 16;
        let mut bytes = Vec::with_capacity(16);
        for word in record {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        if let Some(previous) = patches.last_mut()
            && previous.offset + previous.bytes.len() as u64 == offset
        {
            previous.bytes.extend_from_slice(&bytes);
        } else {
            patches.push(MappingPatch { offset, bytes });
        }
    }
    patches
}

/// Immutable Generated Page metadata for one native DrawBatch. Produced
/// alongside BindGroups, so the subsequent GPU mapping pass reads resident
/// offsets instead of resolving MeshId and material parts a second time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BatchMappingSource {
    pub(super) part: MeshPart,
    pub(super) buffer: BufferId,
    pub(super) source_size: usize,
    pub(super) argument_part: u32,
}

impl BatchMappingSource {
    pub(super) fn for_draw(
        draw: &PreparedDraw,
        mesh: &MeshResources,
    ) -> gpui::Result<Option<Self>> {
        let Some(generated) = &mesh.generated else {
            return Ok(None);
        };
        let part = generated_part_index(draw.mesh.parts(), draw.part)
            .ok_or_else(|| anyhow!("missing generated material part"))?;
        let argument_part = generated.argument_base
            .checked_add(u32::try_from(part).context("generated material index overflow")?)
            .ok_or_else(|| anyhow!("generated argument page index overflow"))?;
        Ok(Some(Self {
            part: draw.part,
            buffer: generated.arguments,
            source_size: usize::try_from(generated.argument_page_bytes)
                .context("generated argument Page exceeds addressable host range")?,
            argument_part,
        }))
    }
}

/// Resolve the generated count's address independently of the dense native
/// Batch order. GPU culling uses the resident physical slot for the source
/// count; it writes the current logical first_instance into its own indirect
/// command every frame. Without GPU culling the generated output remains a
/// conventional densely indexed DrawIndirect table.
pub(super) fn generated_mapping_target(
    physical_slots: Option<&[u32]>,
    batch_index: usize,
    first_instance: usize,
) -> gpui::Result<(u32, u32)> {
    if let Some(slots) = physical_slots {
        let physical = slots.get(first_instance).copied()
            .ok_or_else(|| anyhow!("generated physical slot index outside culling map"))?;
        Ok((physical, 0))
    } else {
        Ok((
            u32::try_from(batch_index).context("generated batch index overflow")?,
            u32::try_from(first_instance).context("generated draw index overflow")?,
        ))
    }
}

/// Add one generated Section/part record during the existing native Draw
/// BindGroup traversal. Reuse the last committed Page's spare Vec, so
/// repeated streaming deltas don't allocate one new record array per Page.
pub(super) fn push_page_record(
    groups: &mut HashMap<BufferId, (usize, Vec<u32>)>,
    resident_mappings: &mut HashMap<BufferId, Mapping>,
    source: Option<BatchMappingSource>,
    output_slot: u32,
    first_instance: u32,
) -> gpui::Result<()> {
    let Some(source) = source else {
        return Ok(());
    };
    let (_, mapping) = match groups.entry(source.buffer) {
        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::hash_map::Entry::Vacant(entry) => {
            let mut scratch = resident_mappings
                .get_mut(&source.buffer)
                .map(|mapping| std::mem::take(&mut mapping.scratch_entries))
                .unwrap_or_default();
            scratch.clear();
            entry.insert((source.source_size, scratch))
        }
    };
    mapping.extend([
        source.argument_part,
        output_slot,
        first_instance,
        0,
    ]);
    Ok(())
}

/// Writing new GPU-generated Part Counts to an existing physical Page makes
/// previously computed indirect arguments stale even if every mapping word
/// and source buffer handle is bit-identical. Invalidating only that Page's
/// output forces a fresh GPU mapping dispatch without walking all Pages.
pub(super) fn invalidate_argument_page(
    mappings: &mut HashMap<BufferId, Mapping>,
    source: BufferId,
) {
    if let Some(mapping) = mappings.get_mut(&source) {
        mapping.output = None;
    }
}

/// Mesh parts are validated as contiguous, strictly increasing index
/// ranges at construction. Resolve the generated part in O(log N) rather than
/// scanning every material part again for each generated Draw batch.
fn generated_part_index(parts: &[MeshPart], target: MeshPart) -> Option<usize> {
    parts
        .binary_search_by_key(&target.first_index(), |part| part.first_index())
        .ok()
        .filter(|&index| parts[index] == target)
}

impl SceneResources {
    pub(super) fn sync_generated_draws(
        &mut self,
        device: &mut dyn ExtensionDevice,
        voxel_shader: bool,
    ) -> gpui::Result<()> {
        let mut groups = if let Some(ready) = self.generated_pending_groups.take() {
            // Normal streaming path: no second traversal of every Batch.
            ready
        } else {
            // Standalone compute tests and first-time resource initialization
            // still support the original, validated per-Batch fallback.
            let mut fallback = std::mem::take(&mut self.generated_group_scratch);
            fallback.clear();
            let resident_sources = self.generated_batch_sources.len() == self.batches.len();
            let physical_slots = self.culling.as_ref()
                .map(|culling| culling.committed_slot_layout().1);
            for (index, batch) in self.batches.iter().enumerate() {
                let source = if resident_sources {
                    self.generated_batch_sources[index]
                } else {
                    let draw = &self.prepared.draws[batch.first_instance];
                    let mesh = self.meshes.get(&mesh_key(draw, voxel_shader))
                        .ok_or_else(|| anyhow!("GPUI 3D mesh was not uploaded"))?;
                    BatchMappingSource::for_draw(draw, mesh)?
                };
                let (slot, first_instance) = generated_mapping_target(
                    physical_slots, index, batch.first_instance,
                )?;
                push_page_record(
                    &mut fallback, &mut self.generated_mappings,
                    source, slot, first_instance,
                )?;
            }
            fallback
        };
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
            self.generated_group_scratch = groups;
            return Ok(());
        }
        // The generated source table is addressed by resident physical slots
        // during GPU culling. Empty/recycled slots may extend past the active
        // dense batch count, so capacity follows the physical high-water mark.
        let source_slots = self.culling.as_ref().map_or(
            self.batches.len(),
            |culling| culling.committed_slot_layout().0,
        );
        let needed = source_slots.max(1).next_power_of_two();
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
        let binding_limits = device.buffer_binding_limits();
        for (source, (source_size, entries)) in groups.drain() {
            if self
                .generated_mappings
                .get(&source)
                .is_some_and(|mapping| mapping.entries == entries && mapping.output == Some(output))
            {
                // No upload or compute needed; retain the allocated work area
                // instead of dropping a Vec for each unchanged generated mesh.
                if let Some(mapping) = self.generated_mappings.get_mut(&source) {
                    mapping.scratch_entries = entries;
                }
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
                        scratch_entries: Vec::new(),
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
            let output_changed = self.generated_mappings[&source].output != Some(output);
            // The input mapping is positional, but the computed geometry
            // count is keyed by physical Draw Slot. A Section insertion can
            // shuffle many input records while leaving all survivor output
            // values unchanged. Keep these two dirty domains independent.
            let output_dirty = self.culling.as_ref().map(|_| {
                generated_output_dirty_patches(
                    &self.generated_mappings[&source].entries,
                    &entries,
                    true,
                )
            });
            // Removing a suffix leaves every surviving GPU argument intact.
            // Commit the shorter mapping without uploading or dispatching it.
            if patches.is_empty() && !output_changed {
                let mapping = self.generated_mappings.get_mut(&source)
                    .ok_or_else(|| anyhow!("generated mapping disappeared"))?;
                let old_entries = std::mem::replace(&mut mapping.entries, entries);
                mapping.scratch_entries = old_entries;
                continue;
            }
            let compute_dirty = output_dirty.as_deref().unwrap_or(&patches);
            let dispatch_ranges = mapping_dispatch_ranges(
                compute_dirty, entries.len() * 4, output_changed, binding_limits,
            )?;
            updates.push(Update {
                source,
                source_size,
                input,
                entries,
                patches,
                dispatch_ranges,
            });
        }
        self.generated_group_scratch = groups;
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
        let shader = self
            .mesh_draw_shader
            .ok_or_else(|| anyhow!("missing generated draw shader"))?;
        let compute_caps = device.compute_capabilities();
        let total_dispatches = updates.iter()
            .map(|update| update.dispatch_ranges.len())
            .sum::<usize>();
        let mut bindings = Vec::<Vec<ComputeBufferBinding>>::with_capacity(total_dispatches);
        let mut grids = Vec::with_capacity(total_dispatches);
        for update in &updates {
            for range in &update.dispatch_ranges {
                // Rebind only the aligned dirty mapping window. The compute
                // shader's arrayLength() and index are relative to this view,
                // while map.y targets the unchanged absolute output draw slot.
                let resources = [
                    (update.source, 0u64, update.source_size as u64,
                     ComputeBufferAccess::ReadOnly),
                    (update.input, range.offset, range.size,
                     ComputeBufferAccess::ReadOnly),
                    (output, 0u64, capacity as u64 * 16,
                     ComputeBufferAccess::ReadWrite),
                ];
                let mut group = Vec::with_capacity(resources.len());
                for (slot, (buffer, offset, size, access)) in resources.into_iter().enumerate() {
                    binding_limits.validate_range(offset, size, false)?;
                    group.push(ComputeBufferBinding {
                        binding: slot as u32,
                        buffer: BufferBinding {
                            buffer,
                            offset,
                            size,
                            stride: None,
                        },
                        access,
                    });
                }
                let threads = u32::try_from(range.size.div_ceil(16 * 64))
                    .context("generated draw dirty dispatch overflow")?;
                grids.push(compute_caps.plan_linear_dispatch_2d(threads)?);
                bindings.push(group);
            }
        }
        let dispatches = bindings.iter().zip(&grids)
            .map(|(bindings, &workgroups)| ComputeDispatch {
                shader,
                bindings,
                workgroups,
            })
            .collect::<Vec<_>>();
        // All input records may have moved while no generated count changed.
        // Such a structural Section reorder needs uploads but no compute.
        if !dispatches.is_empty() {
            device.dispatch_compute_batch(&dispatches)?;
        }
        for update in updates {
            let mapping = self
                .generated_mappings
                .get_mut(&update.source)
                .ok_or_else(|| anyhow!("generated draw mapping disappeared"))?;
            let old_entries = std::mem::replace(&mut mapping.entries, update.entries);
            // The formerly committed allocation becomes the next staging
            // buffer; no whole-table clone or allocator churn on updates.
            mapping.scratch_entries = old_entries;
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
        assert!(
            resources.generated_mappings[&source].scratch_entries.capacity() > 0,
            "unchanged generated mapping must retain its staging allocation",
        );
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
    use super::{mapping_patches, mapping_dispatch_ranges, generated_output_dirty_patches, MappingDispatchRange};
    use gfx_core::BufferBindingLimits;

    fn limits(alignment: u64, maximum: u64) -> BufferBindingLimits {
        BufferBindingLimits {
            storage_offset_alignment: alignment,
            max_storage_binding_bytes: maximum,
            ..BufferBindingLimits::default()
        }
    }

    fn dirty(offset: u64) -> super::MappingPatch {
        super::MappingPatch { offset, bytes: vec![0; 16] }
    }

    #[test]
    fn sparse_generated_mapping_only_dispatches_device_aligned_dirty_views() {
        let regions = mapping_dispatch_ranges(
            &[dirty(32), dirty(64_016)], 524_288, false, limits(256, 1 << 20),
        ).unwrap();
        assert_eq!(regions, [
            MappingDispatchRange { offset: 0, size: 48 },
            MappingDispatchRange { offset: 64_000, size: 32 },
        ]);
        assert!(regions.iter().map(|r| r.size).sum::<u64>() < 256);
    }

    #[test]
    fn output_replacement_dispatches_all_records_even_if_mapping_unchanged() {
        let regions = mapping_dispatch_ranges(
            &[], 65_536, true, limits(256, 1 << 20),
        ).unwrap();
        assert_eq!(regions, [MappingDispatchRange { offset: 0, size: 65_536 }]);
        assert!(mapping_dispatch_ranges(&[], 65_536, false, limits(256, 1 << 20))
            .unwrap().is_empty());
    }

    #[test]
    fn fragmented_changes_fall_back_to_full_compute() {
        let patches = (0..24u64).map(|x| dirty(x * 4096)).collect::<Vec<_>>();
        let regions = mapping_dispatch_ranges(
            &patches, 131_072, false, limits(256, 1 << 20),
        ).unwrap();
        assert_eq!(regions, [MappingDispatchRange { offset: 0, size: 131_072 }]);
    }

    #[test]
    fn regenerated_counts_force_refresh_even_with_unchanged_mapping() {
        let source = gfx_core::BufferId::from_parts(12, 1);
        let output = gfx_core::BufferId::from_parts(22, 1);
        let mut mappings = std::collections::HashMap::from([(
            source,
            super::Mapping {
                buffer: source,
                capacity: 64,
                entries: vec![7, 8, 9, 0],
                scratch_entries: Vec::new(),
                output: Some(output),
            },
        )]);
        super::invalidate_argument_page(&mut mappings, source);
        assert!(mappings[&source].output.is_none());
        assert_eq!(mappings[&source].entries, [7, 8, 9, 0]);
        let unknown = gfx_core::BufferId::from_parts(13, 1);
        super::invalidate_argument_page(&mut mappings, unknown);
        assert_eq!(mappings.len(), 1);
    }

    #[test]
    fn structural_mapping_reorder_uploads_input_without_recomputing_stable_counts() {
        let previous = vec![
            17, 5, 0, 0,
            21, 11, 0, 0,
            25, 29, 0, 0,
        ];
        let reordered = vec![
            25, 29, 0, 0,
            17, 5, 0, 0,
            21, 11, 0, 0,
        ];
        assert!(!mapping_patches(&previous, &reordered).is_empty());
        assert!(generated_output_dirty_patches(&previous, &reordered, true).is_empty());
        // A dense Batch-indexed source cannot safely skip this reorder.
        assert!(!generated_output_dirty_patches(&previous, &reordered, false).is_empty());
        let inserted = vec![
            25, 29, 0, 0,
            39, 9, 0, 0,
            17, 5, 0, 0,
            21, 11, 0, 0,
        ];
        let compute = generated_output_dirty_patches(&previous, &inserted, true);
        assert_eq!(compute.len(), 1);
        assert_eq!(compute[0].offset, 16);
        assert_eq!(compute[0].bytes.len(), 16);
        let windows = mapping_dispatch_ranges(
            &compute, inserted.len() * 4, false, limits(256, 1 << 20),
        ).unwrap();
        assert_eq!(windows, [
            MappingDispatchRange { offset: 0, size: 32 },
        ]);
    }

    #[test]
    fn generated_mapping_uses_stable_physical_counts_during_structural_shifts() {
        let slots = [9u32, 2, 17, 5];
        // A Section inserted at logical position zero does not require
        // rewriting any of the old physical count records.
        assert_eq!(
            super::generated_mapping_target(Some(&slots), 1, 1).unwrap(),
            (2, 0),
        );
        assert_eq!(
            super::generated_mapping_target(Some(&slots), 2, 2).unwrap(),
            (17, 0),
        );
        // Standalone/non-culling indirect draws still carry dense offsets
        // and the logical first-instance required by DrawIndirect.
        assert_eq!(
            super::generated_mapping_target(None, 2, 2).unwrap(),
            (2, 2),
        );
        assert!(super::generated_mapping_target(Some(&slots), 4, 8).is_err());
    }

    #[test]
    fn prebuilt_page_groups_preserve_native_draw_order_and_offsets() {
        let a = super::BatchMappingSource {
            part: crate::MeshPart::new(0, 6, 0),
            buffer: gfx_core::BufferId::from_parts(3, 1),
            source_size: 4096,
            argument_part: 17,
        };
        let b = super::BatchMappingSource {
            part: crate::MeshPart::new(6, 12, 1),
            buffer: gfx_core::BufferId::from_parts(4, 1),
            source_size: 8192,
            argument_part: 42,
        };
        let mut groups = std::collections::HashMap::new();
        let mut old = std::collections::HashMap::new();
        super::push_page_record(&mut groups, &mut old, Some(a), 2, 5).unwrap();
        super::push_page_record(&mut groups, &mut old, Some(b), 3, 6).unwrap();
        super::push_page_record(&mut groups, &mut old, Some(a), 7, 10).unwrap();
        super::push_page_record(&mut groups, &mut old, None, 8, 11).unwrap();
        assert_eq!(&groups[&a.buffer].1, &[17, 2, 5, 0, 17, 7, 10, 0]);
        assert_eq!(&groups[&b.buffer].1, &[42, 3, 6, 0]);
        assert_eq!(groups[&a.buffer].0, 4096);
        assert_eq!(groups[&b.buffer].0, 8192);
    }

    #[test]
    fn device_storage_binding_limit_splits_full_gpu_mapping() {
        let regions = mapping_dispatch_ranges(
            &[], 16_384, true, limits(256, 4096),
        ).unwrap();
        assert_eq!(regions, (0..4u64).map(|index|
            MappingDispatchRange { offset: index * 4096, size: 4096 }
        ).collect::<Vec<_>>());
    }

    #[test]
    fn changes_at_both_ends_cover_exact_dirty_draws() {
        let original = (0..16_384u32).collect::<Vec<_>>();
        let mut changed = original.clone();
        changed[4] ^= 42;
        changed[16_360] ^= 17;
        let patches = mapping_patches(&original, &changed);
        let regions = mapping_dispatch_ranges(
            &patches, changed.len() * 4, false, limits(256, 1 << 20),
        ).unwrap();
        let covers = |byte: u64| regions.iter().any(|range|
            byte >= range.offset && byte + 16 <= range.offset + range.size
        );
        assert!(covers(16));
        assert!(covers(65_440));
        assert!(regions.iter().map(|range| range.size).sum::<u64>()
            < changed.len() as u64 * 4 / 2);
    }

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

#[cfg(test)]
mod generated_part_lookup_tests {
    use super::*;

    #[test]
    fn binary_part_lookup_matches_linear_oracle_for_many_material_ranges() {
        let mut parts = Vec::new();
        let mut next_first = 0;
        for index in 0..512u32 {
            let count = (index % 9 + 1) * 6;
            parts.push(MeshPart::new(next_first, count, index % 16));
            next_first += count;
        }
        for (expected, &part) in parts.iter().enumerate() {
            assert_eq!(generated_part_index(&parts, part), Some(expected));
            assert_eq!(
                generated_part_index(&parts, part),
                parts.iter().position(|old| *old == part),
            );
        }
        let changed_material = MeshPart::new(
            parts[50].first_index(), parts[50].index_count(), 999,
        );
        assert_eq!(generated_part_index(&parts, changed_material), None);
        assert_eq!(generated_part_index(&parts, MeshPart::new(next_first, 6, 0)), None);
        assert_eq!(generated_part_index(&[], MeshPart::new(0, 6, 0)), None);
    }
}
