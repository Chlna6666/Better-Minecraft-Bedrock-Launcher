use super::*;

impl NovaRenderer {
    /// Returns the promoted 3D cache to its tiny startup buffers after an aggressive trim.
    ///
    /// The paged allocator already releases logical spans, but keeping the promoted vertex and
    /// index buffers alive would retain the full GPU/upload-heap allocation after the 3D preview
    /// closes. Demotion is deliberately restricted to a drained, empty cache so no submitted
    /// command can still reference the old buffers.
    pub(super) fn demote_custom_mesh_3d_buffers_if_idle(
        &mut self,
        level: GpuiMemoryTrimLevel,
    ) -> Result<bool> {
        if !matches!(level, GpuiMemoryTrimLevel::Aggressive)
            || !self.custom_mesh_3d_buffers_ready
            || !self.frame_upload.custom_mesh_3d_meshes.is_empty()
            || !self.custom_mesh_3d_mesh_cache.is_empty()
            || !self.pending_submissions.is_empty()
        {
            return Ok(false);
        }

        let layout = self.custom_mesh_3d_resource_set_layout;
        let old_vertices = self.custom_mesh_3d_vertices_buffer;
        let old_indices = self.custom_mesh_3d_indices_buffer;
        let (vertices, indices) = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => demote_custom_mesh_3d_buffers_on_device(
                device,
                layout,
                &mut self.frame_resources,
                old_vertices,
                old_indices,
            )?,
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => demote_custom_mesh_3d_buffers_on_device(
                device,
                layout,
                &mut self.frame_resources,
                old_vertices,
                old_indices,
            )?,
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => demote_custom_mesh_3d_buffers_on_device(
                device,
                layout,
                &mut self.frame_resources,
                old_vertices,
                old_indices,
            )?,
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                return Err(anyhow::anyhow!(
                    "nova-gfx renderer requires an explicit nova-gfx backend feature"
                ));
            }
        };

        self.custom_mesh_3d_vertices_buffer = vertices;
        self.custom_mesh_3d_indices_buffer = indices;
        if let Some(current) = self.frame_resources.get(self.current_frame_resource_index) {
            self.custom_mesh_3d_resource_set = current.resource_sets.custom_mesh_3d_resource_set;
        }
        self.custom_mesh_3d_buffers_ready = false;
        self.custom_mesh_3d_vertex_cursor = 0;
        self.custom_mesh_3d_index_cursor = 0;
        self.custom_mesh_3d_vertex_upload_scratch.shrink_to_fit();
        self.custom_mesh_3d_index_upload_scratch.shrink_to_fit();
        tracing::debug!(
            placeholder_vertices = CUSTOM_MESH_3D_PLACEHOLDER_VERTICES,
            placeholder_indices = CUSTOM_MESH_3D_PLACEHOLDER_INDICES,
            "nova custom 3D mesh buffers demoted"
        );
        Ok(true)
    }
}

fn demote_custom_mesh_3d_buffers_on_device<D>(
    device: &mut D,
    layout: ResourceSetLayoutId,
    frame_resources: &mut [NovaFrameResources],
    old_vertices: BufferId,
    old_indices: BufferId,
) -> Result<(BufferId, BufferId)>
where
    D: BackendResources,
{
    let vertices = create_custom_mesh_3d_vertices_buffer(
        device,
        "nova renderer idle placeholder",
        CUSTOM_MESH_3D_PLACEHOLDER_VERTICES,
    )?;
    let indices = create_custom_mesh_3d_indices_buffer(
        device,
        "nova renderer idle placeholder",
        CUSTOM_MESH_3D_PLACEHOLDER_INDICES,
    )?;

    let mut replacement_sets = Vec::with_capacity(frame_resources.len());
    for (index, frame) in frame_resources.iter().enumerate() {
        match create_custom_mesh_3d_resource_set(
            device,
            &format!("nova renderer idle frame {index}"),
            layout,
            frame.buffers.global_buffer,
            frame.buffers.custom_mesh_3d_parameters_buffer,
            vertices,
            CUSTOM_MESH_3D_PLACEHOLDER_VERTICES,
        ) {
            Ok(resource_set) => replacement_sets.push(resource_set),
            Err(error) => {
                for resource_set in replacement_sets {
                    let _ = device.destroy_resource_set(resource_set);
                }
                let _ = device.destroy_buffer(vertices);
                let _ = device.destroy_buffer(indices);
                return Err(error);
            }
        }
    }

    for (frame, resource_set) in frame_resources.iter_mut().zip(replacement_sets) {
        let previous = std::mem::replace(
            &mut frame.resource_sets.custom_mesh_3d_resource_set,
            resource_set,
        );
        device.destroy_resource_set(previous)?;
    }
    device.destroy_buffer(old_vertices)?;
    device.destroy_buffer(old_indices)?;
    Ok((vertices, indices))
}
