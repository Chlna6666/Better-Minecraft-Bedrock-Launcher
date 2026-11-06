use crate::{
    device::OpenGlDevice,
    pipelines::{BufferSlot, Pipeline, TextureSlot},
    resources::BindGroup,
};
use gfx_core::*;
use glow::HasContext as _;

#[derive(Default)]
pub(crate) struct Cache {
    buffers: Vec<(u32, u32, glow::NativeBuffer, i32, i32)>,
    textures: Vec<(u32, u32, glow::NativeTexture)>,
    samplers: Vec<(u32, Option<glow::NativeSampler>)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU32;
    #[test]
    fn ranges_targets_units_and_pass_resets_preserve_binding_changes() {
        let mut cache = Cache::default();
        let buffer = glow::NativeBuffer(NonZeroU32::new(1).unwrap());
        let texture = glow::NativeTexture(NonZeroU32::new(2).unwrap());
        let sampler = glow::NativeSampler(NonZeroU32::new(3).unwrap());
        assert!(cache.buffer(glow::UNIFORM_BUFFER, 0, buffer, 0, 32));
        assert!(!cache.buffer(glow::UNIFORM_BUFFER, 0, buffer, 0, 32));
        assert!(cache.buffer(glow::UNIFORM_BUFFER, 0, buffer, 16, 32));
        assert!(cache.buffer(glow::UNIFORM_BUFFER, 0, buffer, 16, 64));
        assert!(cache.buffer(glow::SHADER_STORAGE_BUFFER, 0, buffer, 16, 64));
        assert!(cache.texture(0, glow::TEXTURE_2D, texture));
        assert!(!cache.texture(0, glow::TEXTURE_2D, texture));
        assert!(cache.texture(1, glow::TEXTURE_2D, texture));
        assert!(cache.texture(0, glow::TEXTURE_2D_ARRAY, texture));
        assert!(cache.sampler(0, None));
        assert!(!cache.sampler(0, None));
        assert!(cache.sampler(0, Some(sampler)));
        let capacity = cache.buffers.capacity();
        cache.reset();
        assert_eq!(cache.buffers.capacity(), capacity);
        assert!(cache.buffer(glow::UNIFORM_BUFFER, 0, buffer, 0, 32));
        assert!(cache.texture(0, glow::TEXTURE_2D, texture));
        assert!(cache.sampler(0, None));
    }
}

impl Cache {
    pub(crate) fn reset(&mut self) {
        self.buffers.clear();
        self.textures.clear();
        self.samplers.clear();
    }
    fn buffer(
        &mut self,
        target: u32,
        slot: u32,
        native: glow::NativeBuffer,
        offset: i32,
        size: i32,
    ) -> bool {
        let key = (target, slot, native, offset, size);
        if let Some(previous) = self
            .buffers
            .iter_mut()
            .find(|value| value.0 == target && value.1 == slot)
        {
            let changed = *previous != key;
            *previous = key;
            changed
        } else {
            self.buffers.push(key);
            true
        }
    }
    fn texture(&mut self, unit: u32, target: u32, native: glow::NativeTexture) -> bool {
        if let Some(previous) = self.textures.iter_mut().find(|value| value.0 == unit) {
            let next = (unit, target, native);
            let changed = *previous != next;
            *previous = next;
            changed
        } else {
            self.textures.push((unit, target, native));
            true
        }
    }
    fn sampler(&mut self, unit: u32, native: Option<glow::NativeSampler>) -> bool {
        if let Some((_, previous)) = self.samplers.iter_mut().find(|value| value.0 == unit) {
            let changed = *previous != native;
            *previous = native;
            changed
        } else {
            self.samplers.push((unit, native));
            true
        }
    }
}

impl OpenGlDevice {
    fn binding(set: &BindGroup, logical: u32) -> Result<BindingResource> {
        set.desc
            .entries
            .iter()
            .find(|binding| binding.binding == logical)
            .map(|binding| binding.resource)
            .ok_or_else(|| Error::InvalidInput(format!("OpenGL missing resource {logical}")))
    }

