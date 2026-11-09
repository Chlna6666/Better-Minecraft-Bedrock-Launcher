//! Share immutable descriptors; release references only when their bind groups retire.
use super::*;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum DescriptorKey {
    Buffer(BufferId, u64, u64, bool),
    WritableBuffer(BufferId, u64, u64),
    Texture(TextureViewId),
    StorageTexture(TextureViewId),
    Sampler(SamplerId),
    SamplerIndices(Vec<u32>),
}

struct SharedDescriptor {
    key: DescriptorKey,
    slot: DescriptorSlot,
    references: usize,
    buffer: Option<ID3D12Resource>,
}

#[derive(Default)]
pub(super) struct DescriptorCache {
    lookup: HashMap<DescriptorKey, u32>,
    slots: HashMap<u32, SharedDescriptor>,
}

impl DescriptorCache {
    fn retain(&mut self, key: &DescriptorKey) -> Option<DescriptorSlot> {
        let index = *self.lookup.get(key)?;
        let entry = self.slots.get_mut(&index)?;
        entry.references += 1;
        Some(entry.slot)
    }
    fn insert(&mut self, key: DescriptorKey, slot: DescriptorSlot) {
        self.lookup.insert(key.clone(), slot.index);
        self.slots.insert(
            slot.index,
            SharedDescriptor {
                key,
                slot,
                references: 1,
                buffer: None,
            },
        );
    }
    pub(super) fn release(&mut self, index: u32) -> bool {
        let Some(entry) = self.slots.get_mut(&index) else {
            return true;
        };
        entry.references -= 1;
        if entry.references != 0 {
            return false;
        }
        if let Some(entry) = self.slots.remove(&index) {
            self.lookup.remove(&entry.key);
        }
        true
    }
}

impl DescriptorHeapAllocator {
    pub(super) fn allocate_shared(&mut self, key: DescriptorKey) -> Result<(DescriptorSlot, bool)> {
        if let Some(slot) = self.shared.retain(&key) {
            return Ok((slot, false));
        }
        let slot = self.allocate()?;
        self.shared.insert(key, slot);
        Ok((slot, true))
    }
}

