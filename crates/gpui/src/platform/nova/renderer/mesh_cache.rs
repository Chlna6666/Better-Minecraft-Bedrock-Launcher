use super::*;
use std::sync::{Mutex, OnceLock};

const INDEX_FORMAT_U16_FLAG: u32 = 1 << 31;
const MESH_CACHE_UNUSED_EPOCHS: u64 = 240;
const INDEX_BUFFER_CAPACITY_BYTES: usize =
    MAX_CUSTOM_MESH_3D_INDICES * PACKED_CUSTOM_MESH_3D_INDEX_BYTES;

type SurfaceMeshAllocatorKey = (usize, u64);

#[derive(Clone, Copy, Debug, Default)]
struct MeshFreeSpan {
    offset: usize,
    len: usize,
}

#[derive(Clone, Copy, Debug)]
struct MeshAllocation {
    entry: MeshCacheEntry,
    vertex_capacity: usize,
    index_byte_offset: usize,
    index_byte_capacity: usize,
    last_used_epoch: u64,
}

#[derive(Clone, Copy, Debug)]
struct RetiredMeshAllocation {
    vertex_offset: usize,
    vertex_capacity: usize,
    index_byte_offset: usize,
    index_byte_capacity: usize,
}

#[derive(Debug)]
struct SurfaceMeshPageAllocator {
    epoch: u64,
    allocations: FxHashMap<GpuMesh3dId, MeshAllocation>,
    free_vertices: Vec<MeshFreeSpan>,
    free_index_bytes: Vec<MeshFreeSpan>,
    retired: Vec<RetiredMeshAllocation>,
    evictions: u64,
    compactions: u64,
}

impl Default for SurfaceMeshPageAllocator {
    fn default() -> Self {
        Self {
            epoch: 0,
            allocations: FxHashMap::default(),
            free_vertices: vec![MeshFreeSpan {
                offset: 0,
                len: MAX_CUSTOM_MESH_3D_VERTICES,
            }],
            free_index_bytes: vec![MeshFreeSpan {
                offset: 0,
                len: INDEX_BUFFER_CAPACITY_BYTES,
            }],
            retired: Vec::new(),
            evictions: 0,
            compactions: 0,
        }
    }
}

impl SurfaceMeshPageAllocator {
    fn begin_frame(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
    }

    fn allocation(&self, id: GpuMesh3dId, generation: u64) -> Option<MeshAllocation> {
        self.allocations
            .get(&id)
            .copied()
            .filter(|allocation| allocation.entry.generation == generation)
    }

    fn mark_used(&mut self, id: GpuMesh3dId) {
        if let Some(allocation) = self.allocations.get_mut(&id) {
            allocation.last_used_epoch = self.epoch;
        }
    }

    fn reserve(&mut self, vertex_count: usize, index_byte_count: usize) -> Option<(usize, usize)> {
        let vertex_offset = allocate_best_fit(&mut self.free_vertices, vertex_count, 1)?;
        let Some(index_byte_offset) =
            allocate_best_fit(&mut self.free_index_bytes, index_byte_count, 4)
        else {
            insert_free_span(
                &mut self.free_vertices,
                MeshFreeSpan {
                    offset: vertex_offset,
                    len: vertex_count,
                },
            );
            return None;
        };
        Some((vertex_offset, index_byte_offset))
    }

    fn commit(
        &mut self,
        id: GpuMesh3dId,
        entry: MeshCacheEntry,
        vertex_capacity: usize,
        index_byte_offset: usize,
        index_byte_capacity: usize,
    ) {
        self.allocations.insert(
            id,
            MeshAllocation {
                entry,
                vertex_capacity,
                index_byte_offset,
                index_byte_capacity,
                last_used_epoch: self.epoch,
            },
        );
    }

    fn release_reservation(
        &mut self,
        vertex_offset: usize,
        vertex_count: usize,
        index_byte_offset: usize,
        index_byte_count: usize,
    ) {
        insert_free_span(
            &mut self.free_vertices,
            MeshFreeSpan {
                offset: vertex_offset,
                len: vertex_count,
            },
        );
        insert_free_span(
            &mut self.free_index_bytes,
            MeshFreeSpan {
                offset: index_byte_offset,
                len: index_byte_count,
            },
        );
    }

