//! Group-zero Metal resource bindings shared with the MSL compiler ABI.
use super::*;
use gfx_core::{BindingType, MetalShaderBindings, ShaderStages, METAL_SIZE_BUFFER_SLOT, metal_binding_slot};
use std::sync::Arc;

enum NativeResource {
    Buffer(Retained<ProtocolObject<dyn MTLBuffer>>, usize),
    Texture(Retained<ProtocolObject<dyn MTLTexture>>),
    Sampler(Retained<ProtocolObject<dyn MTLSamplerState>>),
}

struct NativeBinding {
    slot: u8,
    stages: ShaderStages,
    resource: NativeResource,
}

pub(super) struct NativeBindGroup {
    entries: Vec<NativeBinding>,
    buffer_sizes: [u64; 30],
}

#[derive(Default)]
pub(super) struct PreparedBindings {
    group: Option<Arc<NativeBindGroup>>,
    vertex_sizes: Vec<u32>,
    fragment_sizes: Vec<u32>,
}

impl MetalDevice {
    pub(super) fn create_native_bind_group(
        &self,
        desc: &BindGroupDescriptor,
        layout: &BindGroupLayoutDescriptor,
    ) -> Result<NativeBindGroup> {
        let mut entries = Vec::with_capacity(desc.entries.len());
        let mut buffer_sizes = [0; 30];
        for entry in &desc.entries {
            let declaration = layout.entries.iter().find(|item| item.binding == entry.binding)
                .ok_or_else(|| Error::InvalidInput("Metal binding absent from layout".into()))?;
            let slot = metal_binding_slot(0, entry.binding, declaration.binding_type)
                .ok_or_else(|| Error::Unavailable("unsupported Metal binding slot".into()))?;
            let resource = match (entry.resource, declaration.binding_type) {
                (BindingResource::Buffer(binding), kind @ (BindingType::UniformBuffer | BindingType::StorageBuffer | BindingType::StorageBufferReadWrite)) => {
                    let buffer = self.buffers.get(binding.buffer)?;
                    binding.validate_against(buffer.desc.size)?;
                    let (usage, alignment) = match kind {
                        BindingType::UniformBuffer => (BufferUsage::UNIFORM, 256),
                        BindingType::StorageBuffer => (BufferUsage::STORAGE, 4),
                        BindingType::StorageBufferReadWrite => (BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE, 4),
                        _ => unreachable!(),
                    };
                    if !buffer.desc.usage.contains(usage) || binding.offset % alignment != 0 {
                        return Err(Error::InvalidInput(format!("Metal buffer binding {} has invalid usage or offset alignment", entry.binding)));
                    }
                    buffer_sizes[usize::from(slot)] = binding.size;
                    let native = buffer.resource.clone().ok_or_else(|| Error::Unavailable("missing native Metal buffer".into()))?;
                    let offset = usize::try_from(binding.offset).map_err(|_| Error::InvalidInput("Metal buffer offset overflow".into()))?;
                    NativeResource::Buffer(native, offset)
                }
                (BindingResource::Texture(binding), BindingType::SampledTexture) => {
                    let view = self.texture_views.get(binding.texture_view)?;
                    let texture = self.textures.get(view.texture)?;
                    if !texture.desc.usage.contains(TextureUsage::SAMPLED) {
                        return Err(Error::InvalidInput("Metal sampled texture requires SAMPLED usage".into()));
                    }
                    NativeResource::Texture(view.resource.clone())
                }
                (BindingResource::Sampler(binding), BindingType::Sampler) => {
                    NativeResource::Sampler(self.samplers.get(binding.sampler)?.resource.clone())
                }
                _ => return Err(Error::Unavailable("unsupported Metal resource binding kind".into())),
            };
            entries.push(NativeBinding { slot, stages: declaration.stages, resource });
        }
        Ok(NativeBindGroup { entries, buffer_sizes })
    }

    pub(super) fn validate_shader_bindings(
        &self,
        layout: Option<BindGroupLayoutId>,
        shader: Option<&MetalShaderBindings>,
        stage: ShaderStages,
    ) -> Result<()> {
        let Some(shader) = shader else { return Ok(()); };
        let (mut buffers, mut textures, mut samplers) = (0u32, 0u32, 0u16);
        if let Some(layout) = layout {
            for entry in &self.bind_group_layouts.get(layout)?.desc.entries {
                if !entry.stages.contains(stage) { continue; }
                let slot = metal_binding_slot(0, entry.binding, entry.binding_type)
                    .ok_or_else(|| Error::Unavailable("unsupported Metal shader layout slot".into()))?;
                match entry.binding_type {
                    BindingType::UniformBuffer | BindingType::StorageBuffer | BindingType::StorageBufferReadWrite => buffers |= 1 << slot,
                    BindingType::SampledTexture => textures |= 1 << slot,
                    BindingType::Sampler => samplers |= 1 << slot,
                    BindingType::StorageTexture { .. } => {}
                }
            }
        }
        if shader.buffers & !buffers != 0 || shader.textures & !textures != 0 || shader.samplers & !samplers != 0 {
            return Err(Error::InvalidInput("Metal pipeline layout omits a reflected shader binding".into()));
        }
        Ok(())
    }

