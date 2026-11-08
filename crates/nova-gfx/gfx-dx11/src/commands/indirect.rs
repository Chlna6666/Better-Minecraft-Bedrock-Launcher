//! Pass-local GPU base offsets for Naga's b13 ABI.

use crate::{
    device::{backend, required},
    registry::Registry,
    resources::Buffer,
};
use gfx_core::{BufferId, BufferUsage, Error, MemoryTrimLevel, RenderCommand, Result};
use windows::Win32::Graphics::Direct3D11::*;

const PAGE_BYTES: u32 = 64 * 1024;
const SLOT_BYTES: u32 = 256;
const SLOTS_PER_PAGE: usize = (PAGE_BYTES / SLOT_BYTES) as usize;

/// Buffer and byte range for the two Naga b13 base offsets. Native indirect
/// records end with (first_vertex, first_instance) for non-indexed draws,
/// and (base_vertex, first_instance) for indexed draws.
pub(crate) fn constant_source(command: &RenderCommand) -> Option<(BufferId, u64, u64)> {
    match command {
        RenderCommand::DrawIndirect(draw) => Some((draw.arguments, draw.offset, 16)),
        RenderCommand::DrawIndexedIndirect(draw) => Some((draw.arguments, draw.offset, 20)),
        _ => None,
    }
}

pub(crate) struct IndirectConstants {
    pages: Vec<ID3D11Buffer>,
    supports_offsets: bool,
    active: bool,
}

impl IndirectConstants {
    pub(crate) fn new(device: &ID3D11Device) -> Self {
        let mut options = D3D11_FEATURE_DATA_D3D11_OPTIONS::default();
        // SAFETY: options is the initialized output structure for this feature query.
        let supports_offsets = unsafe {
            device.CheckFeatureSupport(
                D3D11_FEATURE_D3D11_OPTIONS,
                (&mut options as *mut D3D11_FEATURE_DATA_D3D11_OPTIONS).cast(),
                std::mem::size_of_val(&options) as u32,
            )
        }
        .is_ok()
            && options.ConstantBufferOffsetting.as_bool();
        Self {
            pages: Vec::new(),
            supports_offsets,
            active: false,
        }
    }

