//! Compute-generated face descriptors, retained templates and native draw arguments.
use super::{artifacts, gpu::{ArenaAllocation, MeshArena, MeshResources}};
use crate::{Mesh, VoxelMeshSource};
use anyhow::{Context as _, anyhow};
use gfx_core::*;
use gpui::ExtensionDevice;
use std::{collections::HashMap, time::Instant};

#[cfg(all(test, target_os = "windows"))]
#[path = "generated_tests.rs"]
pub(super) mod tests;

#[cfg(all(test, target_os = "windows"))]
#[path = "generated_lighting_tests.rs"]
mod lighting_tests;

pub(super) struct GeneratedMesh {
    pub templates: BufferId,
    pub arguments: BufferId,
    pub template_bytes: u64,
    /// Aligned suballocation within one shared GPU template storage page.
    pub template_offset: u64,
    pub template_allocation: Option<ArenaAllocation>,
    /// Absolute index of the first 112-byte header record in the shared page.
    pub template_base: u32,
    pub template_page_bytes: u64,
    /// Resident material-count buffer shared by other generated Sections.
    pub argument_allocation: Option<ArenaAllocation>,
    pub argument_base: u32,
    pub argument_offset: u64,
    pub argument_page_bytes: u64,
}
pub(super) struct Shaders(pub [ShaderModuleId; 6]);
pub(super) fn create_shaders(
    device: &mut dyn ExtensionDevice,
    backend: BackendKind,
) -> gpui::Result<Shaders> {
    let mut shaders = Vec::new();
    for entry in [
        "cs_mesh",
        "cs_mesh_static_counts",
        "cs_mesh_static_emit",
        "cs_mesh_instance_counts",
        "cs_mesh_instance_scan",
        "cs_mesh_instance_emit",
    ] {
        match create_shader(device, backend, entry) {
            Ok(shader) => shaders.push(shader),
            Err(error) => {
                for shader in shaders {
                    let _ = device.destroy_shader_module(shader);
                }
                return Err(error);
            }
        }
    }
    Ok(Shaders(
        shaders
            .try_into()
            .map_err(|_| anyhow!("missing meshing stage"))?,
    ))
}

pub(super) fn create_shader(
    device: &mut dyn ExtensionDevice,
    backend: BackendKind,
    entry: &str,
) -> gpui::Result<ShaderModuleId> {
    let artifact = match backend {
        #[cfg(target_os = "windows")]
        BackendKind::Dx11 => artifacts::scene_view_dx11_shader(entry),
        #[cfg(target_os = "windows")]
        BackendKind::Dx12 => artifacts::scene_view_dx12_shader(entry),
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        BackendKind::Vulkan => artifacts::scene_view_vulkan_shader(entry),
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        BackendKind::OpenGl => artifacts::scene_view_opengl_shader(entry),
        _ => None,
    }
    .ok_or_else(|| anyhow!("GPU voxel meshing shader is unavailable"))?;
    Ok(device.create_shader_module(&ShaderModuleDescriptor {
        label: Some("voxel meshing".into()),
        binary: artifact.to_binary(ShaderStage::Compute, entry)?,
    })?)
}

/// Map a face material to its first matching part. Preserve the existing
/// fallback to part zero for malformed or missing material references.
fn material_part_lookup(source: &VoxelMeshSource) -> HashMap<u32, usize> {
    let mut lookup = HashMap::with_capacity(source.parts.len());
    for (index, part) in source.parts.iter().enumerate() {
        lookup.entry(part.material_slot()).or_insert(index);
    }
    lookup
}

