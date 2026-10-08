use crate::{
    device::OpenGlDevice,
    pipelines::{BufferSlot, Pipeline, TextureSlot},
    resources::ResourceSet,
};
use gfx_core::*;
use glow::HasContext as _;

impl OpenGlDevice {
    fn binding(set: &ResourceSet, logical: u32) -> Result<BindingResource> {
        set.desc
            .bindings
            .iter()
            .find(|binding| binding.binding == logical)
            .map(|binding| binding.resource)
            .ok_or_else(|| Error::InvalidInput(format!("OpenGL missing resource {logical}")))
    }

    pub(crate) fn bind_resources(
        &self,
        pipeline: &Pipeline,
        set: Option<&ResourceSet>,
    ) -> Result<()> {
        let Some(set) = set else {
            return if pipeline.buffers.is_empty() && pipeline.textures.is_empty() {
                Ok(())
            } else {
                Err(Error::InvalidInput(
                    "OpenGL shader needs a resource set".into(),
                ))
            };
        };
        for binding in &pipeline.buffers {
            self.bind_buffer_range(set, binding)?;
        }
        for binding in &pipeline.textures {
            self.bind_sampled_texture(set, binding)?;
        }
        Ok(())
    }

    fn bind_buffer_range(&self, set: &ResourceSet, binding: &BufferSlot) -> Result<()> {
        let BindingResource::Buffer(range) = Self::binding(set, binding.logical)? else {
            return Err(Error::InvalidInput(
                "OpenGL shader buffer bound to a non-buffer".into(),
            ));
        };
        let buffer = self.buffers.get(range.buffer)?;
        let uniform = binding.kind == ResourceBindingType::UniformBuffer;
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
            self.gl.bind_buffer_range(
                if uniform {
                    glow::UNIFORM_BUFFER
                } else {
                    glow::SHADER_STORAGE_BUFFER
                },
                binding.slot,
                Some(native),
                offset,
                native_size,
            );
        }
        Ok(())
    }

    fn bind_sampled_texture(&self, set: &ResourceSet, binding: &TextureSlot) -> Result<()> {
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
            self.gl.active_texture(glow::TEXTURE0 + binding.unit);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(view.native));
            self.gl.bind_sampler(binding.unit, sampler);
        }
        Ok(())
    }
}