    fn remove(&mut self, id: GpuMesh3dId, defer_release: bool) -> Option<MeshAllocation> {
        let allocation = self.allocations.remove(&id)?;
        if defer_release {
            self.retired.push(RetiredMeshAllocation {
                vertex_offset: allocation.entry.vertex_offset as usize,
                vertex_capacity: allocation.vertex_capacity,
                index_byte_offset: allocation.index_byte_offset,
                index_byte_capacity: allocation.index_byte_capacity,
            });
        } else {
            self.release_allocation(allocation);
        }
        Some(allocation)
    }

    fn release_allocation(&mut self, allocation: MeshAllocation) {
        self.release_reservation(
            allocation.entry.vertex_offset as usize,
            allocation.vertex_capacity,
            allocation.index_byte_offset,
            allocation.index_byte_capacity,
        );
    }

    fn reclaim_retired(&mut self) {
        for allocation in std::mem::take(&mut self.retired) {
            self.release_reservation(
                allocation.vertex_offset,
                allocation.vertex_capacity,
                allocation.index_byte_offset,
                allocation.index_byte_capacity,
            );
        }
    }

    fn evict_unused(
        &mut self,
        current: &FxHashSet<GpuMesh3dId>,
        aggressive: bool,
    ) -> Vec<GpuMesh3dId> {
        let epoch = self.epoch;
        let mut candidates = self
            .allocations
            .iter()
            .filter_map(|(id, allocation)| {
                if current.contains(id) {
                    return None;
                }
                let age = epoch.wrapping_sub(allocation.last_used_epoch);
                (aggressive || age >= MESH_CACHE_UNUSED_EPOCHS)
                    .then_some((*id, allocation.last_used_epoch))
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|(_, last_used_epoch)| *last_used_epoch);
        let mut evicted = Vec::with_capacity(candidates.len());
        for (id, _) in candidates {
            if let Some(allocation) = self.allocations.remove(&id) {
                self.release_allocation(allocation);
                self.evictions = self.evictions.saturating_add(1);
                evicted.push(id);
            }
        }
        evicted
    }

    fn reset(&mut self) {
        let next_epoch = self.epoch.wrapping_add(1);
        let evictions = self.evictions;
        let compactions = self.compactions.saturating_add(1);
        *self = Self::default();
        self.epoch = next_epoch;
        self.evictions = evictions;
        self.compactions = compactions;
    }

    fn used_vertex_count(&self) -> usize {
        self.allocations
            .values()
            .map(|allocation| allocation.vertex_capacity)
            .sum()
    }

    fn used_index_bytes(&self) -> usize {
        self.allocations
            .values()
            .map(|allocation| allocation.index_byte_capacity)
            .sum()
    }

    fn free_vertex_count(&self) -> usize {
        self.free_vertices.iter().map(|span| span.len).sum()
    }

    fn free_index_bytes(&self) -> usize {
        self.free_index_bytes.iter().map(|span| span.len).sum()
    }

    fn largest_free_vertex_span(&self) -> usize {
        self.free_vertices
            .iter()
            .map(|span| span.len)
            .max()
            .unwrap_or(0)
    }

    fn largest_free_index_span(&self) -> usize {
        self.free_index_bytes
            .iter()
            .map(|span| span.len)
            .max()
            .unwrap_or(0)
    }
}

fn surface_mesh_allocators()
-> &'static Mutex<FxHashMap<SurfaceMeshAllocatorKey, SurfaceMeshPageAllocator>> {
    static ALLOCATORS: OnceLock<
        Mutex<FxHashMap<SurfaceMeshAllocatorKey, SurfaceMeshPageAllocator>>,
    > = OnceLock::new();
    ALLOCATORS.get_or_init(|| Mutex::new(FxHashMap::default()))
}

fn surface_mesh_allocator_key(renderer: &NovaRenderer) -> SurfaceMeshAllocatorKey {
    // nova-gfx resource IDs are only unique inside one logical device. Every GPUI window owns
    // its own Nova renderer/device, so two windows can both have SurfaceId(raw=0/1). Using only
    // surface.raw() in the process-global allocator map lets one window clear/reuse another
    // window's live vertex/index spans. The atlas Arc is stable for the renderer lifetime and is
    // created per window, making its address an efficient renderer namespace without adding a
    // second allocation or another atomic ID to the hot path.
    (Arc::as_ptr(&renderer.atlas).addr(), renderer.surface.raw())
}