fn encode(source: &VoxelMeshSource) -> (Vec<u8>, Vec<u32>) {
    // Two 112-byte header records allow every generated Section to be
    // addressed by one GPU template-record index, without per-Mesh BindGroups.
    // Header 0 stores the original float origin as raw u32 words; header 1
    // stores the grid dimensions and the absolute lighting-word offset.
    let mut bytes = Vec::with_capacity(224 + source.templates.len() * 112);
    for value in [
        source.origin.x,
        source.origin.y,
        source.origin.z,
        source.lighting.as_ref().map_or(0., |light| light.ambient),
    ] {
        bytes.extend(value.to_le_bytes());
    }
    bytes.resize(112, 0);
    for value in source.size.into_iter().chain([0]) {
        bytes.extend(value.to_le_bytes());
    }
    bytes.resize(224, 0);
    let mut static_counts = vec![0u32; source.parts.len()];
    // The previous per-template .position() scanned all material parts for
    // every face. Resolve each material slot once, independent of face count.
    let material_parts = material_part_lookup(source);
    for (index, face) in source.templates.iter().enumerate() {
        let part = material_parts.get(&face.material_slot).copied().unwrap_or(0);
        let center = face.vertices.iter().map(|v| v.position.y).sum::<f32>() * 0.25;
        let address = if index < source.static_start {
            source.neighbour_delta(face) as u32
        } else if center < source.clip_y[0] || center >= source.clip_y[1] {
            u32::MAX
        } else {
            let slot = static_counts[part];
            static_counts[part] += 1;
            slot
        };
        for value in [
            part as u32,
            address,
            source.parts[part].first_index() / 6,
            center.to_bits(),
        ] {
            bytes.extend(value.to_le_bytes());
        }
        for vertex in face.vertices {
            for value in [vertex.position.x, vertex.position.y, vertex.position.z] {
                bytes.extend(value.to_le_bytes());
            }
            let color = vertex.color.iter().enumerate().fold(0u32, |word, (i, c)| {
                word | ((c * 255.0).round() as u32) << (8 * i)
            });
            bytes.extend(color.to_le_bytes());
            bytes.extend(vertex.uv.x.to_le_bytes());
            bytes.extend(vertex.uv.y.to_le_bytes());
        }
    }
    if let Some(light) = &source.lighting {
        let base = (bytes.len() / 4) as u32;
        bytes[124..128].copy_from_slice(&base.to_le_bytes());
        bytes.extend_from_slice(&light.levels);
        bytes.resize(bytes.len().div_ceil(4) * 4, 0);
    }
    (bytes, static_counts)
}

/// Section-scoped GPU resources prepared for one shared native compute batch.
/// The three meshing scratch/input buffers are released only AFTER the batch
/// dispatch completes. The immutable generated output/argument/template buffers
/// remain resident for subsequent frames.
struct PendingGenerated {
    buffers: Vec<BufferId>,
    bindings: Vec<ComputeBufferBinding>,
    stages: Vec<(usize, [u32; 3])>,
    generation: u64,
    output_bytes: usize,
    template_bytes: u64,
    // A generated section occupies an independently retired slice of a
    // page shared with other Section outputs. Legacy GPU tests use None.
    allocation: Option<ArenaAllocation>,
    template_allocation: Option<ArenaAllocation>,
    template_offset: u64,
    template_base: u32,
    template_page_bytes: u64,
    argument_allocation: Option<ArenaAllocation>,
    argument_base: u32,
    argument_offset: u64,
    argument_page_bytes: u64,
    vertex_base: u32,
    vertex_binding_bytes: usize,
    /// Per-Section mesher input writes, deferred so one batch upload covers a
    /// whole streaming batch instead of one native submit per Section.
    staging: Vec<StagedBufferWrite>,
    /// Slots whose buffers belong to the batch scratch pool. The pool owner
    /// destroys them once after the batch dispatches; per-Section cleanup and
    /// post-dispatch scratch collection must skip them.
    shared_scratch: [bool; 6],
}

/// Owned staging write for one mesher input buffer.
struct StagedBufferWrite {
    descriptor: BufferWriteDescriptor,
    data: Vec<u8>,
}

impl PendingGenerated {
    fn into_resources(self) -> MeshResources {
        MeshResources {
            generation: self.generation,
            vertices: self.buffers[2],
            vertex_binding_bytes: self.vertex_binding_bytes,
            vertex_stride: 16,
            vertex_base: self.vertex_base,
            tangents: None,
            tangent_bytes: 0,
            indices: self.buffers[3],
            index_format: IndexFormat::Uint16,
            index_offset: 0,
            procedural_quads: true,
            allocation: self.allocation,
            generated: Some(GeneratedMesh {
                templates: self.buffers[1],
                arguments: self.buffers[3],
                template_bytes: self.template_bytes,
                template_offset: self.template_offset,
                template_allocation: self.template_allocation,
                template_base: self.template_base,
                template_page_bytes: self.template_page_bytes,
                argument_allocation: self.argument_allocation,
                argument_base: self.argument_base,
                argument_offset: self.argument_offset,
                argument_page_bytes: self.argument_page_bytes,
            }),
        }
    }
}

