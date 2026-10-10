//! Owner-thread sampling of retained resources; no allocation policy changes.
use super::*;
use crate::{
    BufferMemoryProfile, DeviceMemoryProfile, MemoryBudgetProfile, MemoryUsage,
    RendererMemoryProfile,
};
use gfx_core::{
    DeviceMemoryBudget, DiagnosticsDevice, MemoryAccounting, MemoryBudget, ResourceStats,
};

impl NovaBackend {
    pub(super) fn compact_memory(&mut self) -> gfx_core::Result<gfx_core::MemoryCompactReport> {
        #[allow(unreachable_patterns)]
        match self {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => device.compact_memory(),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => device.compact_memory(),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => device.compact_memory(),
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(device) => device.compact_memory(),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => device.compact_memory(),
            _ => Ok(Default::default()),
        }
    }
    fn memory_diagnostics(&self) -> (ResourceStats, gfx_core::Result<Option<DeviceMemoryBudget>>) {
        #[allow(unreachable_patterns)]
        match self {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            Self::Dx11(device) => (device.resource_stats(), device.memory_budget()),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => (device.resource_stats(), device.memory_budget()),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => (device.resource_stats(), device.memory_budget()),
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            Self::OpenGl(device) => (device.resource_stats(), device.memory_budget()),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => (device.resource_stats(), device.memory_budget()),
            _ => (ResourceStats::default(), Ok(None)),
        }
    }
}

fn budget_profile(budget: MemoryBudget) -> MemoryBudgetProfile {
    MemoryBudgetProfile {
        usage_bytes: budget.usage_bytes,
        budget_bytes: budget.budget_bytes,
        headroom_bytes: budget.headroom_bytes(),
        over_budget_bytes: budget.over_budget_bytes(),
        utilization_percent: budget.utilization(),
    }
}

impl FrameResourceBuffers {
    fn memory_capacities(self) -> [usize; 12] {
        [
            GLOBAL_UPLOAD_BYTES,
            TEXT_RASTER_UPLOAD_BYTES,
            self.quad_capacity * PACKED_QUAD_BYTES,
            self.shadow_capacity * PACKED_SHADOW_BYTES,
            self.path_rasterization_vertex_capacity * PACKED_PATH_RASTERIZATION_VERTEX_BYTES,
            self.path_sprite_capacity * PACKED_PATH_SPRITE_BYTES,
            self.mono_sprite_capacity * PACKED_MONO_SPRITE_BYTES,
            self.poly_sprite_capacity * PACKED_POLY_SPRITE_BYTES,
            MAX_UNDERLINES * PACKED_UNDERLINE_BYTES,
            MAX_BACKDROP_BLURS * 2 * BACKDROP_BLUR_PASS_BYTES,
            MAX_BACKDROP_BLURS * PACKED_BACKDROP_BLUR_BYTES,
            self.animation_value_capacity * PACKED_ANIMATION_VALUE_BYTES,
        ]
    }
}

impl NovaRenderer {
    pub(super) fn sample_memory(&mut self, force: bool, render_error: Option<&anyhow::Error>) {
        if self.destroyed || (!force && !self.memory_profile.due()) {
            return;
        }
        let mut window = self.renderer_memory_profile();
        window.render_error = render_error.map(|error| format!("{error:#}"));
        let device = (force || self.memory_profile.device_due()).then(|| {
            let (stats, budget) = lock_backend(&self.backend).memory_diagnostics();
            let (budget, budget_error) = match budget {
                Ok(budget) => (budget.unwrap_or_default(), None),
                Err(error) => (DeviceMemoryBudget::default(), Some(error.to_string())),
            };
            let available = stats.memory_accounting != MemoryAccounting::Unavailable;
            DeviceMemoryProfile {
                binding_mirror_bytes: stats.binding_mirror_bytes,
                backend: self.backend_info.label().to_string(),
                adapter_name: self.backend_info.adapter_name().to_string(),
                accounting: match stats.memory_accounting {
                    MemoryAccounting::Unavailable => "unavailable",
                    MemoryAccounting::ResourceSizes => "resource-sizes",
                    MemoryAccounting::Allocator => "allocator",
                }
                .to_string(),
                allocated_bytes: available.then_some(stats.allocated_bytes),
                reserved_bytes: available.then_some(stats.reserved_bytes),
                cpu_shadow_bytes: stats.cpu_shadow_bytes,
                upload_ring: MemoryUsage {
                    used_bytes: stats.upload_used_bytes,
                    capacity_bytes: stats.upload_capacity_bytes,
                },
                local_budget: budget.local.map(budget_profile),
                non_local_budget: budget.non_local.map(budget_profile),
                budget_error,
                ..DeviceMemoryProfile::default()
            }
        });
        self.memory_profile.publish(window, device);
    }

