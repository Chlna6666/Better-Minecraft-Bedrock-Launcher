use super::*;

impl NovaRenderer {
    pub(super) fn ensure_custom_mesh_3d_cache_for_current_backend(&mut self) -> Result<()> {
        self.custom_mesh_3d_uploaded_bytes_this_frame = 0;
        if self.frame_upload.custom_mesh_3d_meshes.is_empty() {
            return Ok(());
        }

        let meshes = self.frame_upload.custom_mesh_3d_meshes.clone();
        if self.custom_mesh_3d_frame_has_stale_generation(&meshes)
            || !self.custom_mesh_3d_missing_meshes_fit(&meshes)
        {
            self.clear_custom_mesh_3d_cache();
        }

        if !self.custom_mesh_3d_missing_meshes_fit(&meshes) {
            anyhow::bail!("custom 3D mesh cache capacity exceeded by current frame");
        }

        for mesh in meshes {
            if self
                .custom_mesh_3d_cache_entry(mesh.id, mesh.generation)
                .is_some()
            {
                continue;
            }
            self.upload_custom_mesh_3d_to_cache(mesh.as_ref())?;
        }

        Ok(())
    }

    pub(super) fn custom_mesh_3d_cache_entry(
        &self,
        mesh_id: GpuMesh3dId,
        generation: u64,
    ) -> Option<NovaMeshCacheEntry> {
        self.custom_mesh_3d_mesh_cache
            .get(&mesh_id)
            .copied()
            .filter(|entry| entry.generation == generation)
    }

    pub(super) fn custom_mesh_3d_retained_bytes(&self) -> usize {
        self.custom_mesh_3d_vertex_cursor
            .saturating_mul(PACKED_CUSTOM_MESH_3D_VERTEX_BYTES)
            .saturating_add(
                self.custom_mesh_3d_index_cursor
                    .saturating_mul(PACKED_CUSTOM_MESH_3D_INDEX_BYTES),
            )
    }

    pub(super) fn custom_mesh_3d_buffer_count(&self) -> usize {
        if self.custom_mesh_3d_mesh_cache.is_empty() {
            0
        } else {
            2
        }
    }

    pub(super) fn trim_custom_mesh_3d_cache(&mut self, level: GpuiMemoryTrimLevel) {
        if matches!(
            level,
            GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive
        ) && self.frame_upload.custom_mesh_3d_meshes.is_empty()
        {
            self.clear_custom_mesh_3d_cache();
        }

        let multiplier = match level {
            GpuiMemoryTrimLevel::Light => 16,
            GpuiMemoryTrimLevel::Moderate => 8,
            GpuiMemoryTrimLevel::Aggressive => 1,
        };
        trim_custom_mesh_upload_scratch(
            &mut self.custom_mesh_3d_vertex_upload_scratch,
            256 * PACKED_CUSTOM_MESH_3D_VERTEX_BYTES,
            multiplier,
        );
        trim_custom_mesh_upload_scratch(
            &mut self.custom_mesh_3d_index_upload_scratch,
            512 * PACKED_CUSTOM_MESH_3D_INDEX_BYTES,
            multiplier,
        );
    }

    fn custom_mesh_3d_frame_has_stale_generation(&self, meshes: &[Arc<GpuMesh3d>]) -> bool {
        meshes.iter().any(|mesh| {
            self.custom_mesh_3d_mesh_cache
                .get(&mesh.id)
                .is_some_and(|entry| entry.generation != mesh.generation)
        })
    }

    fn custom_mesh_3d_missing_meshes_fit(&self, meshes: &[Arc<GpuMesh3d>]) -> bool {
        let mut vertex_cursor = self.custom_mesh_3d_vertex_cursor;
        let mut index_cursor = self.custom_mesh_3d_index_cursor;
        for mesh in meshes {
            if self
                .custom_mesh_3d_cache_entry(mesh.id, mesh.generation)
                .is_some()
            {
                continue;
            }
            let Some(next_vertex_cursor) = vertex_cursor.checked_add(mesh.vertices.len()) else {
                return false;
            };
            let Some(next_index_cursor) = index_cursor.checked_add(mesh.indices.len()) else {
                return false;
            };
            if next_vertex_cursor > MAX_CUSTOM_MESH_3D_VERTICES
                || next_index_cursor > MAX_CUSTOM_MESH_3D_INDICES
            {
                return false;
            }
            vertex_cursor = next_vertex_cursor;
            index_cursor = next_index_cursor;
        }
        true
    }

