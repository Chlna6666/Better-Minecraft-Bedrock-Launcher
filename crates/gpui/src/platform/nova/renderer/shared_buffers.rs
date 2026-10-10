//! Two slots own at most two resident versions of each static stream. Only the selected,
//! fence-safe slot changes bindings; an in-flight slot keeps its immutable old version.
use super::*;

#[cfg(all(
    test,
    target_os = "windows",
    any(
        feature = "nova-gfx-dx11",
        feature = "nova-gfx-dx12",
        feature = "nova-gfx-vulkan",
        feature = "nova-gfx-opengl"
    )
))]
mod native_tests;

fn stream_byte_len(upload: &FrameUpload, stream: BufferStream) -> usize {
    let bytes = match stream {
        BufferStream::TextRaster => &upload.text_raster_params,
        BufferStream::Quad => return upload.quads.len(),
        BufferStream::Shadow => &upload.shadows,
        BufferStream::PathVertices => &upload.path_rasterization_vertices,
        BufferStream::PathSprites => &upload.path_sprites,
        BufferStream::MonoSprites => &upload.mono_sprites,
        BufferStream::PolySprites => &upload.poly_sprites,
        BufferStream::Underlines => &upload.underlines,
        BufferStream::BlurPasses => &upload.backdrop_blur_passes,
        BufferStream::BlurRecords => &upload.backdrop_blurs,
    };
    bytes.len()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VersionAction {
    Keep,
    Adopt(usize),
    Allocate(usize),
}

impl NovaRenderer {
    pub(super) fn buffer_is_referenced(&self, buffer: BufferId) -> bool {
        self.frame_resources
            .iter()
            .any(|frame| frame.buffers.ids().contains(&buffer))
    }

    fn static_version_action(&self, stream: BufferStream) -> Result<VersionAction> {
        let slot = self.current_frame_resource_index;
        let (buffer, capacity, stride, hard_limit) =
            stream.allocation(self.frame_resources[slot].buffers);
        let required = stream_byte_len(&self.frame_upload, stream).div_ceil(stride);
        let next_capacity = buffer_capacity::next_capacity(capacity, required, hard_limit)?;
        let shareable = self.retained_upload.stream_shareable(stream);
        let (_, _, dirty) = self.retained_upload.stream_state(slot, stream);
        let mut protected = false;
        for (source, frame) in self.frame_resources.iter().enumerate() {
            if source == slot {
                continue;
            }
            let (matches, _, _) = self.retained_upload.stream_state(source, stream);
            if shareable && matches {
                return Ok(if dirty || stream.allocation(frame.buffers).0 != buffer {
                    VersionAction::Adopt(source)
                } else {
                    VersionAction::Keep
                });
            }
            if stream.allocation(frame.buffers).0 == buffer {
                let pending = self
                    .pending_submissions
                    .iter()
                    .any(|submission| submission.frame_resource_index == source);
                protected |= !shareable || (dirty && pending);
            }
        }
        Ok(if protected || capacity != next_capacity {
            VersionAction::Allocate(next_capacity)
        } else {
            VersionAction::Keep
        })
    }

    pub(super) fn prepare_static_buffers(&mut self) -> Result<()> {
        let slot = self.current_frame_resource_index;
        buffer_capacity::validate_growth_slot(
            slot,
            self.pending_submissions
                .iter()
                .map(|submission| submission.frame_resource_index),
        )?;
        let mut actions = [(BufferStream::TextRaster, VersionAction::Keep); 10];
        for (action, stream) in actions.iter_mut().zip(BufferStream::ALL) {
            *action = (stream, self.static_version_action(stream)?);
        }
        let previous = self.frame_resources[slot].buffers.ids();
        self.apply_static_version_actions(&actions)?;
        // Idle aliases may be updated in place. Their old tokens must stop describing the bytes
        // before any upload, including when an upload later fails partway through.
        for stream in BufferStream::ALL {
            if !self.retained_upload.stream_shareable(stream)
                || !self.retained_upload.stream_state(slot, stream).2
            {
                continue;
            }
            let buffer = stream.allocation(self.frame_resources[slot].buffers).0;
            for alias in 0..self.frame_resources.len() {
                if alias != slot
                    && stream.allocation(self.frame_resources[alias].buffers).0 == buffer
                {
                    self.retained_upload.invalidate_stream(alias, stream);
                }
            }
        }
        if previous != self.frame_resources[slot].buffers.ids() {
            self.invalidate_backdrop_blur_cache();
        }
        Ok(())
    }

    // Fast GPUs may keep selecting slot 0; an idle slot must not pin an obsolete version.
    pub(super) fn coalesce_idle_static_buffers(&mut self) -> Result<()> {
        let active = self.current_frame_resource_index;
        for slot in 0..self.frame_resources.len() {
            if slot == active
                || self
                    .pending_submissions
                    .iter()
                    .any(|pending| pending.frame_resource_index == slot)
            {
                continue;
            }
            let mut actions = [(BufferStream::TextRaster, VersionAction::Keep); 10];
            for (action, stream) in actions.iter_mut().zip(BufferStream::ALL) {
                let reusable = self.retained_upload.stream_state(active, stream).0;
                let dirty = self.retained_upload.stream_state(slot, stream).2;
                let differs = stream.allocation(self.frame_resources[slot].buffers).0
                    != stream.allocation(self.frame_resources[active].buffers).0;
                *action = (
                    stream,
                    if reusable && (dirty || differs) {
                        VersionAction::Adopt(active)
                    } else {
                        VersionAction::Keep
                    },
                );
            }
            if actions
                .iter()
                .all(|(_, action)| *action == VersionAction::Keep)
            {
                continue;
            }
            self.activate_frame_resources(slot)?;
            let result = self.apply_static_version_actions(&actions);
            self.activate_frame_resources(active)?;
            result?;
        }
        Ok(())
    }

    fn apply_static_version_actions(
        &mut self,
        actions: &[(BufferStream, VersionAction)],
    ) -> Result<()> {
        if actions
            .iter()
            .all(|(_, action)| *action == VersionAction::Keep)
        {
            return Ok(());
        }
        let backend = self.backend.clone();
        let mut backend = lock_backend(&backend);
        match &mut *backend {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => self.apply_static_versions(device, actions),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => self.apply_static_versions(device, actions),
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => self.apply_static_versions(device, actions),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => self.apply_static_versions(device, actions),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => self.apply_static_versions(device, actions),
            #[allow(unreachable_patterns)]
            _ => anyhow::bail!("nova backend is unavailable while selecting static buffers"),
        }
    }

    fn apply_static_versions<D: BackendResources>(
        &mut self,
        device: &mut D,
        actions: &[(BufferStream, VersionAction)],
    ) -> Result<()> {
        let slot = self.current_frame_resource_index;
        let current = self.frame_resources[slot].buffers;
        let mut next = current;
        let mut created = Vec::new();
        for &(stream, action) in actions {
            match action {
                VersionAction::Keep => {}
                VersionAction::Adopt(source) => {
                    let (buffer, capacity, _, _) =
                        stream.allocation(self.frame_resources[source].buffers);
                    stream.replace(&mut next, buffer, capacity);
                }
                VersionAction::Allocate(capacity) => {
                    let (_, _, stride, _) = stream.allocation(current);
                    let usage = if stream == BufferStream::TextRaster {
                        BufferUsage::UNIFORM
                    } else {
                        BufferUsage::STORAGE
                    };
                    match device.create_buffer(&BufferDescriptor {
                        label: Some(format!("gpui nova static version {stream:?}")),
                        size: (capacity * stride) as u64,
                        usage: usage | BufferUsage::COPY_DST,
                        memory_location: MemoryLocation::CpuToGpu,
                    }) {
                        Ok(buffer) => {
                            created.push(buffer);
                            stream.replace(&mut next, buffer, capacity);
                        }
                        Err(error) => {
                            buffer_capacity::release_buffers(device, created);
                            return Err(error.into());
                        }
                    }
                }
            }
        }
        if next.ids() != current.ids() {
            if let Err(error) = buffer_capacity::bindings::rebind_slot(self, device, next) {
                buffer_capacity::release_buffers(device, created);
                return Err(error);
            }
            self.frame_resources[slot].buffers = next;
            self.activate_frame_resources(slot)?;
            self.draw_step_scratch.invalidate_draw_steps();
            buffer_capacity::release_buffers(
                device,
                current
                    .ids()
                    .into_iter()
                    .filter(|buffer| !self.buffer_is_referenced(*buffer)),
            );
        }
        for &(stream, action) in actions {
            match action {
                VersionAction::Adopt(source) => {
                    self.retained_upload.inherit_stream(slot, source, stream)
                }
                // A new version contains no valid gaps, so P2-B must use a full first upload.
                VersionAction::Allocate(_) => self.retained_upload.invalidate_stream(slot, stream),
                VersionAction::Keep => {}
            }
        }
        Ok(())
    }
}
