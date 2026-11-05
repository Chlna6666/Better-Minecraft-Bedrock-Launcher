//! Compute-generated face descriptors, retained templates and native draw arguments.
use super::{artifacts, gpu::MeshResources};
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

pub(super) fn create(
    device: &mut dyn ExtensionDevice,
    mesh: &Mesh,
    shaders: &Shaders,
) -> gpui::Result<MeshResources> {
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
    let cells = words
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
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
    let result = (|| {
        for (size, usage) in specs {
            buffers.push(device.create_buffer(&BufferDescriptor {
                label: Some("GPU voxel meshing".into()),
                size: size as u64,
                usage,
                memory_location: MemoryLocation::GpuOnly,
            })?);
        }
        let writes =
            [(0, &cells), (1, &templates), (3, &args), (4, &params)].map(|(slot, data)| {
                BufferWrite {
                    descriptor: BufferWriteDescriptor {
                        buffer: buffers[slot],
                        offset: 0,
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
                    offset: 0,
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
        let dispatches = stages
            .into_iter()
            .map(|(stage, count)| {
                let count = u32::try_from(count).context("voxel dispatch overflow")?;
                let width = count.min(device.compute_capabilities().max_workgroups[0]);
                Ok(ComputeDispatch {
                    shader: shaders.0[stage],
                    bindings: &bindings,
                    workgroups: [width, count.div_ceil(width), 1],
                })
            })
            .collect::<gpui::Result<Vec<_>>>()?;
        device.dispatch_compute_batch(&dispatches)?;
        Ok::<_, anyhow::Error>(())
    })();
    if let Err(error) = result {
        for buffer in buffers {
            let _ = device.destroy_buffer(buffer);
        }
        return Err(error);
    }
    let generated = GeneratedMesh {
        templates: buffers[1],
        arguments: buffers[3],
        template_bytes: templates.len() as u64,
    };
    if let Err(error) = device.destroy_buffer_batch(&[buffers[0], buffers[4], buffers[5]]) {
        for index in [0, 1, 2, 3, 4, 5] {
            let _ = device.destroy_buffer(buffers[index]);
        }
        return Err(error.into());
    }
    Ok(MeshResources {
        generation: mesh.generation(),
        vertices: buffers[2],
        vertex_binding_bytes: output_bytes,
        vertex_stride: 16,
        vertex_base: 0,
        tangents: None,
        tangent_bytes: 0,
        indices: buffers[3],
        index_format: IndexFormat::Uint16,
        index_offset: 0,
        procedural_quads: true,
        allocation: None,
        generated: Some(generated),
    })
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
