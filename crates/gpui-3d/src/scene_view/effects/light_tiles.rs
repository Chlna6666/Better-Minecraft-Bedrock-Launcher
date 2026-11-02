use gfx_core::*;
use gpui::ExtensionDevice;

pub(super) struct LightTiles {
    pub buffer: BufferId,
    pub bytes: u64,
    pub size: [u32; 2],
    pub(super) shaders: [ShaderModuleId; 2],
}
impl LightTiles {
    pub(super) fn new(
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
        extent: Extent2d,
    ) -> gpui::Result<Self> {
        let size = [extent.width().div_ceil(16), extent.height().div_ceil(16)];
        let bytes = u64::from(size[0]) * u64::from(size[1]) * 65 * 4;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("gpui-3d tiled light lists".into()),
            size: bytes,
            usage: BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE,
            memory_location: MemoryLocation::GpuOnly,
        })?;
        let mut cleanup = super::construction::Construction::default();
        cleanup.record(buffer, |d, id| {
            let _ = d.destroy_buffer(id);
        });
        let result = (|| {
            let mut shaders = Vec::new();
            for entry in ["cs_clear_light_tiles", "cs_light_tiles"] {
                shaders.push(cleanup.record(
                    super::super::generated::create_shader(device, backend, entry)?,
                    |d, id| {
                        let _ = d.destroy_shader_module(id);
                    },
                ));
            }
            Ok(Self {
                buffer,
                bytes,
                size,
                shaders: [shaders[0], shaders[1]],
            })
        })();
        cleanup.finish(device, result)
    }
    pub(super) fn update(
        &self,
        device: &mut dyn ExtensionDevice,
        frame: BufferId,
        lights: (BufferId, u64, u32),
    ) -> gpui::Result<()> {
        let bindings = [
            ComputeBufferBinding {
                binding: 0,
                buffer: BufferBinding {
                    buffer: frame,
                    offset: 0,
                    size: 400,
                    stride: None,
                },
                access: ComputeBufferAccess::Uniform,
            },
            ComputeBufferBinding {
                binding: 1,
                buffer: BufferBinding {
                    buffer: lights.0,
                    offset: 0,
                    size: lights.1,
                    stride: Some(80),
                },
                access: ComputeBufferAccess::ReadOnly,
            },
            ComputeBufferBinding {
                binding: 2,
                buffer: BufferBinding {
                    buffer: self.buffer,
                    offset: 0,
                    size: self.bytes,
                    stride: Some(4),
                },
                access: ComputeBufferAccess::ReadWrite,
            },
        ];
        device.dispatch_compute(&ComputeDispatch {
            shader: self.shaders[0],
            bindings: &[bindings[0], bindings[2]],
            workgroups: [(self.size[0] * self.size[1]).div_ceil(64), 1, 1],
        })?;
        if lights.2 > 0 {
            device.dispatch_compute(&ComputeDispatch {
                shader: self.shaders[1],
                bindings: &bindings,
                workgroups: [lights.2.div_ceil(64), 1, 1],
            })?;
        }
        Ok(())
    }
    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        for shader in self.shaders {
            device.destroy_shader_module(shader)?;
        }
        device.destroy_buffer(self.buffer)?;
        Ok(())
    }
}
