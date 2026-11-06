//! Compute-generated face descriptors, retained templates and native draw arguments.
use super::{artifacts, gpu::{ArenaAllocation, MeshArena, MeshResources}};
use crate::{Mesh, VoxelMeshSource};
use anyhow::{Context as _, anyhow};
use gfx_core::*;
use gpui::ExtensionDevice;
use std::collections::HashMap;

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
    let mut bytes = Vec::with_capacity(32 + source.templates.len() * 112);
    for value in [
        source.origin.x,
        source.origin.y,
        source.origin.z,
        source.lighting.as_ref().map_or(0., |light| light.ambient),
    ] {
        bytes.extend(value.to_le_bytes());
    }
    for value in source.size.into_iter().chain([0]) {
        bytes.extend(value.to_le_bytes());
    }
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
        bytes[28..32].copy_from_slice(&base.to_le_bytes());
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
    vertex_base: u32,
    vertex_binding_bytes: usize,
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
            }),
        }
    }
}

fn discard_pending_generated(
    device: &mut dyn ExtensionDevice,
    pending: Vec<PendingGenerated>,
    mut arena: Option<&mut MeshArena>,
) {
    for mesh in pending {
        for (slot, &buffer) in mesh.buffers.iter().enumerate() {
            if (slot != 2 || mesh.allocation.is_none())
                && (slot != 1 || mesh.template_allocation.is_none())
            {
                let _ = device.destroy_buffer(buffer);
            }
        }
        if let Some(arena) = arena.as_deref_mut() {
            if let Some(allocation) = mesh.template_allocation {
                let _ = arena.release(device, allocation);
            }
            if let Some(allocation) = mesh.allocation {
                let _ = arena.release(device, allocation);
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
    mut arena: Option<(&mut MeshArena, BackendKind)>,
) -> gpui::Result<PendingGenerated> {
    let source = mesh
        .gpu_voxels()
        .ok_or_else(|| anyhow!("missing voxel source"))?;
    let (templates, static_counts) = encode(source);
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
    let mut pooled_allocation = None;
    let mut template_allocation = None;
    let mut template_offset = 0u64;
    let mut pooled_offset = 0u64;
    let mut pooled_capacity = output_bytes;
    let result = (|| {
        for (slot, (size, usage)) in specs.into_iter().enumerate() {
            if (slot == 1 || slot == 2) && let Some((pool, backend)) = arena.as_mut() {
                let (allocation, buffer, offset, capacity) =
                    pool.allocate(device, *backend, size as u64)?;
                // Both immutable template inputs and generated face outputs
                // live in GPU-owner shared pages, but occupy disjoint slices.
                // Compute uses offset-qualified bindings while the shader
                // continues to address each Section relative to its slice.
                if slot == 1 {
                    template_allocation = Some(allocation);
                    template_offset = offset;
                } else {
                    pooled_allocation = Some(allocation);
                    pooled_offset = offset;
                    pooled_capacity = usize::try_from(capacity)
                        .context("generated face page exceeds host address space")?;
                    if offset / 16 >= 0x8000_0000 {
                        return Err(anyhow!("generated face pool exceeds vertex address range"));
                    }
                }
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
        let writes =
            [(0, &cells), (1, &templates), (3, &args), (4, &params)].map(|(slot, data)| {
                BufferWrite {
                    descriptor: BufferWriteDescriptor {
                        buffer: buffers[slot],
                        offset: if slot == 1 { template_offset } else { 0 },
                    },
                    data: data.as_slice(),
                }
            });
        device.write_buffer_batch(&writes)?;
        let bindings = specs
            .iter()
            .enumerate()
            .map(|(slot, (size, _))| ComputeBufferBinding {
                binding: slot as u32,
                buffer: BufferBinding {
                    buffer: buffers[slot],
                    offset: match slot {
                        1 => template_offset,
                        2 => pooled_offset,
                        _ => 0,
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
        Ok::<_, anyhow::Error>((bindings, stage_grids))
    })();
    let (bindings, stages) = match result {
        Ok(prepared) => prepared,
        Err(error) => {
            for (slot, buffer) in buffers.into_iter().enumerate() {
                if (slot != 2 || pooled_allocation.is_none())
                    && (slot != 1 || template_allocation.is_none())
                {
                    let _ = device.destroy_buffer(buffer);
                }
            }
            if let Some((pool, _)) = arena {
                if let Some(allocation) = template_allocation {
                    let _ = pool.release(device, allocation);
                }
                if let Some(allocation) = pooled_allocation {
                    let _ = pool.release(device, allocation);
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
        allocation: pooled_allocation,
        template_allocation,
        template_offset,
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
    create_many_impl(device, meshes, shaders, None)
}

/// Production GPU meshing suballocates output descriptors into shared writable
/// storage pages. Existing test helpers retain a standalone variant above.
pub(super) fn create_many_pooled(
    device: &mut dyn ExtensionDevice,
    meshes: &[&Mesh],
    shaders: &Shaders,
    arena: &mut MeshArena,
    backend: BackendKind,
) -> gpui::Result<Vec<MeshResources>> {
    create_many_impl(device, meshes, shaders, Some((arena, backend)))
}

fn create_many_impl(
    device: &mut dyn ExtensionDevice,
    meshes: &[&Mesh],
    shaders: &Shaders,
    mut arena: Option<(&mut MeshArena, BackendKind)>,
) -> gpui::Result<Vec<MeshResources>> {
    if meshes.is_empty() {
        return Ok(Vec::new());
    }
    let mut pending = Vec::with_capacity(meshes.len());
    for mesh in meshes {
        let pool = arena.as_mut().map(|(arena, backend)| (&mut **arena, *backend));
        match prepare_generated(device, mesh, pool) {
            Ok(mesh) => pending.push(mesh),
            Err(error) => {
                discard_pending_generated(
                    device, pending, arena.as_mut().map(|(arena, _)| &mut **arena),
                );
                return Err(error);
            }
        }
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
    let submitted = if dispatches.is_empty() {
        Ok(())
    } else {
        device.dispatch_compute_batch(&dispatches).map_err(anyhow::Error::from)
    };
    drop(dispatches);
    if let Err(error) = submitted {
        discard_pending_generated(
            device, pending, arena.as_mut().map(|(arena, _)| &mut **arena),
        );
        return Err(error);
    }
    // All compute reads of the source buffers have been ordered ahead of
    // retiring their scratch allocations. Each Section retains only its
    // generated faces, template table and native draw argument buffer.
    let mut scratch = Vec::with_capacity(pending.len() * 3);
    for mesh in &pending {
        scratch.extend([mesh.buffers[0], mesh.buffers[4], mesh.buffers[5]]);
    }
    if let Err(error) = device.destroy_buffer_batch(&scratch) {
        discard_pending_generated(
            device, pending, arena.as_mut().map(|(arena, _)| &mut **arena),
        );
        return Err(error.into());
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