fn allocate_best_fit(spans: &mut Vec<MeshFreeSpan>, len: usize, alignment: usize) -> Option<usize> {
    if len == 0 {
        return Some(0);
    }
    let alignment = alignment.max(1);
    let mut best: Option<(usize, usize, usize)> = None;
    for (index, span) in spans.iter().copied().enumerate() {
        let aligned_offset = align_up(span.offset, alignment)?;
        let prefix = aligned_offset.checked_sub(span.offset)?;
        let required = prefix.checked_add(len)?;
        if required > span.len {
            continue;
        }
        let waste = span.len - required;
        if best.is_none_or(|(_, _, best_waste)| waste < best_waste) {
            best = Some((index, aligned_offset, waste));
        }
    }
    let (index, aligned_offset, _) = best?;
    let span = spans.swap_remove(index);
    let prefix = aligned_offset - span.offset;
    let suffix_offset = aligned_offset.checked_add(len)?;
    let suffix = span
        .offset
        .checked_add(span.len)?
        .checked_sub(suffix_offset)?;
    if prefix > 0 {
        spans.push(MeshFreeSpan {
            offset: span.offset,
            len: prefix,
        });
    }
    if suffix > 0 {
        spans.push(MeshFreeSpan {
            offset: suffix_offset,
            len: suffix,
        });
    }
    Some(aligned_offset)
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    let mask = alignment.checked_sub(1)?;
    value.checked_add(mask).map(|value| value & !mask)
}

fn insert_free_span(spans: &mut Vec<MeshFreeSpan>, span: MeshFreeSpan) {
    if span.len == 0 {
        return;
    }
    spans.push(span);
    spans.sort_unstable_by_key(|span| span.offset);
    let mut merged = Vec::<MeshFreeSpan>::with_capacity(spans.len());
    for span in spans.drain(..) {
        if let Some(last) = merged.last_mut() {
            let last_end = last.offset.saturating_add(last.len);
            if span.offset <= last_end {
                let next_end = span.offset.saturating_add(span.len);
                last.len = last_end.max(next_end).saturating_sub(last.offset);
                continue;
            }
        }
        merged.push(span);
    }
    *spans = merged;
}

fn index_uses_u16(mesh: &GpuMesh3d) -> bool {
    mesh.vertices.len() <= usize::from(u16::MAX)
        && mesh
            .indices
            .iter()
            .all(|index| *index <= u32::from(u16::MAX))
}

fn packed_index_offset(index_byte_offset: usize, uses_u16: bool) -> Result<u32> {
    let offset =
        u32::try_from(index_byte_offset).context("custom 3D mesh index byte offset exceeds u32")?;
    if offset & INDEX_FORMAT_U16_FLAG != 0 {
        anyhow::bail!("custom 3D mesh index byte offset exceeds packed offset range");
    }
    Ok(if uses_u16 {
        offset | INDEX_FORMAT_U16_FLAG
    } else {
        offset
    })
}

impl NovaRenderer {
    pub(super) fn ensure_custom_mesh_3d_cache_for_current_backend(&mut self) -> Result<()> {
        self.custom_mesh_3d_uploaded_bytes_this_frame = 0;
        let current_meshes = std::mem::take(&mut self.frame_upload.custom_mesh_3d_meshes);
        let current_ids = std::mem::take(&mut self.frame_upload.custom_mesh_3d_ids);
        let result = self.ensure_custom_mesh_3d_cache(&current_meshes, &current_ids);
        self.frame_upload.custom_mesh_3d_meshes = current_meshes;
        self.frame_upload.custom_mesh_3d_ids = current_ids;
        result
    }