    pub(crate) fn bind_resources(
        &self,
        pipeline: &Pipeline,
        set: Option<&BindGroup>,
        cache: &mut Cache,
    ) -> Result<()> {
        let Some(set) = set else {
            return if pipeline.buffers.is_empty() && pipeline.textures.is_empty() {
                Ok(())
            } else {
                Err(Error::InvalidInput(
                    "OpenGL shader needs a bind group".into(),
                ))
            };
        };
        for binding in &pipeline.buffers {
            self.bind_buffer_range(set, binding, cache)?;
        }
        for binding in &pipeline.textures {
            self.bind_sampled_texture(set, binding, cache)?;
        }
        Ok(())
    }

    fn bind_buffer_range(
        &self,
        set: &BindGroup,
        binding: &BufferSlot,
        cache: &mut Cache,
    ) -> Result<()> {
        let BindingResource::Buffer(range) = Self::binding(set, binding.logical)? else {
            return Err(Error::InvalidInput(
                "OpenGL shader buffer bound to a non-buffer".into(),
            ));
        };
        let buffer = self.buffers.get(range.buffer)?;
        let uniform = binding.kind == BindingType::UniformBuffer;
        let size = if uniform {
            range.size.div_ceil(16) * 16
        } else {
            range.size
        };
        let native_size = i32::try_from(size)
            .map_err(|_| Error::InvalidInput("OpenGL buffer range exceeds native size".into()))?;
        let source_offset = i32::try_from(range.offset)
            .map_err(|_| Error::InvalidInput("OpenGL buffer offset exceeds native size".into()))?;
        // SAFETY: resources and byte ranges were validated when the set was created;
        // the owner context is current. Mirrors copy current GPU data before each draw.
        unsafe {
            let (native, offset) = if let Some(mirror) = set
                .mirrors
                .iter()
                .find(|mirror| mirror.binding == binding.logical)
            {
                self.gl
                    .bind_buffer(glow::COPY_READ_BUFFER, Some(buffer.native));
                self.gl
                    .bind_buffer(glow::COPY_WRITE_BUFFER, Some(mirror.native));
                self.gl.copy_buffer_sub_data(
                    glow::COPY_READ_BUFFER,
                    glow::COPY_WRITE_BUFFER,
                    source_offset,
                    0,
                    i32::try_from(range.size).map_err(|_| {
                        Error::InvalidInput("OpenGL mirror copy exceeds native size".into())
                    })?,
                );
                (mirror.native, 0)
            } else {
                if range.offset + size > buffer.native_size as u64 {
                    return Err(Error::InvalidInput(
                        "OpenGL padded uniform range exceeds native buffer".into(),
                    ));
                }
                (buffer.native, source_offset)
            };
            let target = if uniform {
                glow::UNIFORM_BUFFER
            } else {
                glow::SHADER_STORAGE_BUFFER
            };
            if cache.buffer(target, binding.slot, native, offset, native_size) {
                self.gl
                    .bind_buffer_range(target, binding.slot, Some(native), offset, native_size);
            }
        }
        Ok(())
    }

    fn bind_sampled_texture(
        &self,
        set: &BindGroup,
        binding: &TextureSlot,
        cache: &mut Cache,
    ) -> Result<()> {
        let BindingResource::Texture(texture) = Self::binding(set, binding.logical)? else {
            return Err(Error::InvalidInput(
                "OpenGL shader texture bound to a non-texture".into(),
            ));
        };
        let view = self.views.get(texture.texture_view)?;
        if !view.usage.contains(TextureUsage::SAMPLED) {
            return Err(Error::InvalidInput(
                "OpenGL texture view lacks SAMPLED".into(),
            ));
        }
        let sampler = if let Some(slot) = binding.sampler {
            let BindingResource::Sampler(binding) = Self::binding(set, slot)? else {
                return Err(Error::InvalidInput(
                    "OpenGL sampler bound to a non-sampler".into(),
                ));
            };
            Some(*self.samplers.get(binding.sampler)?)
        } else {
            None
        };
        // SAFETY: the view and sampler belong to this current owner context. Native units
        // were allocated within the driver's limit during pipeline reflection.
        unsafe {
            if cache.texture(binding.unit, view.target, view.native) {
                self.gl.active_texture(glow::TEXTURE0 + binding.unit);
                self.gl.bind_texture(view.target, Some(view.native));
            }
            if cache.sampler(binding.unit, sampler) {
                self.gl.bind_sampler(binding.unit, sampler);
            }
        }
        Ok(())
    }
}
