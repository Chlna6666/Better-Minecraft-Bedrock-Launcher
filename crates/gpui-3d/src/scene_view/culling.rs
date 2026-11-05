//! Resident batch bounds and compute-generated indirect arguments.

use super::{FRAME_PARAMS_STRIDE, gpu::buffer_memory_location, scene::DrawBatchRange};
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


/// Byte-accurate identity of a 48-byte GPU culling bounds record.
/// Converting floats to their bit patterns distinguishes signed zeros and
/// prevents a cache hit from silently changing the generated GPU bytes.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
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

pub(super) struct CullResources {
    pub(super) hiz: super::hiz::HiZ,
    backend: BackendKind,
    bounds: BufferId,
    pub(super) batches: BufferId,
    range: BufferId,
    range_bounds: Option<(Option<Aabb>, bool)>,
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
        let instance_capacity = draws
            .len()
            .max(64)
            .checked_next_power_of_two()
            .context("GPU instance capacity overflow")?;
        let batch_capacity = batches
            .len()
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
        let (next_slots, logical_to_physical) = self.resident_slots.plan(draws)?;
        if next_slots.high_water() > self.instance_capacity {
            return Err(anyhow!("GPU culling physical slot capacity exceeded").into());
        }
        let mut next_bounds_keys = std::mem::take(&mut self.scratch_bounds_keys);
        next_bounds_keys.clear();
        next_bounds_keys.resize(next_slots.high_water(), None);
        for (draw, &slot) in draws.iter().zip(&logical_to_physical) {
            next_bounds_keys[slot as usize] = Some(BoundsRecordKey::from_draw(draw));
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
        let bounds_ranges = super::uploads::changed_ranges_for_keys(
            &self.bounds_keys,
            &next_bounds_keys,
            self.bounds_keys.len() * BOUNDS_STRIDE,
            BOUNDS_STRIDE, 12, 3,
        );
        let mut bounds_patches = Vec::with_capacity(bounds_ranges.len());
        for range in bounds_ranges {
            let mut bytes = vec![0u8; range.len()];
            for (row, key) in next_bounds_keys[
                range.start / BOUNDS_STRIDE..range.end / BOUNDS_STRIDE
            ].iter().enumerate() {
                if let Some(key) = key {
                    let start = row * BOUNDS_STRIDE;
                    for (word, bits) in key.0.iter().enumerate() {
                        bytes[start + word * 4..start + word * 4 + 4]
                            .copy_from_slice(&bits.to_le_bytes());
                    }
                }
            }
            bounds_patches.push((range.start as u64, bytes));
        }

        // The shader uses a compact u32 indirection table. Reordering Section
        // batches modifies only these four-byte records, not their 48-byte
        // bounds. Avoid serializing unchanged mapping rows.
        let mapping_ranges = super::uploads::changed_ranges_for_u32_records(
            &self.uploaded_slot_map, &logical_to_physical, 1, 12, 3,
        );
        let mut mapping_patches = Vec::with_capacity(mapping_ranges.len());
        for range in mapping_ranges {
            let mut bytes = Vec::with_capacity(range.len());
            for slot in &logical_to_physical[range.start / 4..range.end / 4] {
                bytes.extend_from_slice(&slot.to_le_bytes());
            }
            mapping_patches.push((range.start as u64, bytes));
        }

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
        Ok(())
    }

    pub(super) fn dispatch(
        &mut self,
        device: &mut dyn ExtensionDevice,
        shader: ShaderModuleId,
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
        let mut range_bytes = [0; 32];
        for (chunk, value) in range_bytes
            .chunks_exact_mut(4)
            .zip([min[0], min[1], min[2], if occlusion_enabled { 1.0 } else { 0.0 }, max[0], max[1], max[2], 0.0])
        {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        if self.range_bounds != Some((range, occlusion_enabled)) {
            device.write_buffer(self.range, 0, &range_bytes)?;
            self.range_bounds = Some((range, occlusion_enabled));
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
            compute_binding(5, self.range, 32, ComputeBufferAccess::Uniform),
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
        let count = u32::try_from(self.count).context("GPU culling batch count exceeds u32")?;
        let grid = device.compute_capabilities().plan_linear_dispatch_2d(count)?;
        device.dispatch_compute(&ComputeDispatch {
            shader,
            bindings: &bindings,
            workgroups: grid,
        })?;
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
        (32, BufferUsage::UNIFORM | BufferUsage::COPY_DST),
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
    let dirty = next.iter().enumerate()
        .filter(|(index, key)| previous.get(*index).copied() != Some(**key))
        .count();
    output.clear();
    if dirty == 0 && previous == next && previous_bytes.len() == size {
        return Ok(0);
    }
    output.extend_from_slice(previous_bytes);
    output.resize(size, 0);
    for (index, key) in next.iter().enumerate() {
        if previous.get(index) == Some(key)
            && previous_bytes.len() >= (index + 1) * ARGUMENT_STRIDE as usize
        {
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

}