    fn upload_custom_mesh_3d_to_cache(&mut self, mesh: &GpuMesh3d) -> Result<()> {
        let vertex_offset = u32::try_from(self.custom_mesh_3d_vertex_cursor)
            .context("custom 3D mesh vertex cache offset exceeds u32")?;
        let index_offset = u32::try_from(self.custom_mesh_3d_index_cursor)
            .context("custom 3D mesh index cache offset exceeds u32")?;
        let vertex_count = u32::try_from(mesh.vertices.len())
            .context("custom 3D mesh vertex count exceeds u32")?;
        let index_count =
            u32::try_from(mesh.indices.len()).context("custom 3D mesh index count exceeds u32")?;

        let mut vertex_bytes = std::mem::take(&mut self.custom_mesh_3d_vertex_upload_scratch);
        vertex_bytes.clear();
        vertex_bytes.reserve(
            mesh.vertices
                .len()
                .saturating_mul(PACKED_CUSTOM_MESH_3D_VERTEX_BYTES),
        );
        for vertex in mesh.vertices.iter().copied() {
            write_custom_mesh_3d_vertex(&mut vertex_bytes, vertex);
        }
        let mut index_bytes = std::mem::take(&mut self.custom_mesh_3d_index_upload_scratch);
        index_bytes.clear();
        index_bytes.reserve(
            mesh.indices
                .len()
                .saturating_mul(PACKED_CUSTOM_MESH_3D_INDEX_BYTES),
        );
        for index in mesh.indices.iter().copied() {
            write_custom_mesh_3d_index(&mut index_bytes, index);
        }

        let vertex_byte_offset =
            (self.custom_mesh_3d_vertex_cursor * PACKED_CUSTOM_MESH_3D_VERTEX_BYTES) as u64;
        let index_byte_offset =
            (self.custom_mesh_3d_index_cursor * PACKED_CUSTOM_MESH_3D_INDEX_BYTES) as u64;
        let vertex_buffer = self.custom_mesh_3d_vertices_buffer;
        let index_buffer = self.custom_mesh_3d_indices_buffer;
        let upload_result: Result<()> = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => write_custom_mesh_3d_cache_buffers(
                device,
                vertex_buffer,
                index_buffer,
                vertex_byte_offset,
                index_byte_offset,
                &vertex_bytes,
                &index_bytes,
            ),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => write_custom_mesh_3d_cache_buffers(
                device,
                vertex_buffer,
                index_buffer,
                vertex_byte_offset,
                index_byte_offset,
                &vertex_bytes,
                &index_bytes,
            ),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => write_custom_mesh_3d_cache_buffers(
                device,
                vertex_buffer,
                index_buffer,
                vertex_byte_offset,
                index_byte_offset,
                &vertex_bytes,
                &index_bytes,
            ),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => Err(anyhow::anyhow!(
                "nova-gfx renderer requires an explicit nova-gfx backend feature"
            )),
        };
        if let Err(error) = upload_result {
            self.custom_mesh_3d_vertex_upload_scratch = vertex_bytes;
            self.custom_mesh_3d_index_upload_scratch = index_bytes;
            return Err(error);
        }

        self.custom_mesh_3d_uploaded_bytes_this_frame = self
            .custom_mesh_3d_uploaded_bytes_this_frame
            .saturating_add(vertex_bytes.len())
            .saturating_add(index_bytes.len());
        self.custom_mesh_3d_mesh_cache.insert(
            mesh.id,
            NovaMeshCacheEntry {
                generation: mesh.generation,
                vertex_offset,
                vertex_count,
                index_offset,
                index_count,
            },
        );
        self.custom_mesh_3d_vertex_cursor = self
            .custom_mesh_3d_vertex_cursor
            .saturating_add(mesh.vertices.len());
        self.custom_mesh_3d_index_cursor = self
            .custom_mesh_3d_index_cursor
            .saturating_add(mesh.indices.len());
        self.custom_mesh_3d_vertex_upload_scratch = vertex_bytes;
        self.custom_mesh_3d_index_upload_scratch = index_bytes;
        Ok(())
    }

    fn clear_custom_mesh_3d_cache(&mut self) {
        self.custom_mesh_3d_mesh_cache.clear();
        self.custom_mesh_3d_vertex_cursor = 0;
        self.custom_mesh_3d_index_cursor = 0;
    }
}

fn trim_custom_mesh_upload_scratch(vec: &mut Vec<u8>, floor: usize, multiplier: usize) {
    let target = floor.max(1);
    if vec.capacity() > target.saturating_mul(multiplier.max(1)) {
        vec.shrink_to(target);
    }
}

fn write_custom_mesh_3d_cache_buffers<D>(
    device: &mut D,
    vertex_buffer: BufferId,
    index_buffer: BufferId,
    vertex_byte_offset: u64,
    index_byte_offset: u64,
    vertex_bytes: &[u8],
    index_bytes: &[u8],
) -> Result<()>
where
    D: BackendResources,
{
    device.write_buffer(vertex_buffer, vertex_byte_offset, vertex_bytes)?;
    device.write_buffer(index_buffer, index_byte_offset, index_bytes)?;
    Ok(())
}