fn discard_pending_generated(
    device: &mut dyn ExtensionDevice,
    pending: Vec<PendingGenerated>,
    mut arena: Option<(&mut MeshArena, &mut MeshArena, &mut MeshArena)>,
) {
    for mesh in pending {
        for (slot, &buffer) in mesh.buffers.iter().enumerate() {
            if (slot != 2 || mesh.allocation.is_none())
                && (slot != 1 || mesh.template_allocation.is_none())
                && (slot != 3 || mesh.argument_allocation.is_none())
            {
                let _ = device.destroy_buffer(buffer);
            }
        }
        if let Some((faces, templates, arguments)) = arena.as_mut() {
            if let Some(allocation) = mesh.argument_allocation {
                let _ = arguments.release(device, allocation);
            }
            if let Some(allocation) = mesh.template_allocation {
                let _ = templates.release(device, allocation);
            }
            if let Some(allocation) = mesh.allocation {
                let _ = faces.release(device, allocation);
            }
        }
    }
}

/// Allocate and upload only the changed Section's immutable compact source.
/// No Compute submission occurs here, allowing multiple independent Sections
/// to share one GPU command submission instead of one submission per mesh.
fn prepare_generated(
    device: &mut dyn ExtensionDevice,
    mesh: &Mesh,
    mut arena: Option<(&mut MeshArena, &mut MeshArena, &mut MeshArena, BackendKind)>,
    mut scratch: Option<&mut crate::scene_view::gpu::BatchScratch>,
) -> gpui::Result<PendingGenerated> {
    let source = mesh
        .gpu_voxels()
        .ok_or_else(|| anyhow!("missing voxel source"))?;
    let (mut templates, static_counts) = encode(source);
    let mut words = if source.model_faces == 0 {
        Vec::new()
    } else {
        source.cells.to_vec()
    };
    let class_offset = words.len() as u32;
    if let Some(instances) = &source.instances {
        instances.classes.encode(&mut words);
    }
    let ranges_offset = words.len() as u32;
    let jobs = source
        .instances
        .as_ref()
        .map_or(&[][..], |instances| instances.jobs.as_ref());
    for part in &source.parts {
        let first = jobs.partition_point(|job| job.material < part.material_slot());
        let end = jobs.partition_point(|job| job.material <= part.material_slot());
        words.extend([first as u32, (end - first) as u32]);
    }
    let jobs_offset = words.len() as u32;
    for job in jobs {
        words.extend([
            job.position[0],
            job.position[1],
            job.position[2],
            job.height.to_bits(),
            job.first_face,
            job.face_count,
            job.material,
            job.occlusion_mask,
        ]);
    }
    let mut cells = words
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    // A static-only Section has no cube cells or instance jobs. D3D12 and
    // Vulkan cannot portably create/bind a zero-byte storage buffer, yet the
    // shared mesher layout always declares binding 0. Keep a single inert
    // word to preserve the same 6-binding cross-backend pipeline.
    if cells.is_empty() {
        cells.extend_from_slice(&0u32.to_le_bytes());
    }
    let args = static_counts
        .into_iter()
        .flat_map(|count| [0u32, 1, 0, count])
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    let params = source
        .size
        .into_iter()
        .chain([source.static_start as u32])
        .flat_map(u32::to_le_bytes)
        .chain(
            [source.origin.x, source.origin.y, source.origin.z, 0.0]
                .into_iter()
                .flat_map(f32::to_le_bytes),
        )
        .chain(
            [
                source.clip_y[0],
                source.clip_y[1],
                f32::from_bits((source.templates.len() - source.static_start) as u32),
                f32::from_bits(source.ambient_occlusion),
            ]
            .into_iter()
            .flat_map(f32::to_le_bytes),
        )
        .chain(
            [
                jobs.len() as u32,
                ranges_offset,
                jobs_offset,
                class_offset
                    | if source
                        .instances
                        .as_ref()
                        .is_some_and(|instances| instances.classes.is_packed())
                    {
                        0x80000000
                    } else {
                        0
                    },
            ]
            .into_iter()
            .flat_map(u32::to_le_bytes),
        )
        .collect::<Vec<_>>();
    let output_bytes = source
        .capacity()
        .checked_mul(16)
        .ok_or_else(|| anyhow!("voxel capacity overflow"))?;
    let specs = [
        (cells.len(), BufferUsage::STORAGE | BufferUsage::COPY_DST),
        (
            templates.len(),
            BufferUsage::STORAGE | BufferUsage::COPY_DST,
        ),
        (
            output_bytes,
            BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE,
        ),
        (
            args.len(),
            BufferUsage::STORAGE
                | BufferUsage::STORAGE_WRITE
                | BufferUsage::INDIRECT
                | BufferUsage::COPY_DST,
        ),
        (64, BufferUsage::UNIFORM | BufferUsage::COPY_DST),
        (
            jobs.len().max(1) * 4,
            BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE,
        ),
    ];
    let mut buffers = Vec::new();
    // Slots served by the batch scratch pool are owned by the batch, not the
    // Section: cleanup and post-dispatch destruction must skip them.
    let mut shared_scratch = [false; 6];
    let mut scratch_offsets = [0u64; 6];
    let mut pooled_allocation = None;
    let mut template_allocation = None;
    let mut template_offset = 0u64;
    let mut template_base = 0u32;
    let mut template_page_bytes = templates.len() as u64;
    let mut argument_allocation = None;
    let mut argument_offset = 0u64;
    let mut argument_base = 0u32;
    let mut argument_page_bytes = args.len() as u64;
    let mut pooled_offset = 0u64;
    let mut pooled_capacity = output_bytes;
    let result = (|| {
        for (slot, (size, usage)) in specs.into_iter().enumerate() {
            if (slot == 1 || slot == 2 || slot == 3)
                && let Some((faces, templates, arguments, backend)) = arena.as_mut()
            {
                // Do not unify three distinct &mut MeshArena lifetimes in a
                // returned reference. Dispatch the allocation operation
                // itself; only its owned result crosses the match expression.
                let (allocation, buffer, offset, capacity) = match slot {
                    1 => templates.allocate(device, *backend, size as u64),
                    2 => faces.allocate(device, *backend, size as u64),
                    3 => arguments.allocate(device, *backend, size as u64),
                    _ => unreachable!(),
                }?;
                // Both immutable template inputs and generated face outputs
                // live in GPU-owner shared pages, but occupy disjoint slices.
                // Compute uses offset-qualified bindings while the shader
                // continues to address each Section relative to its slice.
                if slot == 1 {
                    template_allocation = Some(allocation);
                    template_offset = offset;
                    if offset % 112 != 0 {
                        return Err(anyhow!("template page offset violates 112-byte record alignment"));
                    }
                    template_base = u32::try_from(offset / 112)
                        .context("generated template index exceeds u32")?;
                    template_page_bytes = capacity;
                } else if slot == 2 {
                    pooled_allocation = Some(allocation);
                    pooled_offset = offset;
                    pooled_capacity = usize::try_from(capacity)
                        .context("generated face page exceeds host address space")?;
                    if offset / 16 >= 0x8000_0000 {
                        return Err(anyhow!("generated face pool exceeds vertex address range"));
                    }
                } else {
                    argument_allocation = Some(allocation);
                    argument_offset = offset;
                    if offset % 16 != 0 {
                        return Err(anyhow!("generated argument offset violates u32x4 record alignment"));
                    }
                    argument_base = u32::try_from(offset / 16)
                        .context("generated part-count address exceeds u32")?;
                    argument_page_bytes = capacity;
                }
                buffers.push(buffer);
            } else if slot != 1
                && slot != 2
                && slot != 3
                && let Some(scratch) = scratch.as_deref_mut()
            {
                let (buffer, offset) = match slot {
                    4 => scratch.params(device, size as u64)?,
                    5 => scratch.jobs(device, size as u64)?,
                    _ => scratch.cells(device, size as u64)?,
                };
                shared_scratch[slot] = true;
                scratch_offsets[slot] = offset;
                buffers.push(buffer);
            } else {
                buffers.push(device.create_buffer(&BufferDescriptor {
                    label: Some("GPU voxel meshing".into()),
                    size: size as u64,
                    usage,
                    memory_location: MemoryLocation::GpuOnly,
                })?);
            }
        }
        // Lighting words are addressed from the full page at render time,
        // not from a per-Section bound subrange. The mesher ignores this field.
        if source.lighting.is_some() {
            let local = u32::from_le_bytes(templates[124..128].try_into().unwrap());
            let page_offset = u32::try_from(template_offset / 4)
                .context("generated lighting offset exceeds u32")?;
            let absolute = page_offset.checked_add(local)
                .ok_or_else(|| anyhow!("generated lighting page address overflow"))?;
            templates[124..128].copy_from_slice(&absolute.to_le_bytes());
        }
        let writes =
            [(0, &cells), (1, &templates), (3, &args), (4, &params)].map(|(slot, data)| {
                BufferWrite {
                    descriptor: BufferWriteDescriptor {
                        buffer: buffers[slot],
                        offset: if shared_scratch[slot] {
                            scratch_offsets[slot]
                        } else {
                            match slot {
                                1 => template_offset,
                                3 => argument_offset,
                                _ => 0,
                            }
                        },
                    },
                    data: data.as_slice(),
                }
            });
        let staging = writes
            .into_iter()
            .map(|write| StagedBufferWrite {
                descriptor: write.descriptor,
                data: write.data.to_vec(),
            })
            .collect::<Vec<_>>();
        let bindings = specs
            .iter()
            .enumerate()
            .map(|(slot, (size, _))| ComputeBufferBinding {
                binding: slot as u32,
                buffer: BufferBinding {
                    buffer: buffers[slot],
                    offset: if shared_scratch[slot] {
                        scratch_offsets[slot]
                    } else {
                        match slot {
                            1 => template_offset,
                            2 => pooled_offset,
                            3 => argument_offset,
                            _ => 0,
                        }
                    },
                    size: *size as u64,
                    stride: None,
                },
                access: match slot {
                    2 | 3 | 5 => ComputeBufferAccess::ReadWrite,
                    4 => ComputeBufferAccess::Uniform,
                    _ => ComputeBufferAccess::ReadOnly,
                },
            })
            .collect::<Vec<_>>();
        let mut stages = Vec::new();
        if source.model_faces > 0 {
            stages.push((0, source.cells.len().div_ceil(64)));
        }
        if source.templates.len() > source.static_start {
            stages.extend([
                (1, source.parts.len().div_ceil(64)),
                (
                    2,
                    (source.templates.len() - source.static_start).div_ceil(64),
                ),
            ]);
        }
        if !jobs.is_empty() {
            stages.extend([
                (3, jobs.len().div_ceil(64)),
                (4, source.parts.len()),
                (5, jobs.len().div_ceil(64)),
            ]);
        }
        let compute_caps = device.compute_capabilities();
        let stage_grids = stages
            .into_iter()
            .map(|(stage, count)| {
                let count = u32::try_from(count).context("voxel dispatch overflow")?;
                Ok((stage, compute_caps.plan_linear_dispatch_2d(count)?))
            })
            .collect::<gpui::Result<Vec<_>>>()?;
        Ok::<_, anyhow::Error>((bindings, stage_grids, staging))
    })();
    let (bindings, stages, staging) = match result {
        Ok(prepared) => prepared,
        Err(error) => {
            for (slot, buffer) in buffers.into_iter().enumerate() {
                if !shared_scratch[slot]
                    && (slot != 2 || pooled_allocation.is_none())
                    && (slot != 1 || template_allocation.is_none())
                    && (slot != 3 || argument_allocation.is_none())
                {
                    let _ = device.destroy_buffer(buffer);
                }
            }
            if let Some((faces, templates, arguments, _)) = arena {
                if let Some(allocation) = argument_allocation {
                    let _ = arguments.release(device, allocation);
                }
                if let Some(allocation) = template_allocation {
                    let _ = templates.release(device, allocation);
                }
                if let Some(allocation) = pooled_allocation {
                    let _ = faces.release(device, allocation);
                }
            }
            return Err(error);
        }
    };
    Ok(PendingGenerated {
        buffers,
        bindings,
        stages,
        generation: mesh.generation(),
        output_bytes,
        template_bytes: templates.len() as u64,
        vertex_base: (pooled_offset / 16) as u32,
        vertex_binding_bytes: pooled_capacity,
        staging,
        shared_scratch,
        allocation: pooled_allocation,
        template_allocation,
        template_offset,
        template_base,
        template_page_bytes,
        argument_allocation,
        argument_base,
        argument_offset,
        argument_page_bytes,
    })
}