    pub(super) fn prepare_bindings(
        &self,
        pipeline: &MetalRenderPipeline,
        ids: &[BindGroupId],
    ) -> Result<PreparedBindings> {
        let group = match (pipeline.group_layout, ids) {
            (None, []) => return Ok(PreparedBindings::default()),
            (Some(expected), [id]) => {
                let group = self.bind_groups.get(*id)?;
                if group.layout != expected {
                    return Err(Error::InvalidInput("Metal bind group layout mismatch".into()));
                }
                group.native.clone()
            }
            _ => return Err(Error::InvalidInput("Metal bind group count does not match pipeline".into())),
        };
        let vertex_sizes = size_table(pipeline.vertex_bindings.as_ref(), &group)?;
        let fragment_sizes = size_table(pipeline.fragment_bindings.as_ref(), &group)?;
        Ok(PreparedBindings { group: Some(group), vertex_sizes, fragment_sizes })
    }
}

fn size_table(shader: Option<&MetalShaderBindings>, group: &NativeBindGroup) -> Result<Vec<u32>> {
    let Some(shader) = shader else { return Ok(Vec::new()); };
    if shader.size_slots.len() > 1024 {
        return Err(Error::InvalidInput("Metal runtime-array size table exceeds setBytes capacity".into()));
    }
    shader.size_slots.iter().map(|&slot| {
        if slot == u8::MAX { return Ok(0); }
        let size = *group.buffer_sizes.get(usize::from(slot))
            .filter(|&&size| size != 0)
            .ok_or_else(|| Error::InvalidInput("missing Metal runtime-array binding range".into()))?;
        u32::try_from(size).map_err(|_| Error::InvalidInput("Metal runtime-array binding exceeds u32 size ABI".into()))
    }).collect()
}

impl PreparedBindings {
    pub(super) fn encode(&self, encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>) {
        if let Some(group) = &self.group {
            for entry in &group.entries {
                let slot = usize::from(entry.slot);
                let vertex = entry.stages.contains(ShaderStages::VERTEX);
                let fragment = entry.stages.contains(ShaderStages::FRAGMENT);
                // SAFETY: Native objects are retained by the immutable group.
                // Slot, offset, range and usage were checked before encoding.
                unsafe {
                    match &entry.resource {
                        NativeResource::Buffer(buffer, offset) => {
                            if vertex { encoder.setVertexBuffer_offset_atIndex(Some(buffer), *offset, slot); }
                            if fragment { encoder.setFragmentBuffer_offset_atIndex(Some(buffer), *offset, slot); }
                        }
                        NativeResource::Texture(texture) => {
                            if vertex { encoder.setVertexTexture_atIndex(Some(texture), slot); }
                            if fragment { encoder.setFragmentTexture_atIndex(Some(texture), slot); }
                        }
                        NativeResource::Sampler(sampler) => {
                            if vertex { encoder.setVertexSamplerState_atIndex(Some(sampler), slot); }
                            if fragment { encoder.setFragmentSamplerState_atIndex(Some(sampler), slot); }
                        }
                    }
                }
            }
        }
        // SAFETY: setBytes copies these initialized, nonempty u32 slices into
        // command storage before returning. The reserved slot cannot overlap
        // user resources and each table is at most 4096 bytes.
        unsafe {
            if !self.vertex_sizes.is_empty() {
                let bytes = NonNull::from(self.vertex_sizes.as_slice()).cast::<c_void>();
                encoder.setVertexBytes_length_atIndex(bytes, self.vertex_sizes.len() * 4, usize::from(METAL_SIZE_BUFFER_SLOT));
            }
            if !self.fragment_sizes.is_empty() {
                let bytes = NonNull::from(self.fragment_sizes.as_slice()).cast::<c_void>();
                encoder.setFragmentBytes_length_atIndex(bytes, self.fragment_sizes.len() * 4, usize::from(METAL_SIZE_BUFFER_SLOT));
            }
        }
    }
}
