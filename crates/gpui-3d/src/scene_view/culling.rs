//! Resident batch bounds and compute-generated indirect arguments.

use super::{FRAME_PARAMS_STRIDE, gpu::{buffer_memory_location, dx11_streaming_headroom}, scene::DrawBatchRange};
use crate::{Aabb, PreparedDraw};
use anyhow::{Context as _, anyhow};
use gfx_core::{
    BackendKind, BufferBinding, BufferDescriptor, BufferId, BufferUsage, ComputeBufferAccess,
    ComputeBufferBinding, ComputeDispatch, MemoryLocation, ShaderModuleId,
};
use gpui::ExtensionDevice;
use super::scene::stable_slots::StableDrawSlots;
use std::{collections::hash_map::DefaultHasher, hash::{Hash, Hasher}};

const BOUNDS_STRIDE: usize = 48;
pub(super) const ARGUMENT_STRIDE: u64 = 16;


// The shader-compiled workgroup width is an ABI property, not a guessed
// per-GPU limit. Batches are stable in logical order; a contiguous trailing
// singleton range can be dispatched with one thread per draw. The leading
// segment (potentially containing real instancing) uses the original kernel.
// Avoid two dispatches if the tail cannot save at least one workgroup.
const SINGLETON_CULL_WORKGROUP_WIDTH: u32 = 64;

fn culling_dispatch_groups(count: u32, tail_start: u32, shader_available: bool) -> (u32, u32) {
    let tail_count = count.saturating_sub(tail_start);
    if shader_available && tail_count > SINGLETON_CULL_WORKGROUP_WIDTH {
        (tail_start, tail_count.div_ceil(SINGLETON_CULL_WORKGROUP_WIDTH))
    } else {
        (count, 0)
    }
}

/// Byte-accurate identity of a 48-byte GPU culling bounds record.
/// Converting floats to their bit patterns distinguishes signed zeros and
/// prevents a cache hit from silently changing the generated GPU bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct BoundsRecordKey([u32; 12]);

/// Exact 16-byte GPU culling batch descriptor. This remains identical when
/// unrelated Section deltas arrive, so the existing GPU buffer stays resident.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BatchRecordKey([u32; 4]);

impl BoundsRecordKey {
    fn from_draw(draw: &PreparedDraw) -> Self {
        let origin = draw.culling_origin;
        Self([
            draw.bounds.min.x.to_bits(),
            draw.bounds.min.y.to_bits(),
            draw.bounds.min.z.to_bits(),
            draw.part.index_count(),
            draw.bounds.max.x.to_bits(),
            draw.bounds.max.y.to_bits(),
            draw.bounds.max.z.to_bits(),
            (if draw.depth_bias != 0.0 || draw.pixel_offset != crate::Vec2::ZERO {
                1.0_f32
            } else {
                0.0_f32
            }).to_bits(),
            origin.map_or(0.0, |v| v.x).to_bits(),
            origin.map_or(0.0, |v| v.y).to_bits(),
            origin.map_or(0.0, |v| v.z).to_bits(),
            (if origin.is_some() { 1.0_f32 } else { 0.0_f32 }).to_bits(),
        ])
    }

    fn fingerprint(&self) -> u64 {
        let mut hash = DefaultHasher::new();
        self.hash(&mut hash);
        hash.finish()
    }
}

/// Use previously encoded records after section insertion/removal, while
/// preserving the original culling shader's exact packed record layout.
fn encode_bounds_cached(
    draws: &[PreparedDraw],
    previous_keys: &[BoundsRecordKey],
    previous_bytes: &[u8],
    next_keys: &mut Vec<BoundsRecordKey>,
    output: &mut Vec<u8>,
    run_scratch: &mut Vec<u8>,
) -> usize {
    next_keys.clear();
    next_keys.reserve(draws.len());
    next_keys.extend(draws.iter().map(BoundsRecordKey::from_draw));
    // Most section publications preserve every culling AABB byte. Avoid
    // reconstructing the full 64-byte-per-instance staging table: the
    // committed CPU Vec can be moved into the next snapshot after upload.
    if next_keys == previous_keys
        && previous_bytes.len() == draws.len() * BOUNDS_STRIDE
    {
        output.clear();
        return 0;
    }
    super::uploads::encode_cached_records(
        next_keys,
        previous_keys,
        previous_bytes,
        BOUNDS_STRIDE,
        output,
        BoundsRecordKey::fingerprint,
        |range, out| {
            encode_bounds_into(&draws[range], run_scratch);
            out.extend_from_slice(run_scratch);
            Ok::<(), std::convert::Infallible>(())
        },
    ).expect("bounds record encoder is infallible")
}

/// Resolve a Scene/Section delta into stable physical GPU storage addresses.
/// No old 48-byte Bounds row is moved; only a changed identity or a newly
/// allocated physical slot needs an upload.
fn plan_resident_bounds(
    previous: &StableDrawSlots,
    draws: &[PreparedDraw],
    keys: &mut Vec<Option<BoundsRecordKey>>,
) -> gpui::Result<(StableDrawSlots, Vec<u32>)> {
    let (next_slots, mapping) = previous.plan(draws)?;
    keys.clear();
    keys.resize(next_slots.high_water(), None);
    for (draw, &physical) in draws.iter().zip(&mapping) {
        keys[physical as usize] = Some(BoundsRecordKey::from_draw(draw));
    }
    Ok((next_slots, mapping))
}

/// Materialize only dirty 48-byte GPU pages from exact record-key bits.
/// Empty/retired slots are deliberately zeroed to prevent stale visibility.
fn resident_bounds_patches(
    previous: &[Option<BoundsRecordKey>],
    next: &[Option<BoundsRecordKey>],
) -> Vec<(u64, Vec<u8>)> {
    super::uploads::changed_ranges_for_keys(
        previous, next, previous.len() * BOUNDS_STRIDE, BOUNDS_STRIDE, 12, 3,
    ).into_iter().map(|range| {
        let mut bytes = vec![0u8; range.len()];
        for (row, key) in next[range.start / BOUNDS_STRIDE..range.end / BOUNDS_STRIDE]
            .iter().enumerate()
        {
            if let Some(key) = key {
                let start = row * BOUNDS_STRIDE;
                for (word, bits) in key.0.iter().enumerate() {
                    bytes[start + word * 4..start + word * 4 + 4]
                        .copy_from_slice(&bits.to_le_bytes());
                }
            }
        }
        (range.start as u64, bytes)
    }).collect()
}

