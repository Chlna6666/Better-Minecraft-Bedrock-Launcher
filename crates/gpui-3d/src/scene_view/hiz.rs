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
    shaders: [Option<ShaderModuleId>; 5],
    // Selected from projected occluder coverage; Shader variants are compiled at build time.
    raster_threads: u32,
    compute_caps: ComputeCapabilities,
    count: usize,
    clear_pending: bool,
    selection_eye: Option<Vec3>,
    // Last successfully uploaded Hi-Z box header. Tracking the exact encoded
    // bytes avoids a tiny driver upload on otherwise unchanged camera samples.
    uploaded_header: Option<[u8; 16]>,
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
        // Workgroup size is a compiled shader property. Pick only precompiled
        // variants supported by this device, never the largest reported size.
        let compute_caps = device.compute_capabilities();
        let profiles = raster_profiles();
        // Clear/reduce still use 64 threads, regardless of raster variant.
        // Reject unsupported compute up front instead of creating invalid
        // shader modules. Raster variants are independently capability-gated.
        if !compute_caps.supports_kernel(ComputeKernelProfile::one_dimensional(64)) {
            return Err(anyhow::anyhow!("Hi-Z requires a 64-thread compute workgroup"));
        }
        let default_variant = compute_caps.select_precompiled_kernel(&profiles, 64)
            .ok_or_else(|| anyhow::anyhow!("no supported Hi-Z raster workgroup"))?;
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
            for (index, entry) in [
                "cs_hiz_clear",
                if shadow { "cs_shadow_raster_32" } else { "cs_hiz_raster_32" },
                if shadow { "cs_shadow_raster_64" } else { "cs_hiz_raster_64" },
                if shadow { "cs_shadow_raster_128" } else { "cs_hiz_raster_128" },
                "cs_hiz_reduce",
            ].into_iter().enumerate() {
                let supported = match index {
                    0 | 4 => true, // fixed 64-thread kernels were validated above
                    raster => compute_caps.supports_kernel(profiles[raster - 1]),
                };
                shaders.push(if supported {
                    Some(super::generated::create_shader(device, backend, entry)?)
                } else {
                    None
                });
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
            for shader in shaders.into_iter().flatten() {
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
            raster_threads: profiles[default_variant].invocations(),
            compute_caps,
            clear_pending: false,
            selection_eye: None,
            uploaded_header: None,
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
            let header = encode_header(self.count, forward);
            if self.uploaded_header != Some(header) {
                device.write_buffer(self.boxes, 0, &header)?;
                self.uploaded_header = Some(header);
            }
            return Ok(());
        }
        let mut bounds = bounds
            .filter(|b| b.faces & 63 != 0 && Aabb::new(b.bounds.min, b.bounds.max).is_some())
            .collect::<Vec<_>>();
        // The closest small voxel proxies used to monopolize the 512-box
        // budget even when a slightly farther continuous roof or cliff would
        // cover far more of the image. Favor conservative projected face area
        // per distance, without a full sort or CPU visibility decisions.
        // Omitting a proxy cannot hide geometry: it only reduces Hi-Z rejection.
        if bounds.len() > self.max_boxes {
            bounds.select_nth_unstable_by(self.max_boxes, |a, b| {
                occluder_priority(b, eye).total_cmp(&occluder_priority(a, eye))
            });
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
        let desired = if count == 0 { 64 }
            else { choose_raster_threads(coverage_sum / count as f32) };
        let profiles = raster_profiles();
        let choice = self.compute_caps.select_precompiled_kernel(&profiles, desired)
            .ok_or_else(|| anyhow::anyhow!("Hi-Z raster variant exceeds device limits"))?;
        self.raster_threads = profiles[choice].invocations();
        let header = encode_header(count, forward);
        let mut bytes = header.to_vec();
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
        self.uploaded_header = Some(header);
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
        let clear = [depth];
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
        // All seven mip reduction bindings are tiny fixed-size arrays. Keep
        // them on the stack and submit their ordered stages as one batch.
        // Vulkan and D3D12 record one native command buffer with inter-stage
        // barriers; D3D11/OpenGL retain the ordered single-dispatch fallback.
        let reduction_bindings: [[ComputeBufferBinding; 2]; 7] =
            std::array::from_fn(|index| {
                let level = index + 1;
                [
                    depth,
                    binding(
                        3,
                        self.levels,
                        level * 256,
                        16,
                        ComputeBufferAccess::Uniform,
                    ),
                ]
            });
        let mut dispatches = Vec::with_capacity(if self.shadow { 2 } else { 9 });
        dispatches.push(ComputeDispatch {
            shader: self.shaders[0]
                .ok_or_else(|| anyhow::anyhow!("Hi-Z clear shader is unavailable"))?,
            bindings: &clear,
            workgroups: [(self.words as u32).div_ceil(64), 1, 1],
        });
        if self.count != 0 {
            dispatches.push(ComputeDispatch {
                shader: self.shaders[match self.raster_threads {
                    32 => 1, 128 => 3, _ => 2,
                }]
                .ok_or_else(|| anyhow::anyhow!("selected Hi-Z raster shader is unavailable"))?,
                bindings: &raster,
                workgroups: [(self.count * 6) as u32, 1, 1],
            });
            if !self.shadow {
                let reduce_shader = self.shaders[4]
                    .ok_or_else(|| anyhow::anyhow!("Hi-Z reduction shader is unavailable"))?;
                for (index, bindings) in reduction_bindings.iter().enumerate() {
                    let size = 128u32 >> (index + 1);
                    dispatches.push(ComputeDispatch {
                        shader: reduce_shader,
                        bindings,
                        workgroups: [(size * size).div_ceil(64), 1, 1],
                    });
                }
            }
        }
        // Dependence: clear -> raster -> mip1 -> ... -> mip7. Do not merge
        // the shaders themselves: the native backends insert memory barriers
        // between ordered dispatches in this single submission.
        device.dispatch_compute_batch(&dispatches)?;
        self.clear_pending = false;
        Ok(())
    }

    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        for shader in self.shaders.into_iter().flatten() {
            device.destroy_shader_module(shader)?;
        }
        for buffer in [self.depths, self.boxes, self.levels] {
            device.destroy_buffer(buffer)?;
        }
        Ok(())
    }
}

