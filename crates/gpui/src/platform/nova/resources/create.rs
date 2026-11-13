use anyhow::{Context, Result};

use super::super::*;
use super::bind_groups::create_renderer_bind_groups;
use super::buffers::create_resource_buffers;
use super::depth::create_depth_texture;
use super::pipelines::create_renderer_pipelines;
use super::shaders::create_renderer_shaders;
use super::{FrameResources, RendererCore, RendererResources};

/// Creates the renderer resources shared by every window using this device and color format.
///
/// These are the compiled artifacts: the resource and pipeline layouts, the render pass, the
/// compiled shader modules, and the render pipelines. None of them depend on a window's size, so
/// a second window reuses them instead of paying for another shader compile and driver pipeline
/// build.
pub(in crate::platform::nova) fn renderer_core<D>(
    device: &mut D,
    surface_config: SurfaceConfig,
    label: &str,
    shader_binaries: ShaderBinaries,
) -> Result<RendererCore>
where
    D: ResourceDevice + PipelineDevice,
{
    let started_at = std::time::Instant::now();
    let layouts = create_resource_layouts(device, label)?;
    let render_pass = device.create_render_pass(&RenderPassDescriptor {
        label: Some(format!("{label} render pass")),
        color_attachments: vec![ColorAttachmentDescriptor {
            format: surface_config.format,
        }],
        depth_attachment: Some(DepthAttachmentDescriptor {
            format: Format::Depth32Float,
            stencil_format: None,
        }),
            sample_count: 1,
            resolve_target: None,
        })?;
    let layouts_elapsed = started_at.elapsed();
    let shaders_started_at = std::time::Instant::now();
    let shaders = create_renderer_shaders(device, label, shader_binaries)?;
    let shaders_elapsed = shaders_started_at.elapsed();
    let pipelines_started_at = std::time::Instant::now();
    let pipelines = create_renderer_pipelines(
        device,
        label,
        surface_config,
        render_pass,
        &layouts,
        shaders,
    )?;

    log::info!(
        "GPUI renderer core: label={label} total_ms={} layouts_ms={} shaders_ms={} pipelines_ms={}",
        started_at.elapsed().as_millis(),
        layouts_elapsed.as_millis(),
        shaders_elapsed.as_millis(),
        pipelines_started_at.elapsed().as_millis(),
    );

    Ok(RendererCore {
        layouts,
        render_pass,
        pipelines,
    })
}

/// Creates the renderer resources sized to one window, reusing `core` for the shared parts.
pub(in crate::platform::nova) fn renderer_resources<D>(
    device: &mut D,
    surface_config: SurfaceConfig,
    label: &str,
    core: &RendererCore,
) -> Result<RendererResources>
where
    D: ResourceDevice + PipelineDevice,
{
    let started_at = std::time::Instant::now();
    let layouts = core.layouts;
    let buffers = create_resource_buffers(device, label)?;
    let buffers_elapsed = started_at.elapsed();
    let textures_started_at = std::time::Instant::now();
    let frame_buffers = buffers.frame_buffers;
    let shared_buffers = buffers.shared;
    let startup_path_mask_size = Extent2d::new(1, 1)?;
    let path_mask_target = create_path_mask_target(
        device,
        label,
        PathMaskTargetDescriptor {
            size: startup_path_mask_size,
            format: surface_config.format,
            bind_group_layout: layouts.path_bind_group_layout,
            frame_buffers: frame_buffers.clone(),
            sampler: shared_buffers.atlas_sampler,
        },
    )?;
    let atlas_resources = create_atlas_texture_resources(
        device,
        label,
        AtlasTextureId {
            index: 0,
            kind: AtlasTextureKind::Bgra,
        },
        Size {
            width: DevicePixels(i32::try_from(NOVA_STARTUP_ATLAS_SIZE).unwrap_or(i32::MAX)),
            height: DevicePixels(i32::try_from(NOVA_STARTUP_ATLAS_SIZE).unwrap_or(i32::MAX)),
        },
        &AtlasResourceDescriptor {
            mono_sprite_bind_group_layout: layouts.mono_bind_group_layout,
            poly_sprite_bind_group_layout: layouts.poly_bind_group_layout,
            frame_buffers: frame_buffers.clone(),
            sampler: shared_buffers.atlas_sampler,
        },
        NovaAtlasBindGroupMode::All,
    )?;
    let textures_elapsed = textures_started_at.elapsed();
    let bindings_started_at = std::time::Instant::now();
    let mut frame_resources = Vec::with_capacity(frame_buffers.len());
    for (index, frame_buffers) in frame_buffers.iter().copied().enumerate() {
        let bind_groups = create_renderer_bind_groups(
            device,
            &format!("{label} frame {index}"),
            &layouts,
            &frame_buffers,
        )?;
        frame_resources.push(FrameResources {
            buffers: frame_buffers,
            bind_groups,
            path_bind_group: path_mask_target
                .bind_groups
                .get(index)
                .copied()
                .context("missing path mask frame bind group")?,
            mono_sprite_bind_group: atlas_resources
                .mono_bind_groups
                .get(index)
                .copied()
                .context("missing atlas mono frame bind group")?,
            poly_sprite_bind_group: atlas_resources
                .poly_bind_groups
                .get(index)
                .copied()
                .context("missing atlas poly frame bind group")?,
        });
    }
    let bindings_elapsed = bindings_started_at.elapsed();
    let depth_started_at = std::time::Instant::now();
    let depth_texture = create_depth_texture(device, label, surface_config.size)?;
    let depth_texture_view = device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} depth texture view")),
        texture: depth_texture,
        base_mip_level: 0,
        mip_level_count: 1,
        format: Format::Depth32Float,
    })?;

    log::info!(
        "GPUI window resources: label={label} total_ms={} buffers_ms={} textures_ms={} bindings_ms={} depth_ms={}",
        started_at.elapsed().as_millis(),
        buffers_elapsed.as_millis(),
        textures_elapsed.as_millis(),
        bindings_elapsed.as_millis(),
        depth_started_at.elapsed().as_millis(),
    );

    Ok(RendererResources {
        render_pass: core.render_pass,
        pipelines: core.pipelines,
        depth_texture,
        depth_texture_view,
        frame_resources,
        quad_bind_group_layout: layouts.quad_bind_group_layout,
        shadow_bind_group_layout: layouts.shadow_bind_group_layout,
        underline_bind_group_layout: layouts.underline_bind_group_layout,
        path_rasterization_bind_group_layout: layouts.path_rasterization_bind_group_layout,
        path_bind_group_layout: layouts.path_bind_group_layout,
        mono_sprite_bind_group_layout: layouts.mono_bind_group_layout,
        poly_sprite_bind_group_layout: layouts.poly_bind_group_layout,
        backdrop_blur_pass_bind_group_layout: layouts.backdrop_blur_pass_bind_group_layout,
        backdrop_blur_bind_group_layout: layouts.backdrop_blur_bind_group_layout,
        backdrop_blur_targets: None,
        atlas_texture: atlas_resources.texture,
        atlas_texture_view: atlas_resources.texture_view,
        atlas_sampler: shared_buffers.atlas_sampler,
        path_texture: path_mask_target.texture,
        path_texture_view: path_mask_target.texture_view,
        path_texture_size: startup_path_mask_size,
    })
}