/// Logical batch reorder affects only four-byte indirection entries.
fn resident_mapping_patches(previous: &[u32], next: &[u32]) -> Vec<(u64, Vec<u8>)> {
    super::uploads::changed_ranges_for_u32_records(previous, next, 1, 12, 3)
        .into_iter().map(|range| {
            let mut bytes = Vec::with_capacity(range.len());
            for &physical in &next[range.start / 4..range.end / 4] {
                bytes.extend_from_slice(&physical.to_le_bytes());
            }
            (range.start as u64, bytes)
        }).collect()
}

/// A fixed-layout Candidate Delta retains the exact mapping between logical
/// draws and resident physical GPU slots. Reject any inconsistent identity,
/// batch layout or resource-kind change and let the full planner handle it.
/// Only the dirty physical rows are encoded; no full BoundsRecordKey vector,
/// inverse map or CPU draw table is constructed.
fn sparse_resident_bounds_patches(
    draws: &[PreparedDraw],
    batches: &[DrawBatchRange],
    meshes: Option<&std::collections::HashMap<(crate::MeshId, bool), super::gpu::MeshResources>>,
    dirty: &[usize],
    slots: &StableDrawSlots,
    mapping: &[u32],
    bounds_keys: &[Option<BoundsRecordKey>],
    batch_keys: &[BatchRecordKey],
) -> Option<Vec<(usize, BoundsRecordKey, [u8; BOUNDS_STRIDE])>> {
    if draws.len() != mapping.len()
        || draws.len() != batches.len()
        || draws.len() != batch_keys.len()
        || bounds_keys.len() != slots.high_water()
    {
        return None;
    }
    let mut patches = Vec::with_capacity(dirty.len());
    let mut seen = std::collections::HashSet::with_capacity(dirty.len());
    for &logical in dirty {
        if !seen.insert(logical) {
            continue;
        }
        let draw = draws.get(logical)?;
        let batch = batches.get(logical)?;
        let physical = *mapping.get(logical)?;
        let slot = physical as usize;
        if batch.first_instance != logical || batch.instance_count != 1
            || !slots.matches_resident_draw(physical, draw)
        {
            return None;
        }
        let generated = meshes.is_some_and(|meshes| {
            meshes.get(&super::scene::mesh_key(draw, true))
                .is_some_and(|mesh| mesh.generated.is_some())
        });
        let old_batch = batch_keys.get(logical)?;
        if old_batch.0 != [
            u32::from(generated), 1, 0, u32::try_from(logical).ok()?,
        ] {
            return None;
        }
        let key = BoundsRecordKey::from_draw(draw);
        if bounds_keys.get(slot)? == &Some(key) {
            continue;
        }
        let mut bytes = [0u8; BOUNDS_STRIDE];
        for (word, value) in key.0.iter().enumerate() {
            bytes[word * 4..word * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        patches.push((slot, key, bytes));
    }
    Some(patches)
}

pub(super) struct CullResources {
    pub(super) hiz: super::hiz::HiZ,
    backend: BackendKind,
    bounds: BufferId,
    pub(super) batches: BufferId,
    range: BufferId,
    range_bounds: Option<(Option<Aabb>, bool, usize, usize)>,
    singleton_start: usize,
    last_singleton_report: Option<(usize, usize)>,
    pub(super) arguments: BufferId,
    pub(super) visible_instances: BufferId,
    pub(super) instance_capacity: usize,
    batch_capacity: usize,
    count: usize,
    instances: usize,
    uploaded_inputs: [Vec<u8>; 2],
    scratch_inputs: [Vec<u8>; 2],
    bounds_keys: Vec<Option<BoundsRecordKey>>,
    scratch_bounds_keys: Vec<Option<BoundsRecordKey>>,
    batch_keys: Vec<BatchRecordKey>,
    scratch_batch_keys: Vec<BatchRecordKey>,
    // Persistent per-Section draw addresses. Bounds stay at physical slots,
    // while this compact logical->physical map follows batch ordering.
    resident_slots: StableDrawSlots,
    slot_map: BufferId,
    uploaded_slot_map: Vec<u32>,
    // Commit only after the entire native upload batch succeeds.
    // The committed bounds-key table doubles as the authoritative CPU mirror;
    // full 48-byte bounds staging arrays are no longer needed.
}

impl CullResources {
    /// The culler has already planned and uploaded the stable Section address
    /// table. Reuse that same physical layout for DrawParams/Instances instead
    /// of hashing every Draw identity a second time in SceneResources.
    pub(super) fn committed_slot_layout(&self) -> (usize, &[u32]) {
        // Only the culler writes this resident snapshot. SceneResource's
        // encode pass borrows it and never needs a second full u32 Vec clone.
        (self.resident_slots.high_water(), &self.uploaded_slot_map)
    }

    pub(super) fn counts(&self) -> (usize, usize) {
        (self.count, self.instances)
    }
    pub(super) fn create(
        device: &mut dyn ExtensionDevice,
        draws: &[PreparedDraw],
        batches: &[DrawBatchRange],
        backend: BackendKind,
        meshes: Option<
            &std::collections::HashMap<(crate::MeshId, bool), super::gpu::MeshResources>,
        >,
    ) -> gpui::Result<Self> {
        // Keep the indirect-argument and visible-instance buffers on the
        // same DX11 growth policy as scene draw/instance buffers. Otherwise a
        // culling-buffer relocation would still invalidate every draw bind
        // group at 32K/64K, defeating the scene-buffer headroom.
        let instance_capacity = dx11_streaming_headroom(backend, draws.len())
            .max(64)
            .checked_next_power_of_two()
            .context("GPU instance capacity overflow")?;
        let batch_capacity = dx11_streaming_headroom(backend, batches.len())
            .max(64)
            .checked_next_power_of_two()
            .context("GPU batch capacity overflow")?;
        let buffers = create_buffers(
            device,
            instance_capacity,
            batch_capacity,
            buffer_memory_location(backend),
        )?;
        let mut resources = Self {
            hiz: match super::hiz::HiZ::create(device, backend) {
                Ok(hiz) => hiz,
                Err(error) => {
                    for buffer in buffers {
                        let _ = device.destroy_buffer(buffer);
                    }
                    return Err(error);
                }
            },
            backend,
            bounds: buffers[0],
            batches: buffers[1],
            arguments: buffers[2],
            visible_instances: buffers[3],
            range: buffers[4],
            slot_map: buffers[5],
            range_bounds: None,
            singleton_start: 0,
            last_singleton_report: None,
            count: 0,
            instances: 0,
            uploaded_inputs: Default::default(),
            scratch_inputs: Default::default(),
            bounds_keys: Vec::new(),
            scratch_bounds_keys: Vec::new(),
            batch_keys: Vec::new(),
            scratch_batch_keys: Vec::new(),
            resident_slots: StableDrawSlots::default(),
            uploaded_slot_map: Vec::new(),
            instance_capacity,
            batch_capacity,
        };
        if let Err(error) = resources.write_inputs(device, draws, batches, meshes) {
            resources.destroy(device)?;
            return Err(error);
        }
        Ok(resources)
    }

    pub(super) fn update(
        &mut self,
        device: &mut dyn ExtensionDevice,
        draws: &[PreparedDraw],
        batches: &[DrawBatchRange],
        meshes: Option<
            &std::collections::HashMap<(crate::MeshId, bool), super::gpu::MeshResources>,
        >,
    ) -> gpui::Result<()> {
        if draws.len() <= self.instance_capacity
            && self.resident_slots.high_water() <= self.instance_capacity
            && batches.len() <= self.batch_capacity {
            self.write_inputs(device, draws, batches, meshes)?;
            return Ok(());
        }
        let next = Self::create(device, draws, batches, self.backend, meshes)?;
        let previous = std::mem::replace(self, next);
        previous.destroy(device)?;
        Ok(())
    }

    /// Fast path for opaque singleton Section edits. Candidate ordering,
    /// batch descriptors and stable physical slots must remain identical.
    /// Camera visibility still runs on the GPU after these sparse writes.
    /// The committed CPU mirror changes only after the GPU upload succeeds.
    pub(super) fn update_dirty_opaque(
        &mut self,
        device: &mut dyn ExtensionDevice,
        draws: &[PreparedDraw],
        batches: &[DrawBatchRange],
        meshes: Option<&std::collections::HashMap<(crate::MeshId, bool), super::gpu::MeshResources>>,
        dirty: &[usize],
    ) -> gpui::Result<bool> {
        if self.count != draws.len()
            || self.instances != draws.len()
            || self.singleton_start != 0
        {
            return Ok(false);
        }
        let Some(patches) = sparse_resident_bounds_patches(
            draws, batches, meshes, dirty,
            &self.resident_slots, &self.uploaded_slot_map,
            &self.bounds_keys, &self.batch_keys,
        ) else {
            return Ok(false);
        };
        if patches.is_empty() {
            return Ok(true);
        }
        let writes = patches.iter().map(|(slot, _, bytes)| gfx_core::BufferWrite {
            descriptor: gfx_core::BufferWriteDescriptor {
                buffer: self.bounds,
                offset: *slot as u64 * BOUNDS_STRIDE as u64,
            },
            data: &bytes[..],
        }).collect::<Vec<_>>();
        device.write_buffer_batch(&writes)?;
        // BufferWrite descriptors borrow the patch byte arrays. Release
        // those borrows before moving the patches into the committed mirror.
        drop(writes);
        for (slot, key, _) in patches {
            self.bounds_keys[slot] = Some(key);
        }
        Ok(true)
    }

    fn write_inputs(
        &mut self,
        device: &mut dyn ExtensionDevice,
        draws: &[PreparedDraw],
        batches: &[DrawBatchRange],
        meshes: Option<
            &std::collections::HashMap<(crate::MeshId, bool), super::gpu::MeshResources>,
        >,
    ) -> gpui::Result<()> {
        // The shared stable-slot identity is (Node, Mesh, Material, Part,
        // occurrence). A Section deletion retires only its corresponding slot;
        // surviving bounds stay resident at their previous GPU addresses.
        let mut next_bounds_keys = std::mem::take(&mut self.scratch_bounds_keys);
        let (next_slots, logical_to_physical) =
            plan_resident_bounds(&self.resident_slots, draws, &mut next_bounds_keys)?;
        if next_slots.high_water() > self.instance_capacity {
            return Err(anyhow!("GPU culling physical slot capacity exceeded").into());
        }
        let mut next_batch_keys = std::mem::take(&mut self.scratch_batch_keys);
        let [mut bounds_scratch, mut batch_bytes] = std::mem::take(&mut self.scratch_inputs);
        if let Err(error) = encode_batches_cached(
            draws, batches, meshes,
            &self.batch_keys, &self.uploaded_inputs[1],
            &mut next_batch_keys, &mut batch_bytes,
        ) {
            self.scratch_bounds_keys = next_bounds_keys;
            self.scratch_batch_keys = next_batch_keys;
            self.scratch_inputs = [bounds_scratch, batch_bytes];
            return Err(error);
        }

        // Generate bounded dirty regions from 48-byte record identities.
        // Materialize ONLY these regions, never the entire resident bounds
        // table. A removed physical slot is zeroed and reused transactionally.
        let bounds_patches = resident_bounds_patches(&self.bounds_keys, &next_bounds_keys);

        // The compact map is the only data affected by logical reordering.
        let mapping_patches = resident_mapping_patches(
            &self.uploaded_slot_map, &logical_to_physical,
        );

        let mut writes = Vec::with_capacity(bounds_patches.len() + mapping_patches.len() + 12);
        for (offset, data) in &bounds_patches {
            writes.push(gfx_core::BufferWrite {
                descriptor: gfx_core::BufferWriteDescriptor {
                    buffer: self.bounds, offset: *offset,
                },
                data,
            });
        }
        for (offset, data) in &mapping_patches {
            writes.push(gfx_core::BufferWrite {
                descriptor: gfx_core::BufferWriteDescriptor {
                    buffer: self.slot_map, offset: *offset,
                },
                data,
            });
        }
        for range in super::uploads::changed_ranges_for_keys(
            &self.batch_keys, &next_batch_keys,
            self.uploaded_inputs[1].len(), ARGUMENT_STRIDE as usize, 12, 4,
        ) {
            writes.push(gfx_core::BufferWrite {
                descriptor: gfx_core::BufferWriteDescriptor {
                    buffer: self.batches, offset: range.start as u64,
                },
                data: &batch_bytes[range],
            });
        }
        let write_result = if writes.is_empty() {
            Ok(())
        } else {
            device.write_buffer_batch(&writes).map(|_| ())
        };
        if let Err(error) = write_result {
            // Keep committed residency and CPU mirrors unchanged on upload
            // failure; the next Scene publication can safely retry every delta.
            self.scratch_bounds_keys = next_bounds_keys;
            self.scratch_batch_keys = next_batch_keys;
            bounds_scratch.clear();
            self.scratch_inputs = [bounds_scratch, batch_bytes];
            return Err(error.into());
        }

        self.resident_slots = next_slots;
        self.uploaded_slot_map = logical_to_physical;
        std::mem::swap(&mut self.bounds_keys, &mut next_bounds_keys);
        next_bounds_keys.clear();
        self.scratch_bounds_keys = next_bounds_keys;
        std::mem::swap(&mut self.batch_keys, &mut next_batch_keys);
        next_batch_keys.clear();
        self.scratch_batch_keys = next_batch_keys;
        let mut old = std::mem::replace(&mut self.uploaded_inputs[1], batch_bytes);
        if self.uploaded_inputs[1].is_empty()
            && old.len() == batches.len() * ARGUMENT_STRIDE as usize
        {
            self.uploaded_inputs[1] = std::mem::take(&mut old);
        }
        old.clear();
        bounds_scratch.clear();
        self.scratch_inputs = [bounds_scratch, old];
        self.count = batches.len();
        self.instances = draws.len();
        // Batching emits all Quad keys before Singleton keys. The last
        // multi-instance range marks the beginning of the largest contiguous
        // singleton tail. The prefix may also contain singleton batches, but
        // the tail *never* contains instancing, so no cross-Section results
        // or indirect instance addresses need to be rearranged.
        self.singleton_start = batches.iter()
            .rposition(|batch| batch.instance_count > 1)
            .map_or(0, |index| index + 1);
        Ok(())
    }

    // Legacy native test fixtures exercise the original one-workgroup-per-
    // batch entry point. Production calls the adaptive selection below.
    #[cfg(test)]
    pub(super) fn dispatch(
        &mut self,
        device: &mut dyn ExtensionDevice,
        shader: ShaderModuleId,
        frame: BufferId,
        range: Option<Aabb>,
        generated: Option<(BufferId, usize)>,
    ) -> gpui::Result<()> {
        self.dispatch_optimized(device, shader, None, frame, range, generated)
    }

    pub(super) fn dispatch_optimized(
        &mut self,
        device: &mut dyn ExtensionDevice,
        shader: ShaderModuleId,
        singleton_shader: Option<ShaderModuleId>,
        frame: BufferId,
        range: Option<Aabb>,
        generated: Option<(BufferId, usize)>,
    ) -> gpui::Result<()> {
        if self.count == 0 {
            return Ok(());
        }
        self.hiz.dispatch(device, frame)?;
        let (min, max) = range.map_or(([-f32::MAX; 3], [f32::MAX; 3]), |range| {
            (
                [range.min.x, range.min.y, range.min.z],
                [range.max.x, range.max.y, range.max.z],
            )
        });
        let occlusion_enabled = self.hiz.has_occluders();
        let count = u32::try_from(self.count).context("GPU culling batch count exceeds u32")?;
        let tail_start = u32::try_from(self.singleton_start)
            .context("GPU culling singleton tail exceeds u32")?;
        let (normal_groups, singleton_groups) = culling_dispatch_groups(
            count, tail_start, singleton_shader.is_some(),
        );
        let compact_start = if singleton_groups == 0 { count } else { tail_start };
        let mut range_bytes = [0; 48];
        for (chunk, value) in range_bytes
            .chunks_exact_mut(4)
            .zip([min[0], min[1], min[2], if occlusion_enabled { 1.0 } else { 0.0 }, max[0], max[1], max[2], 0.0])
        {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        for (bytes, value) in range_bytes[32..48].chunks_exact_mut(4)
            .zip([count, compact_start, 0, 0])
        {
            bytes.copy_from_slice(&value.to_le_bytes());
        }
        if self.range_bounds != Some((range, occlusion_enabled, self.count, compact_start as usize)) {
            device.write_buffer(self.range, 0, &range_bytes)?;
            self.range_bounds = Some((range, occlusion_enabled, self.count, compact_start as usize));
        }
        let bindings = [
            compute_binding(
                0,
                frame,
                FRAME_PARAMS_STRIDE as u64,
                ComputeBufferAccess::Uniform,
            ),
            compute_binding(
                1,
                self.bounds,
                (self.resident_slots.high_water().max(1) * BOUNDS_STRIDE) as u64,
                ComputeBufferAccess::ReadOnly,
            ),
            compute_binding(
                2,
                self.arguments,
                self.count as u64 * ARGUMENT_STRIDE,
                ComputeBufferAccess::ReadWrite,
            ),
            compute_binding(
                3,
                self.visible_instances,
                self.instances as u64 * 4,
                ComputeBufferAccess::ReadWrite,
            ),
            compute_binding(
                4,
                self.batches,
                self.count as u64 * ARGUMENT_STRIDE,
                ComputeBufferAccess::ReadOnly,
            ),
            compute_binding(5, self.range, 48, ComputeBufferAccess::Uniform),
            compute_binding(
                6,
                generated.map_or(self.batches, |(buffer, _)| buffer),
                generated.map_or(self.count as u64 * ARGUMENT_STRIDE, |(_, capacity)| {
                    capacity as u64 * ARGUMENT_STRIDE
                }),
                ComputeBufferAccess::ReadOnly,
            ),
            compute_binding(
                7,
                self.hiz.depths,
                (super::hiz::WORDS * 4) as u64,
                ComputeBufferAccess::ReadOnly,
            ),
            compute_binding(
                8,
                self.slot_map,
                (self.instances.max(1) * 4) as u64,
                ComputeBufferAccess::ReadOnly,
            ),
        ];
        // When Quad instancing and GeneratedMesh singletons coexist, use
        // the normal kernel only for the prefix containing all non-singletons,
        // then use 64 independent lanes for the contiguous singleton tail.
        // Both dispatches share the *same* range uniform and indirect buffer,
        // so no CPU readback, binding rebuild, or draw-record permutation.
        let total_groups = normal_groups.saturating_add(singleton_groups);
        if super::renderer::trace_timings()
            && self.last_singleton_report != Some((normal_groups as usize, singleton_groups as usize))
        {
            eprintln!(
                "gpui3d cull_mode singleton_compaction={} batches={} normal_workgroups={} singleton_workgroups={} workgroups={} saved_workgroups={} tail_start={}",
                singleton_groups != 0, count, normal_groups, singleton_groups,
                total_groups, count.saturating_sub(total_groups), compact_start,
            );
            self.last_singleton_report =
                Some((normal_groups as usize, singleton_groups as usize));
        }
        let capabilities = device.compute_capabilities();
        match (normal_groups, singleton_groups) {
            (0, 0) => {}
            (normal, 0) => {
                let grid = capabilities.plan_linear_dispatch_2d(normal)?;
                device.dispatch_compute(&ComputeDispatch {
                    shader,
                    bindings: &bindings,
                    workgroups: grid,
                })?;
            }
            (0, singleton) => {
                let grid = capabilities.plan_linear_dispatch_2d(singleton)?;
                device.dispatch_compute(&ComputeDispatch {
                    shader: singleton_shader.expect("singleton kernel exists for selected path"),
                    bindings: &bindings,
                    workgroups: grid,
                })?;
            }
            (normal, singleton) => {
                // Vulkan and DX12 co-record both kernels into a single native
                // batch instead of paying two compute submissions per frame.
                // The original and singleton kernels write disjoint indirect
                // argument ranges, so neither dispatch depends on the other.
                let dispatches = [
                    ComputeDispatch {
                        shader,
                        bindings: &bindings,
                        workgroups: capabilities.plan_linear_dispatch_2d(normal)?,
                    },
                    ComputeDispatch {
                        shader: singleton_shader.expect("singleton kernel exists for selected path"),
                        bindings: &bindings,
                        workgroups: capabilities.plan_linear_dispatch_2d(singleton)?,
                    },
                ];
                device.dispatch_compute_batch(&dispatches)?;
            }
        }
        Ok(())
    }

    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        self.hiz.destroy(device)?;
        device.destroy_buffer(self.bounds)?;
        device.destroy_buffer(self.batches)?;
        device.destroy_buffer(self.arguments)?;
        device.destroy_buffer(self.visible_instances)?;
        device.destroy_buffer(self.range)?;
        device.destroy_buffer(self.slot_map)?;
        Ok(())
    }
}

#[cfg(test)]
mod singleton_dispatch_tests {
    use super::*;

    #[test]
    fn generated_singletons_reduce_dispatch_groups_without_reordering() {
        assert_eq!(culling_dispatch_groups(65_993, 0, true), (0, 1032));
        assert_eq!(culling_dispatch_groups(65_993, 65_993, true), (65_993, 0));
        assert_eq!(culling_dispatch_groups(65_993, 0, false), (65_993, 0));
        assert_eq!(culling_dispatch_groups(1, 0, true), (1, 0));
        assert_eq!(culling_dispatch_groups(64, 0, true), (64, 0));
        assert_eq!(culling_dispatch_groups(65, 0, true), (0, 2));
    }

    #[test]
    fn mixed_quad_instancing_and_generated_sections_keep_both_dispatch_paths() {
        assert_eq!(culling_dispatch_groups(65_993, 96, true), (96, 1030));
        assert_eq!(culling_dispatch_groups(65_993, 96, false), (65_993, 0));
        assert_eq!(culling_dispatch_groups(200, 180, true), (200, 0));
        assert_eq!(culling_dispatch_groups(200, 135, true), (135, 2));
        assert_eq!(culling_dispatch_groups(200, 200, true), (200, 0));
    }
}

#[cfg(test)]
fn encode_bounds(draws: &[PreparedDraw]) -> Vec<u8> {
    let mut bytes = Vec::new();
    encode_bounds_into(draws, &mut bytes);
    bytes
}

fn encode_bounds_into(draws: &[PreparedDraw], bounds: &mut Vec<u8>) {
    bounds.clear();
    bounds.reserve(draws.len() * BOUNDS_STRIDE);
    for draw in draws {
        for value in [draw.bounds.min.x, draw.bounds.min.y, draw.bounds.min.z] {
            bounds.extend_from_slice(&value.to_le_bytes());
        }
        bounds.extend_from_slice(&draw.part.index_count().to_le_bytes());
        for value in [
            draw.bounds.max.x,
            draw.bounds.max.y,
            draw.bounds.max.z,
            if draw.depth_bias != 0.0 || draw.pixel_offset != crate::Vec2::ZERO {
                1.0
            } else {
                0.0
            },
        ] {
            bounds.extend_from_slice(&value.to_le_bytes());
        }
        let origin = draw.culling_origin;
        for value in [
            origin.map_or(0.0, |value| value.x),
            origin.map_or(0.0, |value| value.y),
            origin.map_or(0.0, |value| value.z),
            if origin.is_some() { 1.0 } else { 0.0 },
        ] {
            bounds.extend_from_slice(&value.to_le_bytes());
        }
    }
}

fn create_buffers(
    device: &mut dyn ExtensionDevice,
    instances: usize,
    batches: usize,
    memory_location: MemoryLocation,
) -> gpui::Result<[BufferId; 6]> {
    let read = BufferUsage::STORAGE | BufferUsage::COPY_DST;
    let write = BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE;
    let specs = [
        (instances as u64 * BOUNDS_STRIDE as u64, read),
        (batches as u64 * ARGUMENT_STRIDE, read),
        (
            batches as u64 * ARGUMENT_STRIDE,
            write | BufferUsage::INDIRECT,
        ),
        (instances as u64 * 4, write),
        (48, BufferUsage::UNIFORM | BufferUsage::COPY_DST),
        (instances as u64 * 4, read),
    ];
    let mut buffers = Vec::with_capacity(6);
    for (size, usage) in specs {
        match device.create_buffer(&BufferDescriptor {
            label: Some("gpui-3d GPU culling".into()),
            size,
            usage,
            memory_location: if usage.contains(BufferUsage::UNIFORM) {
                memory_location
            } else {
                MemoryLocation::GpuOnly
            },
        }) {
            Ok(buffer) => buffers.push(buffer),
            Err(error) => {
                for buffer in buffers {
                    device.destroy_buffer(buffer)?;
                }
                return Err(error.into());
            }
        }
    }
    buffers
        .try_into()
        .map_err(|_| anyhow!("GPU culling buffer count mismatch"))
}

fn compute_binding(
    binding: u32,
    buffer: BufferId,
    size: u64,
    access: ComputeBufferAccess,
) -> ComputeBufferBinding {
    ComputeBufferBinding {
        binding,
        buffer: BufferBinding {
            buffer,
            offset: 0,
            size,
            stride: None,
        },
        access,
    }
}

/// Resolve the logical culling batch metadata once, with no intermediate
/// 16-byte-per-batch staging array.
fn batch_record_keys(
    draws: &[PreparedDraw],
    batches: &[DrawBatchRange],
    meshes: Option<&std::collections::HashMap<(crate::MeshId, bool), super::gpu::MeshResources>>,
    keys: &mut Vec<BatchRecordKey>,
) -> gpui::Result<()> {
    keys.clear();
    keys.reserve(batches.len());
    for batch in batches {
        let end = batch
            .first_instance
            .checked_add(batch.instance_count)
            .ok_or_else(|| anyhow!("GPU batch range overflow"))?;
        draws
            .get(batch.first_instance..end)
            .filter(|draws| !draws.is_empty())
            .ok_or_else(|| anyhow!("GPU batch references invalid draw range"))?;
        let generated = meshes.is_some_and(|meshes| {
            meshes
                .get(&super::scene::mesh_key(&draws[batch.first_instance], true))
                .is_some_and(|mesh| mesh.generated.is_some())
        });
        if generated && batch.instance_count != 1 {
            return Err(anyhow!("generated mesh culling requires singleton batches"));
        }
        keys.push(BatchRecordKey([
            u32::from(generated),
            u32::try_from(batch.instance_count)?,
            0,
            u32::try_from(batch.first_instance)?,
        ]));
    }
    Ok(())
}

/// Preserve the previous GPU Batch table and patch only records affected by
/// Section addition/removal or generated-mesh changes. An empty output when
/// keys/extent match is a sentinel for reusing the already committed buffer.
fn encode_batches_cached(
    draws: &[PreparedDraw],
    batches: &[DrawBatchRange],
    meshes: Option<&std::collections::HashMap<(crate::MeshId, bool), super::gpu::MeshResources>>,
    previous: &[BatchRecordKey],
    previous_bytes: &[u8],
    next: &mut Vec<BatchRecordKey>,
    output: &mut Vec<u8>,
) -> gpui::Result<usize> {
    batch_record_keys(draws, batches, meshes, next)?;
    let size = batches.len()
        .checked_mul(ARGUMENT_STRIDE as usize)
        .ok_or_else(|| anyhow!("GPU batch descriptor size overflow"))?;
    // One sequential comparison finds the first genuinely dirty batch row.
    // Previous code separately counted all changed records, compared both
    // entire key vectors, then walked them a third time to encode changes.
    // On a stable 100K+ Section scene this returns without copying the
    // resident CPU snapshot; on deltas it visits no unaffected prefix twice.
    let first_dirty = next.iter().enumerate().position(|(index, key)| {
        previous.get(index) != Some(key)
            || previous_bytes.len() < (index + 1) * ARGUMENT_STRIDE as usize
    });
    output.clear();
    if first_dirty.is_none()
        && previous.len() == next.len()
        && previous_bytes.len() == size
    {
        return Ok(0);
    }
    output.extend_from_slice(previous_bytes);
    output.resize(size, 0);
    let mut dirty = 0;
    for (index, key) in next.iter().enumerate().skip(first_dirty.unwrap_or(next.len())) {
        if previous.get(index) != Some(key) {
            dirty += 1;
        } else if previous_bytes.len() >= (index + 1) * ARGUMENT_STRIDE as usize {
            continue;
        }
        let start = index * ARGUMENT_STRIDE as usize;
        for (word, value) in key.0.iter().enumerate() {
            output[start + word * 4..start + word * 4 + 4]
                .copy_from_slice(&value.to_le_bytes());
        }
    }
    Ok(dirty)
}

#[cfg(test)]
fn encode_batches_into(
    draws: &[PreparedDraw],
    batches: &[DrawBatchRange],
    meshes: Option<&std::collections::HashMap<(crate::MeshId, bool), super::gpu::MeshResources>>,
    bytes: &mut Vec<u8>,
) -> gpui::Result<()> {
    let mut keys = Vec::new();
    batch_record_keys(draws, batches, meshes, &mut keys)?;
    bytes.clear();
    bytes.reserve(keys.len() * ARGUMENT_STRIDE as usize);
    for key in keys {
        for word in key.0 {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    Ok(())
}

#[cfg(all(test, target_os = "windows"))]
#[path = "culling_tests.rs"]
pub(super) mod tests;


#[cfg(test)]
mod bounds_cache_tests {
    use super::*;
    use crate::{Camera, Mesh, Node, Scene, Transform, Vec3};
    use std::sync::Arc;

    fn sample_draws() -> Vec<PreparedDraw> {
        let mut scene = Scene::new();
        let mesh = Arc::new(Mesh::cube());
        for index in 0..16 {
            scene.insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_transform(Transform {
                        translation: Vec3::new(index as f32 * 0.6, 0.0, 0.0),
                        ..Transform::IDENTITY
                    }),
            ).unwrap();
        }
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 40.0), Vec3::ZERO, Vec3::Y, 1.0, 0.1, 100.0,
        );
        crate::PreparedScene::new(&scene, camera, 1.0).unwrap().draws
    }

    #[test]
    fn cached_bounds_match_canonical_encoder_on_shift_and_edit() {
        let draws = sample_draws();
        assert!(draws.len() >= 8);
        let mut old_keys = Vec::new();
        let mut next_keys = Vec::new();
        let mut encoded = Vec::new();
        let mut scratch = Vec::new();
        let first_count = encode_bounds_cached(
            &draws, &old_keys, &[], &mut next_keys, &mut encoded, &mut scratch,
        );
        assert_eq!(first_count, draws.len());
        assert_eq!(encoded, encode_bounds(&draws));

        let old_bytes = encoded.clone();
        let preserved_keys = next_keys.clone();
        let unchanged = encode_bounds_cached(
            &draws, &preserved_keys, &old_bytes,
            &mut next_keys, &mut encoded, &mut scratch,
        );
        assert_eq!(unchanged, 0);
        assert!(encoded.is_empty(),
            "unchanged bounds should reuse resident CPU staging without a full copy");
        assert_eq!(next_keys, preserved_keys);
        std::mem::swap(&mut old_keys, &mut next_keys);
        let mut shifted = draws.clone();
        shifted.remove(2);
        let count = encode_bounds_cached(
            &shifted, &old_keys, &old_bytes, &mut next_keys, &mut encoded, &mut scratch,
        );
        assert_eq!(count, 0, "shifted unchanged bounds should reuse resident bytes");
        assert_eq!(encoded, encode_bounds(&shifted));

        let shifted_bytes = encoded.clone();
        std::mem::swap(&mut old_keys, &mut next_keys);
        shifted[3].bounds.max.x += 0.5;
        let count = encode_bounds_cached(
            &shifted, &old_keys, &shifted_bytes, &mut next_keys, &mut encoded, &mut scratch,
        );
        assert_eq!(count, 1);
        assert_eq!(encoded, encode_bounds(&shifted));
    }

    #[test]
    fn bitwise_key_preserves_culling_record_bytes() {
        let draws = sample_draws();
        for draw in &draws {
            let key = BoundsRecordKey::from_draw(draw);
            let encoded_key = key.0.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>();
            assert_eq!(encoded_key, encode_bounds(std::slice::from_ref(draw)));
        }
    }
    #[test]
    fn incremental_batch_upload_only_encodes_changed_records() {
        let draws = sample_draws();
        let batches = (0..draws.len()).map(|i| DrawBatchRange {
            first_instance: i,
            instance_count: 1,
        }).collect::<Vec<_>>();
        let mut old_keys = Vec::new();
        let mut keys = Vec::new();
        let mut bytes = Vec::new();
        let mut reference = Vec::new();
        let first = encode_batches_cached(
            &draws, &batches, None, &[], &[],
            &mut keys, &mut bytes,
        ).unwrap();
        assert_eq!(first, batches.len());
        encode_batches_into(&draws, &batches, None, &mut reference).unwrap();
        assert_eq!(bytes, reference);
        let previous_bytes = bytes.clone();
        std::mem::swap(&mut old_keys, &mut keys);
        let identical = encode_batches_cached(
            &draws, &batches, None, &old_keys, &previous_bytes,
            &mut keys, &mut bytes,
        ).unwrap();
        assert_eq!(identical, 0);
        assert!(bytes.is_empty(), "unchanged batch input must reuse GPU-resident data");

        let mut appended = batches.clone();
        appended.push(DrawBatchRange { first_instance: 0, instance_count: 1 });
        let appended_count = encode_batches_cached(
            &draws, &appended, None, &old_keys, &previous_bytes,
            &mut keys, &mut bytes,
        ).unwrap();
        assert_eq!(appended_count, 1);
        encode_batches_into(&draws, &appended, None, &mut reference).unwrap();
        assert_eq!(bytes, reference);
        let ranges = super::super::uploads::changed_ranges_for_keys(
            &old_keys, &keys, previous_bytes.len(),
            ARGUMENT_STRIDE as usize, 12, 4,
        );
        assert_eq!(ranges, vec![previous_bytes.len()..reference.len()]);

        let removed = &batches[..batches.len() - 3];
        let removed_count = encode_batches_cached(
            &draws, removed, None, &old_keys, &previous_bytes,
            &mut keys, &mut bytes,
        ).unwrap();
        assert_eq!(removed_count, 0);
        encode_batches_into(&draws, removed, None, &mut reference).unwrap();
        assert_eq!(bytes, reference);
    }

    #[test]
    fn middle_section_batch_delta_encodes_only_changed_descriptor() {
        let draws = sample_draws();
        let batches = (0..draws.len()).map(|index| DrawBatchRange {
            first_instance: index,
            instance_count: 1,
        }).collect::<Vec<_>>();
        let mut keys = Vec::new();
        let mut encoded = Vec::new();
        encode_batches_cached(
            &draws, &batches, None, &[], &[], &mut keys, &mut encoded,
        ).unwrap();
        let prior_keys = keys.clone();
        let prior_bytes = encoded.clone();
        let mut reordered = batches.clone();
        reordered[8].first_instance = 3;
        let changed = encode_batches_cached(
            &draws, &reordered, None, &prior_keys, &prior_bytes,
            &mut keys, &mut encoded,
        ).unwrap();
        assert_eq!(changed, 1);
        let mut reference = Vec::new();
        encode_batches_into(&draws, &reordered, None, &mut reference).unwrap();
        assert_eq!(encoded, reference);
        assert_eq!(&encoded[..8 * ARGUMENT_STRIDE as usize],
                   &prior_bytes[..8 * ARGUMENT_STRIDE as usize]);
    }

    fn apply_sparse_patches(mut bytes: Vec<u8>, patches: &[(u64, Vec<u8>)], len: usize) -> Vec<u8> {
        bytes.resize(len, 0);
        for (offset, data) in patches {
            let start = *offset as usize;
            bytes[start..start + data.len()].copy_from_slice(data);
        }
        bytes
    }

    #[test]
    fn dirty_candidate_updates_only_its_gpu_physical_bounds_slot() {
        let original = sample_draws();
        let mut keys = Vec::new();
        let (slots, mapping) =
            plan_resident_bounds(&StableDrawSlots::default(), &original, &mut keys).unwrap();
        let batches = (0..original.len()).map(|index| DrawBatchRange {
            first_instance: index, instance_count: 1,
        }).collect::<Vec<_>>();
        let mut batch_keys = Vec::new();
        batch_record_keys(&original, &batches, None, &mut batch_keys).unwrap();
        let mut updated = original.clone();
        updated[3].bounds.max.x += 1.0;
        let patch = sparse_resident_bounds_patches(
            &updated, &batches, None, &[3],
            &slots, &mapping, &keys, &batch_keys,
        ).unwrap();
        assert_eq!(patch.len(), 1);
        assert_eq!(patch[0].0, mapping[3] as usize);
        let expected = BoundsRecordKey::from_draw(&updated[3]);
        assert_eq!(patch[0].1, expected);
        assert_eq!(patch[0].2.as_slice(),
                   encode_bounds(std::slice::from_ref(&updated[3])));
        let unchanged = sparse_resident_bounds_patches(
            &original, &batches, None, &[3],
            &slots, &mapping, &keys, &batch_keys,
        ).unwrap();
        assert!(unchanged.is_empty());
        // Identity changes must re-plan the slot instead of writing the
        // bounds of one mesh into another mesh's resident address.
        updated[3] = original[2].clone();
        assert!(sparse_resident_bounds_patches(
            &updated, &batches, None, &[3],
            &slots, &mapping, &keys, &batch_keys,
        ).is_none());
    }

    #[test]
    fn streamed_sections_keep_resident_gpu_bounds_across_reorder_and_removal() {
        let draws = sample_draws();
        let mut initial_keys = Vec::new();
        let (slots, first_map) =
            plan_resident_bounds(&StableDrawSlots::default(), &draws, &mut initial_keys).unwrap();
        assert_eq!(first_map.len(), draws.len());
        let first_upload = resident_bounds_patches(&[], &initial_keys);
        let first_bytes = apply_sparse_patches(
            Vec::new(), &first_upload, slots.high_water() * BOUNDS_STRIDE,
        );
        for (draw, &physical) in draws.iter().zip(&first_map) {
            let expected = encode_bounds(std::slice::from_ref(draw));
            let start = physical as usize * BOUNDS_STRIDE;
            assert_eq!(&first_bytes[start..start + BOUNDS_STRIDE], expected);
        }
        let mut reordered = draws.clone();
        reordered.reverse();
        reordered.remove(3);
        let mut next_keys = Vec::new();
        let (next_slots, next_map) =
            plan_resident_bounds(&slots, &reordered, &mut next_keys).unwrap();
        for (draw, &physical) in reordered.iter().zip(&next_map) {
            let original_index = draws.iter().position(|other| other.node == draw.node).unwrap();
            assert_eq!(physical, first_map[original_index]);
        }
        assert_eq!(next_slots.high_water(), slots.high_water());
        let changes = resident_bounds_patches(&initial_keys, &next_keys);
        assert_eq!(changes.iter().map(|(_, b)| b.len()).sum::<usize>(), BOUNDS_STRIDE);
        let next_bytes = apply_sparse_patches(
            first_bytes, &changes, next_slots.high_water() * BOUNDS_STRIDE,
        );
        for (draw, &physical) in reordered.iter().zip(&next_map) {
            let expected = encode_bounds(std::slice::from_ref(draw));
            let start = physical as usize * BOUNDS_STRIDE;
            assert_eq!(&next_bytes[start..start + BOUNDS_STRIDE], expected);
        }
        assert!(!resident_mapping_patches(&first_map, &next_map).is_empty());
        let empty_change = resident_bounds_patches(&next_keys, &next_keys);
        assert!(empty_change.is_empty(), "unchanged Section bounds must not be uploaded");
    }

    #[test]
    fn only_changed_section_bounds_are_uploaded_and_slots_reused() {
        let draws = sample_draws();
        let mut original_keys = Vec::new();
        let (original_slots, original_map) =
            plan_resident_bounds(&StableDrawSlots::default(), &draws, &mut original_keys).unwrap();
        let mut next = draws.clone();
        next[4].bounds.max.x += 0.125;
        let mut keys = Vec::new();
        let (slots, next_map) =
            plan_resident_bounds(&original_slots, &next, &mut keys).unwrap();
        assert_eq!(slots.high_water(), original_slots.high_water());
        assert_eq!(next_map, original_map);
        let changed = resident_bounds_patches(&original_keys, &keys);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0, original_map[4] as u64 * BOUNDS_STRIDE as u64);
        assert_eq!(changed[0].1.len(), BOUNDS_STRIDE);
        assert!(resident_mapping_patches(&original_map, &next_map).is_empty());
        // A subsequent add reuses a free physical slot instead of reallocating
        // every unchanged resident Section.
        next.remove(0);
        let (removed, _) = plan_resident_bounds(&slots, &next, &mut keys).unwrap();
        next.push(draws[0].clone());
        let (restored, restored_mapping) =
            plan_resident_bounds(&removed, &next, &mut keys).unwrap();
        assert_eq!(restored.high_water(), original_slots.high_water());
        assert_eq!(restored_mapping.last(), original_map.first());
    }

}