    fn ensure_custom_mesh_3d_cache(
        &mut self,
        current_meshes: &[Arc<GpuMesh3d>],
        current_ids: &FxHashSet<GpuMesh3dId>,
    ) -> Result<()> {
        let surface_key = surface_mesh_allocator_key(self);

        if current_meshes.is_empty() {
            if self.pending_submissions.is_empty() {
                self.clear_custom_mesh_3d_cache();
                surface_mesh_allocators()
                    .lock()
                    .expect("nova 3D mesh allocator lock poisoned")
                    .remove(&surface_key);
                trim_custom_mesh_upload_scratch(
                    &mut self.custom_mesh_3d_vertex_upload_scratch,
                    256 * PACKED_CUSTOM_MESH_3D_VERTEX_BYTES,
                    1,
                );
                trim_custom_mesh_upload_scratch(
                    &mut self.custom_mesh_3d_index_upload_scratch,
                    512 * PACKED_CUSTOM_MESH_3D_INDEX_BYTES,
                    1,
                );
            }
            return Ok(());
        }

        if !self.custom_mesh_3d_buffers_ready {
            self.promote_custom_mesh_3d_buffers()?;
        }

        {
            let mut allocators = surface_mesh_allocators()
                .lock()
                .expect("nova 3D mesh allocator lock poisoned");
            let allocator = allocators.entry(surface_key).or_default();
            allocator.begin_frame();
            if self.pending_submissions.is_empty() {
                allocator.reclaim_retired();
                for id in allocator.evict_unused(current_ids, false) {
                    self.custom_mesh_3d_mesh_cache.remove(&id);
                }
            }
            for mesh in current_meshes {
                if allocator.allocation(mesh.id, mesh.generation).is_some() {
                    allocator.mark_used(mesh.id);
                }
            }
        }

        for mesh in current_meshes {
            if self
                .custom_mesh_3d_cache_entry(mesh.id, mesh.generation)
                .is_some()
            {
                continue;
            }
            self.replace_custom_mesh_3d_cache_entry(mesh, current_meshes, current_ids)?;
        }

        let allocator_snapshot = {
            let allocators = surface_mesh_allocators()
                .lock()
                .expect("nova 3D mesh allocator lock poisoned");
            let allocator = allocators
                .get(&surface_key)
                .expect("nova 3D mesh allocator should exist");
            (
                allocator.used_vertex_count(),
                allocator.used_index_bytes(),
                allocator.free_vertex_count(),
                allocator.free_index_bytes(),
                allocator.largest_free_vertex_span(),
                allocator.largest_free_index_span(),
                allocator.retired.len(),
                allocator.evictions,
                allocator.compactions,
            )
        };
        self.custom_mesh_3d_vertex_cursor = allocator_snapshot.0;
        self.custom_mesh_3d_index_cursor = allocator_snapshot.1 / PACKED_CUSTOM_MESH_3D_INDEX_BYTES;
        let fragmented_vertex_count = allocator_snapshot.2.saturating_sub(allocator_snapshot.4);
        let fragmented_index_bytes = allocator_snapshot.3.saturating_sub(allocator_snapshot.5);
        log::debug!(
            "nova custom 3D mesh paged cache: surface={}, allocator_key={surface_key:?}, current_meshes={}, cached_meshes={}, uploaded_bytes={}, used_vertices={}, used_index_bytes={}, free_vertices={}, free_index_bytes={}, largest_free_vertex_span={}, largest_free_index_span={}, fragmented_vertex_count={fragmented_vertex_count}, fragmented_index_bytes={fragmented_index_bytes}, retired_allocations={}, evictions={}, compactions={}",
            self.surface.raw(),
            current_meshes.len(),
            self.custom_mesh_3d_mesh_cache.len(),
            self.custom_mesh_3d_uploaded_bytes_this_frame,
            allocator_snapshot.0,
            allocator_snapshot.1,
            allocator_snapshot.2,
            allocator_snapshot.3,
            allocator_snapshot.4,
            allocator_snapshot.5,
            allocator_snapshot.6,
            allocator_snapshot.7,
            allocator_snapshot.8,
        );
        Ok(())
    }

