//! Resident batch bounds and compute-generated indirect arguments.

use super::{FRAME_PARAMS_STRIDE, gpu::buffer_memory_location, scene::DrawBatchRange};
use crate::{Aabb, PreparedDraw};
use anyhow::{Context as _, anyhow};
use gfx_core::{
    BackendKind, BufferBinding, BufferDescriptor, BufferId, BufferUsage, ComputeBufferAccess,
    ComputeBufferBinding, ComputeDispatch, MemoryLocation, ShaderModuleId,
};
use gpui::ExtensionDevice;
use std::{collections::hash_map::DefaultHasher, hash::{Hash, Hasher}};

const BOUNDS_STRIDE: usize = 48;
pub(super) const ARGUMENT_STRIDE: u64 = 16;


/// Byte-accurate identity of a 48-byte GPU culling bounds record.
/// Converting floats to their bit patterns distinguishes signed zeros and
/// prevents a cache hit from silently changing the generated GPU bytes.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
struct BoundsRecordKey([u32; 12]);

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
    range_bounds: Option<Option<Aabb>>,
    pub(super) arguments: BufferId,
    pub(super) visible_instances: BufferId,
    pub(super) instance_capacity: usize,
    batch_capacity: usize,
    count: usize,
    instances: usize,
    uploaded_inputs: [Vec<u8>; 2],
    scratch_inputs: [Vec<u8>; 2],
    bounds_keys: Vec<BoundsRecordKey>,
    scratch_bounds_keys: Vec<BoundsRecordKey>,
    bounds_encode_scratch: Vec<u8>,
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
            range_bounds: None,
            count: 0,
            instances: 0,
            uploaded_inputs: Default::default(),
            scratch_inputs: Default::default(),
            bounds_keys: Vec::new(),
            scratch_bounds_keys: Vec::new(),
            bounds_encode_scratch: Vec::new(),
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
        if draws.len() <= self.instance_capacity && batches.len() <= self.batch_capacity {
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
        // Reuse both encoded staging vectors. Streaming sections may publish a new
        // scene every few frames, and allocating full draw/cull tables on every
        // update becomes expensive when the scene contains many material parts.
        let [mut bounds, mut bytes] = std::mem::take(&mut self.scratch_inputs);
        let mut next_bounds_keys = std::mem::take(&mut self.scratch_bounds_keys);
        let mut run_scratch = std::mem::take(&mut self.bounds_encode_scratch);
        let bounds_encoded = encode_bounds_cached(
            draws,
            &self.bounds_keys,
            &self.uploaded_inputs[0],
            &mut next_bounds_keys,
            &mut bounds,
            &mut run_scratch,
        );
        self.bounds_encode_scratch = run_scratch;
        if let Err(error) = encode_batches_into(draws, batches, meshes, &mut bytes) {
            self.scratch_bounds_keys = next_bounds_keys;
            self.scratch_inputs = [bounds, bytes];
            return Err(error);
        }
        let write_result = {
            let writes = [
                (self.bounds, bounds.as_slice(), BOUNDS_STRIDE),
                (self.batches, bytes.as_slice(), ARGUMENT_STRIDE as usize),
            ]
            .into_iter()
            .enumerate()
            .flat_map(|(i, (buffer, data, stride))| {
                super::uploads::changed_ranges(
                    &self.uploaded_inputs[i],
                    data,
                    stride,
                    12,
                    4,
                )
                .into_iter()
                .map(move |range| gfx_core::BufferWrite {
                    descriptor: gfx_core::BufferWriteDescriptor {
                        buffer,
                        offset: range.start as u64,
                    },
                    data: &data[range],
                })
            })
            .collect::<Vec<_>>();
            if writes.is_empty() {
                Ok(())
            } else {
                device.write_buffer_batch(&writes).map(|_| ())
            }
        };
        if let Err(error) = write_result {
            // Keep the last successfully uploaded snapshot for the retry.
            self.scratch_bounds_keys = next_bounds_keys;
            self.scratch_inputs = [bounds, bytes];
            return Err(error.into());
        }
        let mut old_keys = std::mem::replace(&mut self.bounds_keys, next_bounds_keys);
        old_keys.clear();
        self.scratch_bounds_keys = old_keys;
        let mut old = std::mem::replace(&mut self.uploaded_inputs, [bounds, bytes]);
        if super::renderer::trace_timings() && bounds_encoded > 0 && bounds_encoded < draws.len() {
            eprintln!(
                "gpui3d cull_bounds_cache rows={} encoded={} reused={}",
                draws.len(),
                bounds_encoded,
                draws.len() - bounds_encoded,
            );
        }
        for input in &mut old {
            input.clear();
        }
        self.scratch_inputs = old;
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
        let mut range_bytes = [0; 32];
        for (chunk, value) in range_bytes
            .chunks_exact_mut(4)
            .zip([min[0], min[1], min[2], 0.0, max[0], max[1], max[2], 0.0])
        {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        if self.range_bounds != Some(range) {
            device.write_buffer(self.range, 0, &range_bytes)?;
            self.range_bounds = Some(range);
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
                (self.instances * BOUNDS_STRIDE) as u64,
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
        ];
        let limit = device.compute_capabilities().max_workgroups[0];
        let count = u32::try_from(self.count).context("GPU culling batch count exceeds u32")?;
        let width = count.min(limit.max(1));
        device.dispatch_compute(&ComputeDispatch {
            shader,
            bindings: &bindings,
            workgroups: [width, count.div_ceil(width), 1],
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
) -> gpui::Result<[BufferId; 5]> {
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
    ];
    let mut buffers = Vec::with_capacity(5);
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

fn encode_batches_into(
    draws: &[PreparedDraw],
    batches: &[DrawBatchRange],
    meshes: Option<&std::collections::HashMap<(crate::MeshId, bool), super::gpu::MeshResources>>,
    bytes: &mut Vec<u8>,
) -> gpui::Result<()> {
    let capacity = batches
        .len()
        .checked_mul(ARGUMENT_STRIDE as usize)
        .ok_or_else(|| anyhow!("GPU bounds size overflow"))?;
    bytes.clear();
    bytes.reserve(capacity);
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
        for value in [
            u32::from(generated),
            u32::try_from(batch.instance_count)?,
            0,
            u32::try_from(batch.first_instance)?,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
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
}
