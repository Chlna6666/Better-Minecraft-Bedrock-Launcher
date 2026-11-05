//! Buffer-backed, current-camera Hi-Z with conservative opaque-volume rasterization.
use crate::{Aabb, Occluder, Vec3};
use gfx_core::*;
use gpui::ExtensionDevice;

pub(super) const WORDS: usize = 21_845;
const MAX_BOXES: usize = 512;

pub(super) struct HiZ {
    pub(super) depths: BufferId,
    boxes: BufferId,
    levels: BufferId,
    shaders: [ShaderModuleId; 5],
    // Selected from projected occluder coverage; Shader variants are compiled at build time.
    raster_threads: u32,
    count: usize,
    clear_pending: bool,
    selection_eye: Option<Vec3>,
    words: usize,
    max_boxes: usize,
    shadow: bool,
}

impl HiZ {
    pub(super) fn has_occluders(&self) -> bool {
        self.count != 0
    }

    pub(super) fn create(
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
    ) -> gpui::Result<Self> {
        Self::create_mode(device, backend, false)
    }

    pub(super) fn create_shadow(
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
    ) -> gpui::Result<Self> {
        Self::create_mode(device, backend, true)
    }

    fn create_mode(
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
        shadow: bool,
    ) -> gpui::Result<Self> {
        let words = if shadow { 512 * 512 } else { WORDS };
        let max_boxes = if shadow { 2048 } else { MAX_BOXES };
        let mut buffers = Vec::new();
        let mut shaders = Vec::new();
        let result = (|| {
            for (size, usage) in [
                (
                    words * 4,
                    BufferUsage::STORAGE | BufferUsage::STORAGE_WRITE | BufferUsage::COPY_DST,
                ),
                (
                    16 + max_boxes * 32,
                    BufferUsage::STORAGE | BufferUsage::COPY_DST,
                ),
                (8 * 256, BufferUsage::UNIFORM | BufferUsage::COPY_DST),
            ] {
                buffers.push(device.create_buffer(&BufferDescriptor {
                    label: Some("gpui-3d Hi-Z".into()),
                    size: size as u64,
                    usage,
                    memory_location: super::gpu::buffer_memory_location(backend),
                })?);
            }
            for entry in [
                "cs_hiz_clear",
                if shadow { "cs_shadow_raster_32" } else { "cs_hiz_raster_32" },
                if shadow { "cs_shadow_raster_64" } else { "cs_hiz_raster_64" },
                if shadow { "cs_shadow_raster_128" } else { "cs_hiz_raster_128" },
                "cs_hiz_reduce",
            ] {
                shaders.push(super::generated::create_shader(device, backend, entry)?);
            }
            device.write_buffer(
                buffers[0],
                0,
                &vec![1.0f32.to_bits(); words]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )?;
            let mut levels = vec![0u8; 8 * 256];
            for level in 1..8 {
                levels[level * 256..level * 256 + 4].copy_from_slice(&(level as u32).to_le_bytes());
            }
            device.write_buffer(buffers[2], 0, &levels)?;
            Ok(())
        })();
        if let Err(error) = result {
            for shader in shaders {
                let _ = device.destroy_shader_module(shader);
            }
            for buffer in buffers {
                let _ = device.destroy_buffer(buffer);
            }
            return Err(error);
        }
        Ok(Self {
            depths: buffers[0],
            boxes: buffers[1],
            levels: buffers[2],
            shaders: shaders
                .try_into()
                .map_err(|_| anyhow::anyhow!("missing Hi-Z shaders"))?,
            count: 0,
            raster_threads: 64,
            clear_pending: false,
            selection_eye: None,
            words,
            max_boxes,
            shadow,
        })
    }