/// Prepare independent changed Sections, then submit all meshing stages through
/// the common cross-backend ordered ComputeDispatch API. Per-Section output
/// buffers remain stable and are not recreated for unchanged Scene snapshots.
pub(super) fn create_many(
    device: &mut dyn ExtensionDevice,
    meshes: &[&Mesh],
    shaders: &Shaders,
) -> gpui::Result<Vec<MeshResources>> {
    create_many_impl(device, meshes, shaders, None, None, None)
}

/// Production GPU meshing suballocates output descriptors into shared writable
/// storage pages. Existing test helpers retain a standalone variant above.
pub(super) fn create_many_pooled(
    device: &mut dyn ExtensionDevice,
    meshes: &[&Mesh],
    shaders: &Shaders,
    arena: &mut MeshArena,
    template_arena: &mut MeshArena,
    argument_arena: &mut MeshArena,
    backend: BackendKind,
    batch_id: u64,
) -> gpui::Result<Vec<MeshResources>> {
    // One batch scratch pool serves every Section's cells/params/jobs input
    // buffers: three native buffer creations per batch instead of per Section.
    let mut scratch = crate::scene_view::gpu::BatchScratch::new(device.buffer_binding_limits());
    let profile = crate::gpu_submission_trace::enabled()
        && device.gpu_work_timestamps_supported();
    let result = create_many_impl(
        device, meshes, shaders,
        Some((arena, template_arena, argument_arena, backend)),
        Some(&mut scratch),
        profile.then_some(batch_id),
    );
    match result {
        Ok(resources) => scratch.destroy(device).map(|_| resources).map_err(anyhow::Error::from),
        Err(error) => {
            let _ = scratch.destroy(device);
            Err(error)
        }
    }
}