    fn replace_custom_mesh_3d_cache_entry(
        &mut self,
        mesh: &Arc<GpuMesh3d>,
        current_meshes: &[Arc<GpuMesh3d>],
        current_ids: &FxHashSet<GpuMesh3dId>,
    ) -> Result<()> {
        let surface_key = surface_mesh_allocator_key(self);
        let uses_u16 = index_uses_u16(mesh);
        let index_stride = if uses_u16 { 2 } else { 4 };
        let vertex_count = mesh.vertices.len();
        let index_byte_count = mesh
            .indices
            .len()
            .checked_mul(index_stride)
            .context("custom 3D mesh index byte count overflow")?;

        let defer_old_release = !self.pending_submissions.is_empty();
        {
            let mut allocators = surface_mesh_allocators()
                .lock()
                .expect("nova 3D mesh allocator lock poisoned");
            let allocator = allocators.entry(surface_key).or_default();
            if allocator.remove(mesh.id, defer_old_release).is_some() {
                self.custom_mesh_3d_mesh_cache.remove(&mesh.id);
            }
        }

        let mut reservation = {
            let mut allocators = surface_mesh_allocators()
                .lock()
                .expect("nova 3D mesh allocator lock poisoned");
            let allocator = allocators.entry(surface_key).or_default();
            allocator.reserve(vertex_count, index_byte_count)
        };

        if reservation.is_none() {
            if !self.pending_submissions.is_empty() {
                self.wait_for_pending_submissions()?;
            }
            let mut allocators = surface_mesh_allocators()
                .lock()
                .expect("nova 3D mesh allocator lock poisoned");
            let allocator = allocators.entry(surface_key).or_default();
            allocator.reclaim_retired();
            for id in allocator.evict_unused(current_ids, true) {
                self.custom_mesh_3d_mesh_cache.remove(&id);
            }
            reservation = allocator.reserve(vertex_count, index_byte_count);
        }

        if reservation.is_none() {
            // Fragmentation can leave enough total free bytes but no sufficiently large span.
            // Compact only as the final fallback; normal updates never clear unrelated meshes.
            if !self.pending_submissions.is_empty() {
                self.wait_for_pending_submissions()?;
            }
            {
                let mut allocators = surface_mesh_allocators()
                    .lock()
                    .expect("nova 3D mesh allocator lock poisoned");
                allocators.entry(surface_key).or_default().reset();
            }
            self.custom_mesh_3d_mesh_cache.clear();
            for current in current_meshes {
                if current.id == mesh.id {
                    continue;
                }
                self.upload_custom_mesh_3d_with_new_allocation(current)?;
            }
            reservation = {
                let mut allocators = surface_mesh_allocators()
                    .lock()
                    .expect("nova 3D mesh allocator lock poisoned");
                allocators
                    .entry(surface_key)
                    .or_default()
                    .reserve(vertex_count, index_byte_count)
            };
        }

        let (vertex_offset, index_byte_offset) =
            reservation.context("custom 3D mesh paged cache capacity exceeded by current frame")?;
        let upload_result =
            self.upload_custom_mesh_3d_to_cache(mesh, vertex_offset, index_byte_offset, uses_u16);
        if let Err(error) = upload_result {
            surface_mesh_allocators()
                .lock()
                .expect("nova 3D mesh allocator lock poisoned")
                .entry(surface_key)
                .or_default()
                .release_reservation(
                    vertex_offset,
                    vertex_count,
                    index_byte_offset,
                    index_byte_count,
                );
            return Err(error);
        }
        Ok(())
    }

    fn upload_custom_mesh_3d_with_new_allocation(&mut self, mesh: &GpuMesh3d) -> Result<()> {
        let surface_key = surface_mesh_allocator_key(self);
        let uses_u16 = index_uses_u16(mesh);
        let index_stride = if uses_u16 { 2 } else { 4 };
        let index_byte_count = mesh
            .indices
            .len()
            .checked_mul(index_stride)
            .context("custom 3D mesh index byte count overflow")?;
        let reservation = surface_mesh_allocators()
            .lock()
            .expect("nova 3D mesh allocator lock poisoned")
            .entry(surface_key)
            .or_default()
            .reserve(mesh.vertices.len(), index_byte_count)
            .context("custom 3D mesh compaction capacity exceeded")?;
        self.upload_custom_mesh_3d_to_cache(mesh, reservation.0, reservation.1, uses_u16)
    }

