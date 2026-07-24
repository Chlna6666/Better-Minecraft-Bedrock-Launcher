use anyhow::{Context, Result};

use super::super::*;
use super::buffers::create_resource_buffers;
use super::depth::create_depth_texture;
use super::pipelines::create_renderer_pipelines;
use super::resource_sets::create_renderer_resource_sets;
use super::shaders::create_renderer_shaders;
use super::types::{NovaFrameResources, NovaRendererResources};

pub(in crate::platform::nova) fn create_renderer_resources<D>(
    device: &mut D,
    surface_config: SurfaceConfig,
    label: &str,
    shader_binaries: NovaShaderBinaries,
) -> Result<NovaRendererResources>
where
    D: BackendResources + BackendPipelines,
{
    let layouts = create_resource_layouts(device, label)?;
    let buffers = create_resource_buffers(device, label)?;
    let frame_buffers = buffers.frame_buffers;
    let shared_buffers = buffers.shared;
    let path_mask_target = create_path_mask_target(
        device,
        label,
        NovaPathMaskTargetDescriptor {
            size: surface_config.size,
            format: surface_config.format,
            resource_set_layout: layouts.path_resource_set_layout,
            frame_buffers: frame_buffers.clone(),
            sampler: shared_buffers.atlas_sampler,
        },
    )?;
    let present_cache_target = create_present_cache_target(
        device,
        label,
        NovaPresentCacheTargetDescriptor {
            size: surface_config.size,
            format: surface_config.format,
            resource_set_layout: layouts.poly_resource_set_layout,
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
            width: DevicePixels(i32::try_from(NOVA_DEFAULT_ATLAS_SIZE).unwrap_or(i32::MAX)),
            height: DevicePixels(i32::try_from(NOVA_DEFAULT_ATLAS_SIZE).unwrap_or(i32::MAX)),
        },
        &NovaAtlasResourceDescriptor {
            mono_sprite_resource_set_layout: layouts.mono_resource_set_layout,
            poly_sprite_resource_set_layout: layouts.poly_resource_set_layout,
            frame_buffers: frame_buffers.clone(),
            sampler: shared_buffers.atlas_sampler,
        },
        NovaAtlasResourceSetMode::All,
    )?;
    let mut frame_resources = Vec::with_capacity(frame_buffers.len());
    for (index, frame_buffers) in frame_buffers.iter().copied().enumerate() {
        let resource_sets = create_renderer_resource_sets(
            device,
            &format!("{label} frame {index}"),
            &layouts,
            &frame_buffers,
            shared_buffers.custom_mesh_3d_vertices_buffer,
        )?;
        frame_resources.push(NovaFrameResources {
            buffers: frame_buffers,
            resource_sets,
            path_resource_set: path_mask_target
                .resource_sets
                .get(index)
                .copied()
                .context("missing path mask frame resource set")?,
            present_cache_resource_set: present_cache_target
                .resource_sets
                .get(index)
                .copied()
                .context("missing present cache frame resource set")?,
            mono_sprite_resource_set: atlas_resources
                .mono_resource_sets
                .get(index)
                .copied()
                .context("missing atlas mono frame resource set")?,
            poly_sprite_resource_set: atlas_resources
                .poly_resource_sets
                .get(index)
                .copied()
                .context("missing atlas poly frame resource set")?,
        });
    }
    let depth_texture = create_depth_texture(device, label, surface_config.size)?;
    let depth_texture_view = device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} depth texture view")),
        texture: depth_texture,
        format: Format::Depth32Float,
    })?;
    let render_pass = device.create_render_pass(&RenderPassCompatibilityDescriptor {
        label: Some(format!("{label} render pass")),
        color_attachment: ColorAttachmentDescriptor {
            format: surface_config.format,
        },
        depth_attachment: Some(DepthAttachmentDescriptor {
            format: Format::Depth32Float,
        }),
    })?;
    let shaders = create_renderer_shaders(device, label, shader_binaries)?;
    let pipelines = create_renderer_pipelines(
        device,
        label,
        surface_config,
        render_pass,
        &layouts,
        shaders,
    )?;

    Ok(NovaRendererResources {
        render_pass,
        pipelines,
        depth_texture,
        depth_texture_view,
        frame_resources,
        custom_mesh_3d_vertices_buffer: shared_buffers.custom_mesh_3d_vertices_buffer,
        custom_mesh_3d_indices_buffer: shared_buffers.custom_mesh_3d_indices_buffer,
        path_resource_set_layout: layouts.path_resource_set_layout,
        mono_sprite_resource_set_layout: layouts.mono_resource_set_layout,
        poly_sprite_resource_set_layout: layouts.poly_resource_set_layout,
        backdrop_blur_pass_resource_set_layout: layouts.backdrop_blur_pass_resource_set_layout,
        backdrop_blur_resource_set_layout: layouts.backdrop_blur_resource_set_layout,
        custom_mesh_3d_pipeline_layout: layouts.custom_mesh_3d_pipeline_layout,
        backdrop_blur_targets: None,
        atlas_texture: atlas_resources.texture,
        atlas_texture_view: atlas_resources.texture_view,
        atlas_sampler: shared_buffers.atlas_sampler,
        path_texture: path_mask_target.texture,
        path_texture_view: path_mask_target.texture_view,
        present_cache_texture: present_cache_target.texture,
        present_cache_texture_view: present_cache_target.texture_view,
    })
}