fn create_many_impl(
    device: &mut dyn ExtensionDevice,
    meshes: &[&Mesh],
    shaders: &Shaders,
    mut arena: Option<(&mut MeshArena, &mut MeshArena, &mut MeshArena, BackendKind)>,
    mut scratch: Option<&mut crate::scene_view::gpu::BatchScratch>,
    batch_id: Option<u64>,
) -> gpui::Result<Vec<MeshResources>> {
    if meshes.is_empty() {
        return Ok(Vec::new());
    }
    // Trace only when explicitly enabled; no per-Section clock reads in the
    // hot path unless performance profiling was requested.
    let trace_started = super::renderer::trace_timings().then(Instant::now);
    let mut pending = Vec::with_capacity(meshes.len());
    for mesh in meshes {
        let pool = arena.as_mut().map(|(faces, templates, arguments, backend)| {
            (&mut **faces, &mut **templates, &mut **arguments, *backend)
        });
        match prepare_generated(device, mesh, pool, scratch.as_deref_mut()) {
            Ok(mesh) => pending.push(mesh),
            Err(error) => {
                discard_pending_generated(
                    device, pending, arena.as_mut().map(|(faces, templates, arguments, _)| (&mut **faces, &mut **templates, &mut **arguments)),
                );
                return Err(error);
            }
        }
    }
    let prepare_ms = trace_started
        .map(|start| start.elapsed().as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    // One native upload submission covers every Section's mesher inputs;
    // per-Section write batches would each cost a full command submission.
    let staging_writes = pending
        .iter()
        .flat_map(|mesh| mesh.staging.iter())
        .map(|write| BufferWrite {
            descriptor: write.descriptor,
            data: &write.data,
        })
        .collect::<Vec<_>>();
    if !staging_writes.is_empty() {
        let bytes = staging_writes.iter().map(|write| write.data.len()).sum::<usize>();
        let started = crate::gpu_submission_trace::enabled().then(Instant::now);
        if let Some(batch_id) = batch_id {
            device.write_buffer_batch_profiled(
                &staging_writes,
                gfx_core::GpuTimestampTag {
                    batch_id,
                    stage: gfx_core::GpuTimestampStage::BufferUpload,
                },
            )
        } else {
            device.write_buffer_batch(&staging_writes)
        }.map_err(anyhow::Error::from)?;
        if let (Some(start), Some((_, _, _, backend))) = (started, arena.as_ref()) {
            let duration = start.elapsed();
            crate::gpu_submission_trace::observe(
                *backend,
                crate::gpu_submission_trace::Phase::GeneratedInputUpload,
                staging_writes.len(), bytes, duration,
            );
            // Include generated input staging in the budget's existing
            // GPU-owner buffer-write feedback; no extra driver call.
            crate::upload_feedback::observe_buffer_upload(
                *backend, bytes, staging_writes.len(), duration,
            );
        }
    }
    drop(staging_writes);
    for mesh in &mut pending {
        mesh.staging = Vec::new();
    }
    let mut dispatches = Vec::new();
    for mesh in &pending {
        for &(stage, workgroups) in &mesh.stages {
            dispatches.push(ComputeDispatch {
                shader: shaders.0[stage],
                bindings: &mesh.bindings,
                workgroups,
            });
        }
    }
    let dispatch_count = dispatches.len();
    let dispatch_started = (trace_started.is_some() || crate::gpu_submission_trace::enabled())
        .then(Instant::now);
    let submitted = if dispatches.is_empty() {
        Ok(())
    } else {
        (if let Some(batch_id) = batch_id {
            device.dispatch_compute_batch_profiled(
                &dispatches,
                gfx_core::GpuTimestampTag {
                    batch_id,
                    stage: gfx_core::GpuTimestampStage::GeneratedMesh,
                },
            )
        } else {
            device.dispatch_compute_batch(&dispatches)
        }).map_err(anyhow::Error::from)
    };
    let dispatch_elapsed = dispatch_started.map(|start| start.elapsed()).unwrap_or_default();
    let dispatch_ms = dispatch_elapsed.as_secs_f64() * 1000.0;
    drop(dispatches);
    if submitted.is_ok() && dispatch_count != 0 {
        if let Some((_, _, _, backend)) = arena.as_ref() {
            crate::gpu_submission_trace::observe(
                *backend, crate::gpu_submission_trace::Phase::GeneratedComputeIssue,
                dispatch_count, meshes.len(), dispatch_elapsed,
            );
        }
    }
    if let Err(error) = submitted {
        discard_pending_generated(
            device, pending, arena.as_mut().map(|(faces, templates, arguments, _)| (&mut **faces, &mut **templates, &mut **arguments)),
        );
        return Err(error);
    }
    // All compute reads of the source buffers have been ordered ahead of
    // retiring their scratch allocations. Each Section retains only its
    // generated faces, template table and native draw argument buffer.
    let mut owned_scratch = Vec::new();
    for mesh in &pending {
        for slot in [0usize, 4, 5] {
            if !mesh.shared_scratch[slot] {
                owned_scratch.push(mesh.buffers[slot]);
            }
        }
    }
    if let Err(error) = device.destroy_buffer_batch(&owned_scratch) {
        discard_pending_generated(
            device, pending, arena.as_mut().map(|(faces, templates, arguments, _)| (&mut **faces, &mut **templates, &mut **arguments)),
        );
        return Err(error.into());
    }
    if let Some(started) = trace_started {
        let total_ms = started.elapsed().as_secs_f64() * 1000.0;
        if total_ms >= 16.0 {
            eprintln!(
                "gpui3d generated_batch backend={:?} meshes={} dispatches={} scratch_buffers={} prepare_ms={:.2} dispatch_ms={:.2} retire_ms={:.2} total_ms={:.2}",
                arena.as_ref().map(|(_, _, _, backend)| *backend),
                meshes.len(),
                dispatch_count,
                owned_scratch.len(),
                prepare_ms,
                dispatch_ms,
                (total_ms - prepare_ms - dispatch_ms).max(0.0),
                total_ms,
            );
        }
    }
    Ok(pending.into_iter().map(PendingGenerated::into_resources).collect())
}

pub(super) fn create(
    device: &mut dyn ExtensionDevice,
    mesh: &Mesh,
    shaders: &Shaders,
) -> gpui::Result<MeshResources> {
    create_many(device, &[mesh], shaders)?
        .pop()
        .ok_or_else(|| anyhow!("missing generated Section output"))
}

#[cfg(test)]
mod material_part_tests {
    use super::*;

    #[test]
    fn template_material_lookup_matches_original_first_part_semantics() {
        let mut source = crate::voxel_mesh::tests::source();
        let mut faces = source.templates.to_vec();
        faces[0].material_slot = 7;
        faces[1].material_slot = 3;
        faces[2].material_slot = 7;
        faces[3].material_slot = 99;
        // Rebuild the source via the public API so material parts are validated.
        source = VoxelMeshSource::new(source.size, source.origin, source.cells.to_vec(), faces)
            .expect("valid source");
        let lookup = material_part_lookup(&source);
        for face in source.templates.iter() {
            assert_eq!(
                lookup.get(&face.material_slot).copied().unwrap_or(0),
                source.parts.iter().position(|part| part.material_slot() == face.material_slot)
                    .unwrap_or(0),
            );
        }
        assert_eq!(lookup.get(&u32::MAX), None);
    }
}
