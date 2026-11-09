//! Device-limit-aware Froxel volume planning.
//!
//! This module only plans a *true 3D* GPU texture layout. It deliberately does
//! not allocate a 2D atlas, silently substitute a buffer, or dispatch a shader:
//! gfx-core's current TextureDimension ABI exposes D2 and D2Array only.
//! Native Texture3D/UAV support must be proven on every enabled backend before
//! allocation and the new ordered Graphics -> Compute -> Graphics path are used.

use gfx_core::{ComputeCapabilities, ComputeKernelProfile, Extent2d};

pub(super) const FROXEL_DEPTH_SLICES: u32 = 64;
pub(super) const MAX_FROXEL_BYTES: u64 = 64 * 1024 * 1024;
const INITIAL_TILE_PIXELS: u32 = 16;
const MAX_TILE_PIXELS: u32 = 256;
/// Two RGBA16F volumes: scattering/extinction and integrated radiance.
const VOLUME_COUNT: u64 = 2;
const BYTES_PER_RGBA16F: u64 = 8;

/// This must match the local workgroup size of the eventual compiled WGSL
/// entry points. It is a legality check, *not* a claim of optimal occupancy.
pub(super) const FROXEL_KERNEL: ComputeKernelProfile = ComputeKernelProfile {
    workgroup_size: [8, 8, 4],
    shared_memory_bytes: 0,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FroxelGrid {
    pub tile_pixels: u32,
    /// Physical texture width, height and depth; unlike a 2D atlas these
    /// dimensions correspond to x/y/z coordinates in texture_3d.
    pub extent: [u32; 3],
    pub workgroups: [u32; 3],
    /// Upper bound on uncompressed texels for the two logical volumes.
    /// Does not include native image padding, metadata, or history textures.
    pub texture_bytes: u64,
}

impl FroxelGrid {
    /// Plan a froxel grid from actual framebuffer size and compute limits.
    /// None retains the existing fragment-integrated fog without an unsafe
    /// backend override. High-resolution windows increase tile size before
    /// allocating unbounded 3D images.
    pub fn plan(extent: Extent2d, caps: ComputeCapabilities) -> Option<Self> {
        if !caps.supports_kernel(FROXEL_KERNEL) {
            return None;
        }
        let mut tile_pixels = INITIAL_TILE_PIXELS;
        loop {
            let x = extent.width().div_ceil(tile_pixels);
            let y = extent.height().div_ceil(tile_pixels);
            let z = FROXEL_DEPTH_SLICES;
            let voxels = u64::from(x)
                .checked_mul(u64::from(y))?
                .checked_mul(u64::from(z))?;
            let texture_bytes = voxels
                .checked_mul(BYTES_PER_RGBA16F)?
                .checked_mul(VOLUME_COUNT)?;
            let workgroups = [
                x.div_ceil(FROXEL_KERNEL.workgroup_size[0]),
                y.div_ceil(FROXEL_KERNEL.workgroup_size[1]),
                z.div_ceil(FROXEL_KERNEL.workgroup_size[2]),
            ];
            if texture_bytes <= MAX_FROXEL_BYTES
                && workgroups.iter().zip(caps.max_workgroups)
                    .all(|(&count, limit)| count > 0 && count <= limit)
            {
                return Some(Self {
                    tile_pixels,
                    extent: [x, y, z],
                    workgroups,
                    texture_bytes,
                });
            }
            if tile_pixels >= MAX_TILE_PIXELS {
                return None;
            }
            tile_pixels = tile_pixels.checked_mul(2)?;
        }
    }

    /// Device-qualified volume grid: reject unsupported 3D sampled/storage
    /// formats instead of using a 2DArray fallback that changes shader semantics.
    pub fn plan_native(
        extent: Extent2d,
        caps: ComputeCapabilities,
        volume: gfx_core::TextureVolumeCapabilities,
    ) -> Option<Self> {
        if !volume.sampled || !volume.storage ||
            volume.max_dimension_3d < FROXEL_DEPTH_SLICES
        {
            return None;
        }
        let mut grid = Self::plan(extent, caps)?;
        while grid.extent[0].max(grid.extent[1]) > volume.max_dimension_3d {
            if grid.tile_pixels >= MAX_TILE_PIXELS {
                return None;
            }
            grid.tile_pixels = grid.tile_pixels.checked_mul(2)?;
            grid.extent[0] = extent.width().div_ceil(grid.tile_pixels);
            grid.extent[1] = extent.height().div_ceil(grid.tile_pixels);
            grid.workgroups[0] = grid.extent[0].div_ceil(FROXEL_KERNEL.workgroup_size[0]);
            grid.workgroups[1] = grid.extent[1].div_ceil(FROXEL_KERNEL.workgroup_size[1]);
            grid.texture_bytes = u64::from(grid.extent[0])
                .checked_mul(u64::from(grid.extent[1]))?
                .checked_mul(u64::from(grid.extent[2]))?
                .checked_mul(BYTES_PER_RGBA16F)?
                .checked_mul(VOLUME_COUNT)?;
        }
        if grid.workgroups.iter().zip(caps.max_workgroups)
            .any(|(&count, maximum)| count > maximum)
        { return None; }
        if !caps.supports_kernel(ComputeKernelProfile {
            workgroup_size: [8, 8, 1], shared_memory_bytes: 0
        }) { return None; }
        Some(grid)
    }

    /// Center of a logarithmically distributed depth slice (in world units).
    /// The matching shader must use the same mapping for light injection,
    /// integration and full-resolution composite sampling.
    pub fn depth_at_slice(self, index: u32, near: f32, far: f32) -> Option<f32> {
        if index >= self.extent[2] || !near.is_finite() || !far.is_finite()
            || near <= 0.0 || far <= near
        {
            return None;
        }
        let t = (index as f32 + 0.5) / self.extent[2] as f32;
        // Log interpolation avoids overflowing far/near for otherwise
        // finite, valid clipping distances.
        Some((near.ln() * (1.0 - t) + far.ln() * t).exp())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capable() -> ComputeCapabilities {
        ComputeCapabilities {
            max_workgroups: [65535, 65535, 65535],
            max_invocations_per_workgroup: 1024,
            max_workgroup_size: [1024, 1024, 64],
            max_workgroup_shared_memory_bytes: 32768,
            indirect_draw: true,
        }
    }

    #[test]
    fn full_hd_grid_uses_3d_dispatch_and_bounded_storage() {
        let grid = FroxelGrid::plan(Extent2d::new(1920, 1080).unwrap(), capable()).unwrap();
        assert_eq!(grid.extent, [120, 68, FROXEL_DEPTH_SLICES]);
        assert_eq!(grid.workgroups, [15, 9, 16]);
        assert_eq!(grid.tile_pixels, 16);
        assert!(grid.texture_bytes < MAX_FROXEL_BYTES);
    }

    #[test]
    fn ultra_high_resolution_reduces_grid_density_before_exceeding_budget() {
        let grid = FroxelGrid::plan(Extent2d::new(7680, 4320).unwrap(), capable()).unwrap();
        assert!(grid.tile_pixels > INITIAL_TILE_PIXELS);
        assert!(grid.texture_bytes <= MAX_FROXEL_BYTES);
        assert_eq!(grid.extent[2], FROXEL_DEPTH_SLICES);
    }

    #[test]
    fn volume_device_limits_select_an_appropriate_tile_size() {
        let caps = gfx_core::TextureVolumeCapabilities {
            max_dimension_3d: 128,
            sampled: true,
            storage: true,
        };
        let grid = FroxelGrid::plan_native(
            Extent2d::new(3840,2160).unwrap(), capable(), caps
        ).unwrap();
        assert!(grid.extent[0] <= 128);
        assert_eq!(grid.extent[2], 64);
        assert!(FroxelGrid::plan_native(
            Extent2d::new(1920,1080).unwrap(), capable(),
            gfx_core::TextureVolumeCapabilities::default()
        ).is_none());
    }

    #[test]
    fn unavailable_compute_limits_keep_existing_fog_path() {
        assert!(FroxelGrid::plan(
            Extent2d::new(1920, 1080).unwrap(),
            ComputeCapabilities::default()
        ).is_none());
        let mut limits = capable();
        limits.max_workgroups[2] = 1;
        assert!(FroxelGrid::plan(Extent2d::new(1920, 1080).unwrap(), limits).is_none());
    }

    #[test]
    fn logarithmic_depth_is_monotonic_and_stays_in_frustum() {
        let grid = FroxelGrid::plan(Extent2d::new(800, 600).unwrap(), capable()).unwrap();
        let mut previous = 0.0;
        for z in 0..grid.extent[2] {
            let current = grid.depth_at_slice(z, 0.1, 512.0).unwrap();
            assert!(current > previous && current < 512.0);
            previous = current;
        }
        assert!(grid.depth_at_slice(grid.extent[2], 0.1, 512.0).is_none());
        assert!(grid.depth_at_slice(0, 0.0, 512.0).is_none());
    }
}


use gfx_core::*;
use gpui::{ExtensionDevice, RendererExtensionComputePass};
use super::targets::Target;
use super::ScreenEffects;
use super::super::SceneView;

/// Shared native compute programs and descriptor layouts, one per GPU owner.
/// SceneView instances only own their small 3D textures and bind groups.
#[derive(Clone, Copy)]
pub(super) struct FroxelProgram {
    inject_layout: BindGroupLayoutId,
    integrate_layout: BindGroupLayoutId,
    inject_shader: ShaderModuleId,
    integrate_shader: ShaderModuleId,
}

impl FroxelProgram {
    pub fn new(
        device: &mut dyn ExtensionDevice,
        context: &gpui::RendererExtensionContext,
    ) -> gpui::Result<Self> {
        let mut cleanup = super::construction::Construction::default();
        let result = (|| {
            let inject_layout = cleanup.record(
                device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                    label: Some("gpui-3d froxel injection bindings".into()),
                    entries: (0..7).map(|binding| BindGroupLayoutEntry {
                        binding,
                        binding_type: match binding {
                            0 => BindingType::UniformBuffer,
                            1 | 2 => BindingType::StorageBuffer,
                            6 => BindingType::StorageTexture {
                                format: Format::Rgba16Float, read_only: false,
                            },
                            _ => BindingType::SampledTexture,
                        },
                        stages: ShaderStages::COMPUTE,
                    }).collect(),
                })?,
                |d, id| { let _ = d.destroy_bind_group_layout(id); },
            );
            let integrate_layout = cleanup.record(
                device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                    label: Some("gpui-3d froxel integration bindings".into()),
                    entries: vec![
                        BindGroupLayoutEntry {
                            binding: 0, binding_type: BindingType::SampledTexture,
                            stages: ShaderStages::COMPUTE,
                        },
                        BindGroupLayoutEntry {
                            binding: 1,
                            binding_type: BindingType::StorageTexture {
                                format: Format::Rgba16Float, read_only: false,
                            },
                            stages: ShaderStages::COMPUTE,
                        },
                    ],
                })?,
                |d, id| { let _ = d.destroy_bind_group_layout(id); },
            );
            let inject_shader = cleanup.record(
                device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("cs_froxel_inject".into()),
                    binary: super::super::resources::embedded_shader(
                        context, ShaderStage::Compute, "cs_froxel_inject",
                    )?,
                })?,
                |d, id| { let _ = d.destroy_shader_module(id); },
            );
            let integrate_shader = cleanup.record(
                device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("cs_froxel_integrate".into()),
                    binary: super::super::resources::embedded_shader(
                        context, ShaderStage::Compute, "cs_froxel_integrate",
                    )?,
                })?,
                |d, id| { let _ = d.destroy_shader_module(id); },
            );
            Ok(Self {
                inject_layout, integrate_layout, inject_shader, integrate_shader,
            })
        })();
        cleanup.finish(device, result)
    }

    pub fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        device.destroy_shader_module(self.inject_shader)?;
        device.destroy_shader_module(self.integrate_shader)?;
        device.destroy_bind_group_layout(self.inject_layout)?;
        Ok(device.destroy_bind_group_layout(self.integrate_layout)?)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InjectBindings {
    frame: BufferId,
    lights: (BufferId, u64),
    tiles: (BufferId, u64),
    shadows: [TextureViewId; 3],
}