    pub(crate) fn stage(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext1,
        buffers: &Registry<Buffer>,
        commands: &[RenderCommand],
    ) -> Result<()> {
        self.active = false;
        if !self.supports_offsets {
            return Ok(());
        }
        let mut count = 0usize;
        for command in commands {
            if let Some((buffer_id, offset, bytes)) = constant_source(command) {
                let buffer = buffers.get(buffer_id)?;
                if !buffer.desc.usage.contains(BufferUsage::INDIRECT)
                    || offset % 4 != 0
                    || offset
                        .checked_add(bytes)
                        .is_none_or(|end| end > buffer.desc.size)
                {
                    return Err(Error::InvalidInput(
                        "indirect argument range/usage mismatch".into(),
                    ));
                }
                count += 1;
            }
        }
        if count == 0 {
            return Ok(());
        }
        while self.pages.len() < count.div_ceil(SLOTS_PER_PAGE) {
            let mut page = None;
            // SAFETY: the descriptor is initialized and no initial data is read.
            unsafe {
                device.CreateBuffer(
                    &D3D11_BUFFER_DESC {
                        ByteWidth: PAGE_BYTES,
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut page),
                )
            }
            .map_err(backend)?;
            self.pages.push(required(page)?);
        }
        let mut index = 0;
        for command in commands {
            let Some((buffer_id, argument_offset, argument_bytes)) = constant_source(command) else {
                continue;
            };
            let source = buffers.get(buffer_id)?;
            let (page, offset) = slot(index);
            let region = D3D11_BOX {
                left: (argument_offset + argument_bytes - 8) as u32,
                right: (argument_offset + argument_bytes) as u32,
                top: 0,
                bottom: 1,
                front: 0,
                back: 1,
            };
            // SAFETY: every source range was validated above; each destination owns a
            // distinct aligned slot. All writes precede this pass's first graphics draw.
            unsafe {
                context.CopySubresourceRegion(
                    &self.pages[page],
                    0,
                    offset,
                    0,
                    0,
                    &source.native,
                    0,
                    Some(&region),
                );
            }
            index += 1;
        }
        self.active = true;
        Ok(())
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn bind(&self, context: &ID3D11DeviceContext1, index: usize) -> Result<()> {
        let (page, offset) = slot(index);
        let buffer = self
            .pages
            .get(page)
            .ok_or_else(|| Error::InvalidInput("indirect constants were not staged".into()))?;
        let values = [Some(buffer.clone())];
        let first = offset / 16;
        let count = SLOT_BYTES / 16;
        // SAFETY: the page is retained; offset and count are multiples of 16 constants,
        // and the range lies within its 64 KiB allocation. The b13 ABI is unchanged.
        unsafe {
            context.VSSetConstantBuffers1(13, 1, Some(values.as_ptr()), Some(&first), Some(&count));
        }
        Ok(())
    }

    pub(crate) fn trim(&mut self, level: MemoryTrimLevel) {
        self.active = false;
        self.pages
            .truncate(usize::from(level != MemoryTrimLevel::Aggressive));
        self.pages.shrink_to_fit();
    }
}

fn slot(index: usize) -> (usize, u32) {
    (
        index / SLOTS_PER_PAGE,
        (index % SLOTS_PER_PAGE) as u32 * SLOT_BYTES,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_and_nonindexed_constants_use_distinct_native_offsets() {
        use gfx_core::*;
        let buffer = ResourceId::new(3);
        let simple = RenderCommand::DrawIndirect(DrawIndirectCommand {
            pipeline: ResourceId::new(1),
            bind_groups: bind_group_list_empty(),
            arguments: buffer,
            offset: 32,
            scissor: None,
        });
        let indexed = RenderCommand::DrawIndexedIndirect(DrawIndexedIndirectCommand {
            pipeline: ResourceId::new(1),
            bind_groups: bind_group_list_empty(),
            arguments: buffer,
            offset: 64,
            index_buffer: IndexBufferBinding {
                buffer: ResourceId::new(2),
                format: IndexFormat::Uint16,
                offset: 0,
            },
            scissor: None,
        });
        assert_eq!(constant_source(&simple), Some((buffer, 32, 16)));
        assert_eq!(constant_source(&indexed), Some((buffer, 64, 20)));
        assert_eq!(32 + 16 - 8, 40);
        assert_eq!(64 + 20 - 8, 76);
    }

    #[test]
    fn slots_are_aligned_and_cross_pages_without_overlap() {
        assert_eq!(slot(0), (0, 0));
        assert_eq!(slot(255), (0, PAGE_BYTES - SLOT_BYTES));
        assert_eq!(slot(256), (1, 0));
        assert_eq!(slot(257), (1, SLOT_BYTES));
    }

    #[test]
    #[ignore = "requires Windows D3D11 hardware; no window is created"]
    fn native_staging_reuses_pages_and_keeps_legacy_fallback() {
        use gfx_core::*;
        let mut device = crate::Dx11Device::new(&DeviceDescriptor::default()).unwrap();
        eprintln!(
            "indirect constant offset support={}",
            device.indirect_constants.supports_offsets
        );
        assert!(device.indirect_constants.supports_offsets);
        let arguments = device
            .create_buffer(&BufferDescriptor {
                label: None,
                size: 16,
                usage: BufferUsage::INDIRECT | BufferUsage::COPY_DST,
                memory_location: MemoryLocation::GpuOnly,
            })
            .unwrap();
        device.write_buffer(arguments, 0, &[0; 16]).unwrap();
        let command = RenderCommand::DrawIndirect(DrawIndirectCommand {
            pipeline: RenderPipelineId::from_parts(1, 1),
            bind_groups: bind_group_list_empty(),
            arguments,
            offset: 0,
            scissor: None,
        });
        let commands = vec![command; SLOTS_PER_PAGE + 1];
        for _ in 0..2 {
            device
                .indirect_constants
                .stage(&device.native, &device.context, &device.buffers, &commands)
                .unwrap();
            assert!(device.indirect_constants.is_active());
            assert_eq!(device.indirect_constants.pages.len(), 2);
        }
        device.wait_queue_idle().unwrap();
        device.indirect_constants.trim(MemoryTrimLevel::Moderate);
        assert_eq!(device.indirect_constants.pages.len(), 1);
        device.indirect_constants.supports_offsets = false;
        device
            .indirect_constants
            .stage(&device.native, &device.context, &device.buffers, &commands)
            .unwrap();
        assert!(!device.indirect_constants.is_active());
        device.indirect_constants.trim(MemoryTrimLevel::Aggressive);
        assert!(device.indirect_constants.pages.is_empty());
        device.destroy_buffer(arguments).unwrap();
    }
}