    pub(super) fn custom_mesh_3d_cache_entry(
        &self,
        mesh_id: GpuMesh3dId,
        generation: u64,
    ) -> Option<MeshCacheEntry> {
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
            && self.pending_submissions.is_empty()
        {
            self.clear_custom_mesh_3d_cache();
            let surface_key = surface_mesh_allocator_key(self);
            surface_mesh_allocators()
                .lock()
                .expect("nova 3D mesh allocator lock poisoned")
                .remove(&surface_key);
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

    fn upload_custom_mesh_3d_to_cache(
        &mut self,
        mesh: &GpuMesh3d,
        vertex_offset: usize,
        index_byte_offset: usize,
        uses_u16: bool,
    ) -> Result<()> {
        let vertex_offset_u32 = u32::try_from(vertex_offset)
            .context("custom 3D mesh vertex cache offset exceeds u32")?;
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
        if uses_u16 {
            index_bytes.reserve(mesh.indices.len().saturating_mul(2));
            for index in mesh.indices.iter().copied() {
                let index = u16::try_from(index)
                    .context("custom 3D mesh index exceeds Uint16 after validation")?;
                index_bytes.extend_from_slice(&index.to_ne_bytes());
            }
        } else {
            index_bytes.reserve(
                mesh.indices
                    .len()
                    .saturating_mul(PACKED_CUSTOM_MESH_3D_INDEX_BYTES),
            );
            for index in mesh.indices.iter().copied() {
                write_custom_mesh_3d_index(&mut index_bytes, index);
            }
        }

        let vertex_byte_offset = (vertex_offset * PACKED_CUSTOM_MESH_3D_VERTEX_BYTES) as u64;
        let vertex_buffer = self.custom_mesh_3d_vertices_buffer;
        let index_buffer = self.custom_mesh_3d_indices_buffer;
        let upload_result: Result<()> = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => write_custom_mesh_3d_cache_buffers(
                device,
                vertex_buffer,
                index_buffer,
                vertex_byte_offset,
                index_byte_offset as u64,
                &vertex_bytes,
                &index_bytes,
            ),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => write_custom_mesh_3d_cache_buffers(
                device,
                vertex_buffer,
                index_buffer,
                vertex_byte_offset,
                index_byte_offset as u64,
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
                index_byte_offset as u64,
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
        let packed_index_offset = packed_index_offset(index_byte_offset, uses_u16)?;
        let entry = MeshCacheEntry {
            generation: mesh.generation,
            vertex_offset: vertex_offset_u32,
            vertex_count,
            index_offset: packed_index_offset,
            index_count,
        };
        self.custom_mesh_3d_mesh_cache.insert(mesh.id, entry);
        let surface_key = surface_mesh_allocator_key(self);
        surface_mesh_allocators()
            .lock()
            .expect("nova 3D mesh allocator lock poisoned")
            .entry(surface_key)
            .or_default()
            .commit(
                mesh.id,
                entry,
                mesh.vertices.len(),
                index_byte_offset,
                index_bytes.len(),
            );
        self.custom_mesh_3d_vertex_upload_scratch = vertex_bytes;
        self.custom_mesh_3d_index_upload_scratch = index_bytes;
        Ok(())
    }

    fn clear_custom_mesh_3d_cache(&mut self) {
        self.custom_mesh_3d_mesh_cache.clear();
        self.custom_mesh_3d_vertex_cursor = 0;
        self.custom_mesh_3d_index_cursor = 0;
    }

    /// Replaces the startup placeholder mesh buffers with full-capacity ones
    /// and rebinds every frame's mesh resource set. Runs once, on the first
    /// frame that actually carries custom 3D meshes; the placeholder set was
    /// never referenced by a mesh draw, and the old resources retire through
    /// the backend's deferred-release queue without stalling.
    fn promote_custom_mesh_3d_buffers(&mut self) -> Result<()> {
        let layout = self.custom_mesh_3d_resource_set_layout;
        let old_vertices = self.custom_mesh_3d_vertices_buffer;
        let old_indices = self.custom_mesh_3d_indices_buffer;
        let (vertices, indices) = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => promote_custom_mesh_3d_buffers_on_device(
                device,
                layout,
                &mut self.frame_resources,
                old_vertices,
                old_indices,
            )?,
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => promote_custom_mesh_3d_buffers_on_device(
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
            NovaBackend::Vulkan(device) => promote_custom_mesh_3d_buffers_on_device(
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
        // activate_frame_resources already ran this frame with the old set;
        // refresh the flattened field so recording binds the promoted one.
        if let Some(current) = self.frame_resources.get(self.current_frame_resource_index) {
            self.custom_mesh_3d_resource_set = current.resource_sets.custom_mesh_3d_resource_set;
        }
        self.custom_mesh_3d_buffers_ready = true;
        Ok(())
    }
}

fn promote_custom_mesh_3d_buffers_on_device<D>(
    device: &mut D,
    layout: ResourceSetLayoutId,
    frame_resources: &mut [FrameResources],
    old_vertices: BufferId,
    old_indices: BufferId,
) -> Result<(BufferId, BufferId)>
where
    D: BackendResources,
{
    let vertices = create_custom_mesh_3d_vertices_buffer(
        device,
        "nova renderer",
        MAX_CUSTOM_MESH_3D_VERTICES,
    )?;
    let indices =
        create_custom_mesh_3d_indices_buffer(device, "nova renderer", MAX_CUSTOM_MESH_3D_INDICES)?;
    for (index, frame) in frame_resources.iter_mut().enumerate() {
        let resource_set = create_custom_mesh_3d_resource_set(
            device,
            &format!("nova renderer frame {index}"),
            layout,
            frame.buffers.global_buffer,
            frame.buffers.custom_mesh_3d_parameters_buffer,
            vertices,
            MAX_CUSTOM_MESH_3D_VERTICES,
        )?;
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