/// Two independent GPU-native 3D volumes. Integration reads one image and
/// writes the other, so no sampled/UAV aliasing is ever legal or required.
pub(super) struct FroxelVolume {
    pub grid: FroxelGrid,
    injection: Target,
    pub integrated: Target,
    injection_group: Option<BindGroupId>,
    integration_group: BindGroupId,
    binding_key: Option<InjectBindings>,
    pub valid: bool,
    pub queued: bool,
    last_view: Option<SceneView>,
    last_settings: Option<ScreenEffects>,
    last_scene_version: u64,
    last_lights: Option<(BufferId, u32)>,
}

impl FroxelVolume {
    pub fn injection_texture(&self) -> TextureId {
        self.injection.texture
    }

    pub fn new(
        device: &mut dyn ExtensionDevice,
        program: &FroxelProgram,
        grid: FroxelGrid,
    ) -> gpui::Result<Self> {
        let mut cleanup = super::construction::Construction::default();
        let result = (|| {
            let create = |device: &mut dyn ExtensionDevice| -> gpui::Result<Target> {
                let desc = TextureDescriptor {
                    label: Some("gpui-3d froxel 3D RGBA16F".into()),
                    size: Extent2d::new(grid.extent[0], grid.extent[1])?,
                    mip_level_count: 1,
                    format: Format::Rgba16Float,
                    usage: TextureUsage::SAMPLED | TextureUsage::STORAGE,
                    memory_location: MemoryLocation::GpuOnly,
                    dimension: TextureDimension::D3 { depth: grid.extent[2] },
                    sample_count: 1,
                };
                device.texture_volume_capabilities().validate(&desc)?;
                let texture = device.create_texture(&desc)?;
                let view = match device.create_texture_view(&TextureViewDescriptor {
                    label: None, texture, base_mip_level: 0,
                    mip_level_count: 1, format: desc.format,
                }) {
                    Ok(view) => view,
                    Err(err) => {
                        let _ = device.destroy_texture(texture);
                        return Err(err.into());
                    }
                };
                Ok(Target { texture, view })
            };
            let injection = create(device)?;
            injection.record(&mut cleanup);
            let integrated = create(device)?;
            integrated.record(&mut cleanup);
            let integration_group = cleanup.record(
                device.create_bind_group(&BindGroupDescriptor {
                    label: Some("gpui-3d froxel volume integration".into()),
                    layout: program.integrate_layout,
                    entries: vec![
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::Texture(TextureBinding {
                                texture_view: injection.view,
                            }),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Texture(TextureBinding {
                                texture_view: integrated.view,
                            }),
                        },
                    ],
                })?,
                |d, group| { let _ = d.destroy_bind_group(group); },
            );
            Ok(Self {
                grid, injection, integrated, injection_group: None,
                integration_group, binding_key: None,
                valid: false, queued: false, last_view: None,
                last_settings: None, last_scene_version: 0, last_lights: None,
            })
        })();
        cleanup.finish(device, result)
    }

    /// A previously submitted volume is reusable only when *all* inputs that
    /// influence its compute path are unchanged, including the scene revision.
    /// No reprojection claim is made on a moving camera: recompute instead.
    pub fn reusable(
        &self, view: &SceneView, settings: ScreenEffects,
        lights: (BufferId, u32), tile_updated: bool, shadow_updated: bool,
    ) -> bool {
        self.valid && !self.queued && !tile_updated && !shadow_updated
            && self.last_scene_version == view.scene.version()
            && self.last_settings == Some(settings)
            && self.last_lights == Some(lights)
            && self.last_view.as_ref().is_some_and(|previous|
                super::renderer::same_capture(previous, view))
    }

    fn bind_injection(
        &mut self, device: &mut dyn ExtensionDevice,
        program: &FroxelProgram, key: InjectBindings,
    ) -> gpui::Result<BindGroupId> {
        if self.binding_key == Some(key) {
            if let Some(group) = self.injection_group { return Ok(group); }
        }
        let mut entries = Vec::with_capacity(7);
        for (binding, buffer, bytes) in [
            (0, key.frame, super::FRAME_BYTES),
            (1, key.lights.0, key.lights.1),
            (2, key.tiles.0, key.tiles.1),
        ] {
            entries.push(BindGroupEntry {
                binding,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer, offset: 0, size: bytes, stride: None,
                }),
            });
        }
        for (index, view) in key.shadows.into_iter().enumerate() {
            entries.push(BindGroupEntry {
                binding: 3 + index as u32,
                resource: BindingResource::Texture(TextureBinding { texture_view: view }),
            });
        }
        entries.push(BindGroupEntry {
            binding: 6,
            resource: BindingResource::Texture(TextureBinding {
                texture_view: self.injection.view,
            }),
        });
        let group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("gpui-3d froxel shadow/light injection".into()),
            layout: program.inject_layout,
            entries,
        })?;
        if let Some(old) = self.injection_group.replace(group) {
            device.destroy_bind_group(old)?;
        }
        self.binding_key = Some(key);
        Ok(group)
    }

    /// Queue exactly two GPU work stages after the last shadow/geometry pass
    /// and before the screen composite. The GPU owner handles submission and
    /// native barriers; neither dispatch waits for a CPU fence.
    pub fn queue(
        &mut self,
        device: &mut dyn ExtensionDevice,
        program: &FroxelProgram,
        frame: BufferId, lights: (BufferId, u64, u32),
        tiles: (BufferId, u64), shadows: [TextureViewId; 3],
        after_graphics_pass: usize,
        queue: &mut Vec<RendererExtensionComputePass>,
        batch_id: u64,
        profiled: bool,
        view: &SceneView,
        settings: ScreenEffects,
    ) -> gpui::Result<()> {
        let group = self.bind_injection(device, program, InjectBindings {
            frame, lights: (lights.0, lights.1), tiles, shadows,
        })?;
        let injection = ComputeBindGroupDispatch {
            shader: program.inject_shader,
            bind_group: group,
            workgroups: self.grid.workgroups,
        };
        let integration = ComputeBindGroupDispatch {
            shader: program.integrate_shader,
            bind_group: self.integration_group,
            workgroups: [
                self.grid.extent[0].div_ceil(8),
                self.grid.extent[1].div_ceil(8),
                1,
            ],
        };
        let caps = device.compute_capabilities();
        injection.validate(caps)?;
        integration.validate(caps)?;
        queue.push(RendererExtensionComputePass {
            after_graphics_pass, dispatch: injection,
            gpu_work_tag: profiled.then_some(GpuTimestampTag {
                batch_id, stage: GpuTimestampStage::FroxelInjection,
            }),
        });
        queue.push(RendererExtensionComputePass {
            after_graphics_pass, dispatch: integration,
            gpu_work_tag: profiled.then_some(GpuTimestampTag {
                batch_id, stage: GpuTimestampStage::FroxelIntegration,
            }),
        });
        self.queued = true;
        self.valid = false;
        self.last_view = Some(view.clone());
        self.last_settings = Some(settings);
        self.last_scene_version = view.scene.version();
        self.last_lights = Some((lights.0, lights.2));
        Ok(())
    }

    pub fn submitted(&mut self) {
        if self.queued {
            self.queued = false;
            self.valid = true;
        }
    }

    pub fn invalidate(&mut self) {
        self.valid = false;
        self.queued = false;
    }

    pub fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        if let Some(group) = self.injection_group {
            device.destroy_bind_group(group)?;
        }
        device.destroy_bind_group(self.integration_group)?;
        self.injection.destroy(device)?;
        self.integrated.destroy(device)
    }
}