impl Dx12Device {
    pub(super) fn shared_sampler_table(
        &mut self,
        tables: &[Dx12DescriptorTable],
        layout: &BindGroupLayoutDescriptor,
    ) -> Result<(Dx12DescriptorTable, ID3D12Resource)> {
        let indices = sampler_index_values(tables, layout)?;
        let count =
            u32::try_from(indices.len()).map_err(|error| Error::InvalidInput(error.to_string()))?;
        let (slot, fresh) = self
            .resource_heap
            .allocate_shared(DescriptorKey::SamplerIndices(indices.clone()))?;
        let result = (|| {
            if fresh {
                let buffer = create_sampler_index_buffer(&self.device, &indices)?;
                let desc=D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format:DXGI_FORMAT_UNKNOWN,
                    ViewDimension:windows::Win32::Graphics::Direct3D12::D3D12_SRV_DIMENSION_BUFFER,
                    Shader4ComponentMapping:windows::Win32::Graphics::Direct3D12::D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous:windows::Win32::Graphics::Direct3D12::D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Buffer:windows::Win32::Graphics::Direct3D12::D3D12_BUFFER_SRV {FirstElement:0,NumElements:count,StructureByteStride:4,Flags:windows::Win32::Graphics::Direct3D12::D3D12_BUFFER_SRV_FLAG_NONE}
                    },
                };
                // SAFETY: The live upload buffer contains `count` u32 indices and the slot is owned.
                unsafe {
                    self.device.CreateShaderResourceView(
                        Some(&buffer),
                        Some(&raw const desc),
                        slot.cpu_handle,
                    );
                }
                if let Some(entry) = self.resource_heap.shared.slots.get_mut(&slot.index) {
                    entry.buffer = Some(buffer);
                }
            }
            let buffer = self
                .resource_heap
                .shared
                .slots
                .get(&slot.index)
                .and_then(|entry| entry.buffer.clone())
                .ok_or_else(|| Error::Backend("shared sampler table has no buffer".into()))?;
            Ok((
                Dx12DescriptorTable {
                    binding: NAGA_HLSL_SAMPLER_INDEX_SPACE,
                    gpu_handle: slot.gpu_handle,
                    descriptor_index: slot.index,
                    resource: None,
                },
                buffer,
            ))
        })();
        if result.is_err() {
            self.resource_heap.free(slot);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx_core::{BindGroupEntry, BindGroupLayoutEntry, SamplerBinding, TextureBinding};
    #[test]
    fn shared_slots_are_released_only_after_the_last_owner_retires() {
        let mut cache = DescriptorCache::default();
        let key = DescriptorKey::SamplerIndices(vec![0, 7, 0]);
        let slot = DescriptorSlot {
            cpu_handle: D3D12_CPU_DESCRIPTOR_HANDLE { ptr: 16 },
            gpu_handle: D3D12_GPU_DESCRIPTOR_HANDLE { ptr: 32 },
            index: 3,
        };
        cache.insert(key.clone(), slot);
        assert_eq!(cache.retain(&key).unwrap().index, 3);
        assert!(!cache.release(3));
        assert!(cache.retain(&key).is_some());
        assert!(!cache.release(3));
        assert!(cache.release(3));
        assert!(cache.retain(&key).is_none());
    }

    #[test]
    #[ignore = "requires D3D12; verify sharing beyond the 4096-entry heap and fence retirement"]
    fn repeated_groups_share_native_descriptors() {
        let mut device = Dx12Device::new(&DeviceDescriptor::default()).unwrap();
        let layout = device
            .create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: None,
                entries: [
                    BindingType::UniformBuffer,
                    BindingType::StorageBuffer,
                    BindingType::SampledTexture,
                    BindingType::Sampler,
                ]
                .into_iter()
                .enumerate()
                .map(|(binding, binding_type)| BindGroupLayoutEntry {
                    binding: binding as u32,
                    binding_type,
                    stages: ShaderStages::FRAGMENT,
                })
                .collect(),
            })
            .unwrap();
        let buffer = device
            .create_buffer(&BufferDescriptor {
                label: None,
                size: 256,
                usage: BufferUsage::UNIFORM | BufferUsage::STORAGE,
                memory_location: MemoryLocation::CpuToGpu,
            })
            .unwrap();
        let texture = device
            .create_texture(&TextureDescriptor {
                label: None,
                size: gfx_core::Extent2d::new(1, 1).unwrap(),
                mip_level_count: 1,
                format: Format::Rgba8Unorm,
                usage: TextureUsage::SAMPLED,
                memory_location: MemoryLocation::GpuOnly,
                dimension: TextureDimension::D2,
                sample_count: 1,
            })
            .unwrap();
        let view = device
            .create_texture_view(&TextureViewDescriptor {
                label: None,
                texture,
                base_mip_level: 0,
                mip_level_count: 1,
                format: Format::Rgba8Unorm,
            })
            .unwrap();
        let sampler = device
            .create_sampler(&SamplerDescriptor::default())
            .unwrap();
        let entries = vec![
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer,
                    offset: 0,
                    size: 256,
                    stride: None,
                }),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer,
                    offset: 0,
                    size: 256,
                    stride: Some(4),
                }),
            },
            BindGroupEntry {
                binding: 2,
                resource: BindingResource::Texture(TextureBinding { texture_view: view }),
            },
            BindGroupEntry {
                binding: 3,
                resource: BindingResource::Sampler(SamplerBinding { sampler }),
            },
        ];
        let desc = BindGroupDescriptor {
            label: None,
            layout,
            entries,
        };
        let mut groups = Vec::new();
        for _ in 0..5000 {
            groups.push(device.create_bind_group(&desc).unwrap());
        }
        assert_eq!(device.resource_heap.shared.slots.len(), 4);
        assert_eq!(device.sampler_heap.shared.slots.len(), 1);
        let mut invalid = desc.clone();
        invalid.entries[3].resource = BindingResource::Sampler(SamplerBinding {
            sampler: SamplerId::from_parts(u32::MAX, 1),
        });
        assert!(device.create_bind_group(&invalid).is_err());
        assert!(
            device
                .resource_heap
                .shared
                .slots
                .values()
                .all(|entry| entry.references == 5000)
        );
        let survivor = groups.pop().unwrap();
        for group in groups {
            device.destroy_bind_group(group).unwrap();
        }
        device.wait_for_pending_work().unwrap();
        device.poll_cleanup();
        assert_eq!(device.resource_heap.shared.slots.len(), 4);
        let another = device.create_bind_group(&desc).unwrap();
        device.destroy_bind_group(survivor).unwrap();
        device.destroy_bind_group(another).unwrap();
        device.wait_for_pending_work().unwrap();
        device.poll_cleanup();
        assert!(device.resource_heap.shared.slots.is_empty());
        assert!(device.sampler_heap.shared.slots.is_empty());
        device.destroy_texture_view(view).unwrap();
        device.destroy_texture(texture).unwrap();
        device.destroy_sampler(sampler).unwrap();
        device.destroy_buffer(buffer).unwrap();
        device.destroy_bind_group_layout(layout).unwrap();
    }
}
