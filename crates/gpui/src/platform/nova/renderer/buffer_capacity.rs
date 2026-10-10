//! Grow only the selected fence-safe slot; replace bindings transactionally.
use super::*;

pub(super) mod bindings;
#[cfg(test)]
mod tests;

pub(super) fn next_capacity(current: usize, required: usize, limit: usize) -> Result<usize> {
    anyhow::ensure!(
        required <= limit,
        "nova buffer upload exceeds hard limit: required={required} max={limit}"
    );
    Ok(if required <= current {
        current
    } else {
        required
            .next_power_of_two()
            .max(current.saturating_mul(2))
            .min(limit)
    })
}

pub(super) fn validate_growth_slot(
    slot: usize,
    mut occupied: impl Iterator<Item = usize>,
) -> Result<()> {
    anyhow::ensure!(
        occupied.all(|pending| pending != slot),
        "cannot grow nova frame slot {slot} while a submission still references it"
    );
    Ok(())
}

fn capacities(buffers: FrameResourceBuffers) -> [usize; 5] {
    [
        buffers.shadow_capacity,
        buffers.path_sprite_capacity,
        buffers.mono_sprite_capacity,
        buffers.poly_sprite_capacity,
        buffers.animation_value_capacity,
    ]
}

fn buffer_ids(buffers: FrameResourceBuffers) -> [BufferId; 5] {
    [
        buffers.shadow_buffer,
        buffers.path_sprite_buffer,
        buffers.mono_sprite_buffer,
        buffers.poly_sprite_buffer,
        buffers.animation_value_buffer,
    ]
}

fn resized_buffers(
    mut buffers: FrameResourceBuffers,
    ids: [BufferId; 5],
    sizes: [usize; 5],
) -> FrameResourceBuffers {
    [
        buffers.shadow_buffer,
        buffers.path_sprite_buffer,
        buffers.mono_sprite_buffer,
        buffers.poly_sprite_buffer,
        buffers.animation_value_buffer,
    ] = ids;
    [
        buffers.shadow_capacity,
        buffers.path_sprite_capacity,
        buffers.mono_sprite_capacity,
        buffers.poly_sprite_capacity,
        buffers.animation_value_capacity,
    ] = sizes;
    buffers
}

pub(super) fn release_buffers<D: BackendResources>(
    device: &mut D,
    buffers: impl IntoIterator<Item = BufferId>,
) {
    for buffer in buffers {
        if let Err(error) = device.destroy_buffer(buffer) {
            log::error!("failed to retire nova grown buffer: {error}");
        }
    }
}

fn allocate_buffers<D: BackendResources>(
    device: &mut D,
    current: FrameResourceBuffers,
    sizes: [usize; 5],
) -> Result<FrameResourceBuffers> {
    let old_sizes = capacities(current);
    let mut ids = buffer_ids(current);
    let strides = [
        PACKED_SHADOW_BYTES,
        PACKED_PATH_SPRITE_BYTES,
        PACKED_MONO_SPRITE_BYTES,
        PACKED_POLY_SPRITE_BYTES,
        PACKED_ANIMATION_VALUE_BYTES,
    ];
    let names = [
        "shadows",
        "path sprites",
        "mono sprites",
        "poly sprites",
        "animation values",
    ];
    let mut created = Vec::new();
    for index in 0..ids.len() {
        if sizes[index] == old_sizes[index] {
            continue;
        }
        match device.create_buffer(&BufferDescriptor {
            label: Some(format!("gpui nova grown {}", names[index])),
            size: (sizes[index] * strides[index]) as u64,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        }) {
            Ok(buffer) => {
                ids[index] = buffer;
                created.push(buffer);
            }
            Err(error) => {
                release_buffers(device, created);
                return Err(error.into());
            }
        }
    }
    Ok(resized_buffers(current, ids, sizes))
}

impl NovaRenderer {
    /// Called after prepare_for_frame_submission has selected an unoccupied slot.
    pub(super) fn ensure_stream_capacity(&mut self) -> Result<()> {
        let current = self.frame_resources[self.current_frame_resource_index].buffers;
        let upload = &self.frame_upload;
        let required = [
            upload.shadows.len().div_ceil(PACKED_SHADOW_BYTES),
            upload.path_sprites.len().div_ceil(PACKED_PATH_SPRITE_BYTES),
            upload.mono_sprites.len().div_ceil(PACKED_MONO_SPRITE_BYTES),
            upload.poly_sprites.len().div_ceil(PACKED_POLY_SPRITE_BYTES),
            upload
                .gpu_indexed_animation_values
                .len()
                .div_ceil(PACKED_ANIMATION_VALUE_BYTES),
        ];
        let limits = [
            MAX_SHADOWS,
            MAX_PATH_SPRITES,
            MAX_MONO_SPRITES,
            MAX_POLY_SPRITES,
            MAX_ANIMATION_VALUES,
        ];
        let old = capacities(current);
        let mut sizes = old;
        for index in 0..sizes.len() {
            sizes[index] = next_capacity(old[index], required[index], limits[index])?;
        }
        if sizes == old {
            return Ok(());
        }
        validate_growth_slot(
            self.current_frame_resource_index,
            self.pending_submissions
                .iter()
                .map(|submission| submission.frame_resource_index),
        )?;
        // Clone the owner handle to hold its guard while mutating the disjoint renderer state.
        let backend = Arc::clone(&self.backend);
        match &mut *lock_backend(&backend) {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => self.grow_stream_buffers(device, current, sizes),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => self.grow_stream_buffers(device, current, sizes),
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => self.grow_stream_buffers(device, current, sizes),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => self.grow_stream_buffers(device, current, sizes),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => self.grow_stream_buffers(device, current, sizes),
            #[allow(unreachable_patterns)]
            _ => anyhow::bail!("nova backend is unavailable while growing frame buffers"),
        }
    }

    fn grow_stream_buffers<D: BackendResources>(
        &mut self,
        device: &mut D,
        current: FrameResourceBuffers,
        sizes: [usize; 5],
    ) -> Result<()> {
        let next = allocate_buffers(device, current, sizes)?;
        let old_ids = buffer_ids(current);
        let next_ids = buffer_ids(next);
        let result = bindings::rebind_slot(self, device, next);
        if let Err(error) = result {
            release_buffers(
                device,
                next_ids
                    .into_iter()
                    .zip(old_ids)
                    .filter_map(|(new, old)| (new != old).then_some(new)),
            );
            return Err(error);
        }
        let slot = self.current_frame_resource_index;
        self.frame_resources[slot].buffers = next;
        self.activate_frame_resources(slot)?;
        for (stream, old, new) in [
            (
                BufferStream::Shadow,
                current.shadow_buffer,
                next.shadow_buffer,
            ),
            (
                BufferStream::PathSprites,
                current.path_sprite_buffer,
                next.path_sprite_buffer,
            ),
            (
                BufferStream::MonoSprites,
                current.mono_sprite_buffer,
                next.mono_sprite_buffer,
            ),
            (
                BufferStream::PolySprites,
                current.poly_sprite_buffer,
                next.poly_sprite_buffer,
            ),
        ] {
            if old != new {
                self.retained_upload.invalidate_stream(slot, stream);
            }
        }
        self.draw_step_scratch.invalidate_draw_steps();
        self.invalidate_backdrop_blur_cache();
        release_buffers(
            device,
            old_ids.into_iter().zip(next_ids).filter_map(|(old, new)| {
                (new != old && !self.buffer_is_referenced(old)).then_some(old)
            }),
        );
        Ok(())
    }
}
