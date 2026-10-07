//! Destruction of window-owned IDs. Device/format-shared pipelines and layouts stay in the core.
use super::*;

struct WindowResources {
    frames: Vec<FrameResources>,
    atlas: FxHashMap<AtlasTextureId, NovaGpuAtlasTexture>,
    path: PathMaskTarget,
    blur: Option<BackdropBlurTargets>,
    depth: RenderTarget,
    sampler: SamplerId,
    swapchain: SwapchainId,
    surface: SurfaceId,
}

impl NovaRenderer {
    pub(super) fn destroy_window_resources(&mut self) {
        let resources = WindowResources {
            path: self.current_path_mask_target(),
            blur: self.filters.targets.take(),
            frames: std::mem::take(&mut self.frame_resources),
            atlas: std::mem::take(&mut self.gpu_atlas_textures),
            depth: RenderTarget {
                texture: self.depth_texture,
                texture_view: self.depth_texture_view,
            },
            sampler: self.atlas_sampler,
            swapchain: self.swapchain,
            surface: self.surface,
        };
        match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => destroy_resources(device, resources, "DX12"),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => destroy_resources(device, resources, "Metal"),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => destroy_resources(device, resources, "Vulkan"),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {}
        }
    }
}

fn destroy_resources<D: BackendResources + BackendSurface + BackendPipelines>(
    device: &mut D,
    resources: WindowResources,
    backend: &str,
) {
    destroy_path_mask_target(device, resources.path, backend);
    if let Some(blur) = resources.blur {
        destroy_backdrop_blur_target_chain(device, blur, backend);
    }
    for (atlas_id, texture) in resources.atlas {
        destroy_gpu_atlas_texture(device, texture, backend, atlas_id);
    }
    for frame in resources.frames {
        destroy_frame(device, frame);
    }
    release(
        device.destroy_texture_view(resources.depth.texture_view),
        "depth view",
    );
    release(
        device.destroy_texture(resources.depth.texture),
        "depth texture",
    );
    release(device.destroy_sampler(resources.sampler), "atlas sampler");
    release(
        BackendSurface::destroy_swapchain(device, resources.swapchain),
        "swapchain",
    );
    release(
        BackendSurface::destroy_surface(device, resources.surface),
        "surface",
    );
}

fn destroy_frame<D: BackendResources>(device: &mut D, frame: FrameResources) {
    let sets = frame.resource_sets;
    for resource_set in [
        sets.quad_resource_set,
        sets.shadow_resource_set,
        sets.path_rasterization_resource_set,
        sets.underline_resource_set,
    ] {
        release(
            device.destroy_resource_set(resource_set),
            "frame resource set",
        );
    }
    // Path and sprite sets are owned by the path/atlas targets, not duplicated here.
    let buffers = frame.buffers;
    for buffer in [
        buffers.global_buffer,
        buffers.text_raster_buffer,
        buffers.quad_buffer,
        buffers.shadow_buffer,
        buffers.path_rasterization_vertex_buffer,
        buffers.path_sprite_buffer,
        buffers.mono_sprite_buffer,
        buffers.poly_sprite_buffer,
        buffers.underline_buffer,
        buffers.backdrop_blur_pass_buffer,
        buffers.backdrop_blur_buffer,
        buffers.animation_value_buffer,
    ] {
        release(device.destroy_buffer(buffer), "frame buffer");
    }
}

fn release(result: gfx_core::Result<()>, resource: &str) {
    if let Err(error) = result {
        log::debug!("failed to destroy Nova window {resource}: {error}");
    }
}