/// Estimated projected coverage for the axis-aligned faces that are actually
/// certified opaque. Uses distance to the nearest point, avoiding center-distance
/// bias against wide planar walls. The stabilizer prevents tiny nearby patches
/// from crowding out all large roofs and cliff faces. This is a selection hint,
/// never a visibility test.
fn occluder_priority(occluder: &Occluder, eye: Vec3) -> f32 {
    let extent = occluder.bounds.max - occluder.bounds.min;
    let mut area = 0.0_f32;
    if occluder.faces & 0b000011 != 0 {
        area = area.max(extent.y * extent.z);
    }
    if occluder.faces & 0b001100 != 0 {
        area = area.max(extent.x * extent.z);
    }
    if occluder.faces & 0b110000 != 0 {
        area = area.max(extent.x * extent.y);
    }
    let nearest = eye.max(occluder.bounds.min).min(occluder.bounds.max);
    let distance_sq = (nearest - eye).length_squared();
    area / (distance_sq + 16.0)
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
// Four aligned vec3 vertices, two uvec2 bounds and control words, rounded
// upward to 128 bytes to cover workgroup address-space layout padding.
fn raster_profiles() -> [ComputeKernelProfile; 3] {
    [32, 64, 128].map(|threads| ComputeKernelProfile {
        workgroup_size: [threads, 1, 1],
        shared_memory_bytes: 128,
    })
}

fn choose_raster_threads(mean_coverage: f32) -> u32 {
    if !mean_coverage.is_finite() { return 64; }
    if mean_coverage < 0.0025 { 32 }
    else if mean_coverage < 0.04 { 64 }
    else { 128 }
 }

#[cfg(test)]
mod occluder_selection_tests {
    use super::*;

    fn proxy(min: [f32; 3], max: [f32; 3], faces: u8) -> Occluder {
        Occluder {
            bounds: Aabb::new(
                Vec3::new(min[0], min[1], min[2]),
                Vec3::new(max[0], max[1], max[2]),
            ).unwrap(),
            faces,
        }
    }

    #[test]
    fn prioritizes_large_opaque_roofs_and_cliffs_over_nearby_tiny_faces() {
        let eye = Vec3::ZERO;
        let small = proxy([0., 0., 2.], [1., 1., 3.], 1 << 4);
        let roof = proxy([-8., 10., -8.], [8., 10., 8.], 1 << 3);
        let wall = proxy([8., -8., -8.], [8., 8., 8.], 1 << 1);
        assert!(occluder_priority(&roof, eye) > occluder_priority(&small, eye));
        assert!(occluder_priority(&wall, eye) > occluder_priority(&small, eye));
        assert_eq!(occluder_priority(&roof, eye), occluder_priority(&roof, eye));
        // Face masks are authoritative: a Y plane without a Y face is
        // not a certified occluder, regardless of its AABB footprint.
        let no_y_face = proxy([-8., 10., -8.], [8., 10., 8.], 1 << 1);
        assert_eq!(occluder_priority(&no_y_face, eye), 0.0);
    }

    #[test]
    fn selection_keeps_highest_coverage_without_sorting_every_occluder() {
        let eye = Vec3::ZERO;
        let mut proxies = (0..100)
            .map(|i| proxy([i as f32, 0., 1.], [i as f32 + 1., 1., 2.], 1 << 4))
            .collect::<Vec<_>>();
        let roof = proxy([-8., 10., -8.], [8., 10., 8.], 1 << 3);
        proxies.push(roof);
        proxies.select_nth_unstable_by(8, |a, b| {
            occluder_priority(b, eye).total_cmp(&occluder_priority(a, eye))
        });
        proxies.truncate(8);
        assert!(proxies.iter().any(|b| b.bounds == roof.bounds));
    }
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
        let profiles = super::raster_profiles();
        let caps = gfx_core::ComputeCapabilities {
            max_workgroups: [65535; 3],
            max_invocations_per_workgroup: 64,
            max_workgroup_size: [64, 64, 64],
            max_workgroup_shared_memory_bytes: 1024,
            indirect_draw: true,
        };
        assert_eq!(caps.select_precompiled_kernel(&profiles, 128), Some(1));
        assert_eq!(caps.select_precompiled_kernel(&profiles, 32), Some(0));
        assert!(!caps.supports_kernel(profiles[2]),
            "128-thread shader must not be created on a 64-thread device");
    }
 }