    pub(super) fn buffer_memory_profile(&self) -> Vec<BufferMemoryProfile> {
        let upload = &self.frame_upload;
        let streams = [
            ("globals", upload.globals.len(), upload.globals.capacity()),
            (
                "text-raster",
                upload.text_raster_params.len(),
                upload.text_raster_params.capacity(),
            ),
            ("quads", upload.quads.owned_len(), upload.quads.capacity()),
            ("shadows", upload.shadows.len(), upload.shadows.capacity()),
            (
                "path-vertices",
                upload.path_rasterization_vertices.len(),
                upload.path_rasterization_vertices.capacity(),
            ),
            (
                "path-sprites",
                upload.path_sprites.len(),
                upload.path_sprites.capacity(),
            ),
            (
                "mono-sprites",
                upload.mono_sprites.len(),
                upload.mono_sprites.capacity(),
            ),
            (
                "poly-sprites",
                upload.poly_sprites.len(),
                upload.poly_sprites.capacity(),
            ),
            (
                "underlines",
                upload.underlines.len(),
                upload.underlines.capacity(),
            ),
            (
                "blur-passes",
                upload.backdrop_blur_passes.len(),
                upload.backdrop_blur_passes.capacity(),
            ),
            (
                "blur-records",
                upload.backdrop_blurs.len(),
                upload.backdrop_blurs.capacity(),
            ),
            (
                "animation-values",
                upload.gpu_indexed_animation_values.len(),
                upload.gpu_indexed_animation_values.capacity(),
            ),
        ];
        let mut gpu_capacities = [0_u64; 12];
        let mut seen = FxHashSet::default();
        for slot in &self.frame_resources {
            for ((total, bytes), buffer) in gpu_capacities
                .iter_mut()
                .zip(slot.buffers.memory_capacities())
                .zip(slot.buffers.ids())
            {
                if seen.insert(buffer) {
                    *total = total.saturating_add(bytes as u64);
                }
            }
        }
        streams
            .into_iter()
            .zip(gpu_capacities)
            .enumerate()
            .map(
                |(index, ((name, used, capacity), gpu_capacity_bytes))| BufferMemoryProfile {
                    name: name.to_string(),
                    cpu: MemoryUsage {
                        used_bytes: used as u64,
                        capacity_bytes: capacity as u64,
                    },
                    gpu_capacity_bytes,
                    gpu_slot_capacity_bytes: self
                        .frame_resources
                        .iter()
                        .map(|slot| slot.buffers.memory_capacities()[index] as u64)
                        .collect(),
                },
            )
            .collect()
    }

    fn renderer_memory_profile(&self) -> RendererMemoryProfile {
        let upload = &self.frame_upload;
        let cpu_retained_chunks = upload.retained_quad_memory();
        let (cpu_atlas_upload, atlas_live_keys, atlas_tile_bytes) = self.atlas.memory_profile();
        let mut profile = RendererMemoryProfile {
            frame_slots: self.frame_resources.len(),
            submitted_frames: self.submitted_frames,
            buffers: self.buffer_memory_profile(),
            cpu_retained_chunks,
            cpu_path_cache_bytes: upload.path_rasterization_cache.payload_bytes(),
            cpu_frame_capacity_bytes: upload.retained_byte_capacity() as u64,
            cpu_atlas_upload,
            atlas_live_keys,
            atlas_pages: self.gpu_atlas_textures.len(),
            atlas_tile_bytes,
            depth_bytes: u64::from(self.current_size.width)
                * u64::from(self.current_size.height)
                * 4,
            path_target_bytes: u64::from(self.path_texture_size.width())
                * u64::from(self.path_texture_size.height())
                * 4,
            blur_target_bytes: self
                .filters
                .targets
                .as_ref()
                .map_or(0, BackdropBlurTargets::byte_size),
            ..RendererMemoryProfile::default()
        };
        for (id, texture) in &self.gpu_atlas_textures {
            let bytes = texture.size.width.0.max(0) as u64
                * texture.size.height.0.max(0) as u64
                * atlas_bytes_per_pixel(id.kind) as u64;
            let category = match id.kind {
                AtlasTextureKind::Monochrome => &mut profile.atlas_monochrome_bytes,
                AtlasTextureKind::Subpixel => &mut profile.atlas_subpixel_bytes,
                AtlasTextureKind::Bgra | AtlasTextureKind::Rgba => &mut profile.atlas_color_bytes,
            };
            *category = category.saturating_add(bytes);
        }
        profile
    }
}