    pub(super) fn update(
        &mut self,
        device: &mut dyn ExtensionDevice,
        bounds: impl Iterator<Item = Occluder>,
        eye: Vec3,
        forward: Vec3,
        changed: bool,
    ) -> gpui::Result<()> {
        if !changed
            && self
                .selection_eye
                .is_some_and(|previous| (previous - eye).length_squared() < 256.)
        {
            device.write_buffer(self.boxes, 0, &encode_header(self.count, forward))?;
            return Ok(());
        }
        let mut bounds = bounds
            .filter(|b| b.faces & 63 != 0 && Aabb::new(b.bounds.min, b.bounds.max).is_some())
            .collect::<Vec<_>>();
        let key = |b: &Occluder| (eye.max(b.bounds.min).min(b.bounds.max) - eye).length_squared();
        if bounds.len() > self.max_boxes {
            bounds.select_nth_unstable_by(self.max_boxes, |a, b| key(a).total_cmp(&key(b)));
            bounds.truncate(self.max_boxes);
        }
        let count = bounds.len();
        // Estimate tile cost from occluder footprint, not just box count. Shadow
        // uses a 96-unit orthographic view; perspective Hi-Z uses distance to the
        // nearest point on the AABB as a conservative cost proxy. No GPU readback.
        let mut coverage_sum = 0.0_f32;
        for b in &bounds {
            let extent = b.bounds.max - b.bounds.min;
            let span = extent.x.max(extent.y).max(extent.z).max(0.0);
            let denominator = if self.shadow {
                96.0_f32 * 96.0
            } else {
                (eye.max(b.bounds.min).min(b.bounds.max) - eye)
                    .length_squared().max(1.0)
            };
            coverage_sum += (span * span / denominator).min(1.0);
        }
        self.raster_threads = if count == 0 {
            64
        } else {
            choose_raster_threads(coverage_sum / count as f32)
        };
        let mut bytes = encode_header(count, forward).to_vec();
        bytes.reserve(count * 32);
        for b in bounds {
            let bounds = b.bounds;
            for value in [
                bounds.min.x,
                bounds.min.y,
                bounds.min.z,
                0.,
                bounds.max.x,
                bounds.max.y,
                bounds.max.z,
            ] {
                bytes.extend(value.to_le_bytes());
            }
            bytes.extend(u32::from(b.faces & 63).to_le_bytes());
        }
        device.write_buffer(self.boxes, 0, &bytes)?;
        self.clear_pending |= self.count > 0;
        self.count = count;
        self.selection_eye = Some(eye);
        Ok(())
    }

    pub(super) fn dispatch(
        &mut self,
        device: &mut dyn ExtensionDevice,
        frame: BufferId,
    ) -> gpui::Result<()> {
        if self.count == 0 && !self.clear_pending {
            return Ok(());
        }
        let depth = binding(
            2,
            self.depths,
            0,
            self.words * 4,
            ComputeBufferAccess::ReadWrite,
        );
        device.dispatch_compute(&ComputeDispatch {
            shader: self.shaders[0],
            bindings: &[depth],
            workgroups: [(self.words as u32).div_ceil(64), 1, 1],
        })?;
        self.clear_pending = false;
        if self.count == 0 {
            return Ok(());
        }
        let raster = [
            binding(
                0,
                frame,
                0,
                super::FRAME_PARAMS_STRIDE,
                ComputeBufferAccess::Uniform,
            ),
            binding(
                1,
                self.boxes,
                0,
                16 + self.count * 32,
                ComputeBufferAccess::ReadOnly,
            ),
            depth,
        ];
        device.dispatch_compute(&ComputeDispatch {
            shader: self.shaders[match self.raster_threads { 32 => 1, 128 => 3, _ => 2 }],
            bindings: &raster,
            workgroups: [(self.count * 6) as u32, 1, 1],
        })?;
        for level in 1..if self.shadow { 1 } else { 8 } {
            let size = 128u32 >> level;
            let bindings = [
                depth,
                binding(
                    3,
                    self.levels,
                    level * 256,
                    16,
                    ComputeBufferAccess::Uniform,
                ),
            ];
            device.dispatch_compute(&ComputeDispatch {
                shader: self.shaders[4],
                bindings: &bindings,
                workgroups: [(size * size).div_ceil(64), 1, 1],
            })?;
        }
        Ok(())
    }

    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        for shader in self.shaders {
            device.destroy_shader_module(shader)?;
        }
        for buffer in [self.depths, self.boxes, self.levels] {
            device.destroy_buffer(buffer)?;
        }
        Ok(())
    }
}

fn encode_header(count: usize, forward: Vec3) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&(count as u32).to_le_bytes());
    for (dst, value) in bytes[4..]
        .chunks_exact_mut(4)
        .zip([forward.x, forward.y, forward.z])
    {
        dst.copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn binding(
    binding: u32,
    buffer: BufferId,
    offset: usize,
    size: usize,
    access: ComputeBufferAccess,
) -> ComputeBufferBinding {
    ComputeBufferBinding {
        binding,
        buffer: BufferBinding {
            buffer,
            offset: offset as u64,
            size: size as u64,
            stride: None,
        },
        access,
    }
}

// This is an inexpensive workload heuristic, not a claim of optimal hardware
// occupancy. The 32/64/128 variants stay below Vulkan's guaranteed minimum
// 128 invocations/workgroup; backend-specific wave widths are not yet exposed.
fn choose_raster_threads(mean_coverage: f32) -> u32 {
    if !mean_coverage.is_finite() { return 64; }
    if mean_coverage < 0.0025 { 32 }
    else if mean_coverage < 0.04 { 64 }
    else { 128 }
 }

#[cfg(test)]
mod adaptive_raster_tests {
    use super::choose_raster_threads;

    #[test]
    fn picks_safe_precompiled_sizes_for_estimated_pixel_cost() {
        assert_eq!(choose_raster_threads(0.0001), 32);
        assert_eq!(choose_raster_threads(0.01), 64);
        assert_eq!(choose_raster_threads(0.25), 128);
        assert_eq!(choose_raster_threads(f32::NAN), 64);
    }
 }
