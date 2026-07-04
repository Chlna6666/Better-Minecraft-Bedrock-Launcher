use anyhow::Result;

use super::super::*;
use super::buffers::create_resource_buffers;
use super::depth::create_depth_texture;
use super::pipelines::create_renderer_pipelines;
use super::resource_sets::create_renderer_resource_sets;
use super::shaders::create_renderer_shaders;
use super::types::NovaRendererResources;

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
    let path_mask_target = create_path_mask_target(
        device,
        label,
        NovaPathMaskTargetDescriptor {
            size: surface_config.size,
            format: surface_config.format,
            resource_set_layout: layouts.path_resource_set_layout,
            global_buffer: buffers.global_buffer,
            path_sprite_buffer: buffers.path_sprite_buffer,
            sampler: buffers.atlas_sampler,
        },
    )?;
    let present_cache_target = create_present_cache_target(
        device,
        label,
        NovaPresentCacheTargetDescriptor {
            size: surface_config.size,
            format: surface_config.format,
            resource_set_layout: layouts.poly_resource_set_layout,
            global_buffer: buffers.global_buffer,
            sprite_buffer: buffers.present_copy_sprite_buffer,
            sampler: buffers.atlas_sampler,
        },
    )?;
    let resource_sets = create_renderer_resource_sets(device, label, &layouts, &buffers)?;
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
        NovaAtlasResourceDescriptor {
            mono_sprite_resource_set_layout: layouts.mono_resource_set_layout,
            poly_sprite_resource_set_layout: layouts.poly_resource_set_layout,
            global_buffer: buffers.global_buffer,
            text_raster_buffer: buffers.text_raster_buffer,
            mono_sprite_buffer: buffers.mono_sprite_buffer,
            poly_sprite_buffer: buffers.poly_sprite_buffer,
            sampler: buffers.atlas_sampler,
        },
    )?;
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
        global_buffer: buffers.global_buffer,
        text_raster_buffer: buffers.text_raster_buffer,
        quad_buffer: buffers.quad_buffer,
        shadow_buffer: buffers.shadow_buffer,
        path_rasterization_vertex_buffer: buffers.path_rasterization_vertex_buffer,
        path_sprite_buffer: buffers.path_sprite_buffer,
        mono_sprite_buffer: buffers.mono_sprite_buffer,
        poly_sprite_buffer: buffers.poly_sprite_buffer,
        present_copy_sprite_buffer: buffers.present_copy_sprite_buffer,
        underline_buffer: buffers.underline_buffer,
        backdrop_blur_pass_buffer: buffers.backdrop_blur_pass_buffer,
        backdrop_blur_buffer: buffers.backdrop_blur_buffer,
        animation_binding_buffer: buffers.animation_binding_buffer,
        animation_value_buffer: buffers.animation_value_buffer,
        custom_mesh_3d_parameters_buffer: buffers.custom_mesh_3d_parameters_buffer,
        custom_mesh_3d_vertices_buffer: buffers.custom_mesh_3d_vertices_buffer,
        custom_mesh_3d_indices_buffer: buffers.custom_mesh_3d_indices_buffer,
        quad_resource_set: resource_sets.quad_resource_set,
        shadow_resource_set: resource_sets.shadow_resource_set,
        path_rasterization_resource_set: resource_sets.path_rasterization_resource_set,
        path_resource_set_layout: layouts.path_resource_set_layout,
        path_resource_set: path_mask_target.resource_set,
        mono_sprite_resource_set_layout: layouts.mono_resource_set_layout,
        mono_sprite_resource_set: atlas_resources.mono_resource_set,
        poly_sprite_resource_set_layout: layouts.poly_resource_set_layout,
        poly_sprite_resource_set: atlas_resources.poly_resource_set,
        present_cache_resource_set: present_cache_target.resource_set,
        underline_resource_set: resource_sets.underline_resource_set,
        backdrop_blur_pass_resource_set_layout: layouts.backdrop_blur_pass_resource_set_layout,
        backdrop_blur_resource_set_layout: layouts.backdrop_blur_resource_set_layout,
        custom_mesh_3d_pipeline_layout: layouts.custom_mesh_3d_pipeline_layout,
        custom_mesh_3d_resource_set: resource_sets.custom_mesh_3d_resource_set,
        backdrop_blur_targets: None,
        atlas_texture: atlas_resources.texture,
        atlas_texture_view: atlas_resources.texture_view,
        atlas_sampler: buffers.atlas_sampler,
        path_texture: path_mask_target.texture,
        path_texture_view: path_mask_target.texture_view,
        present_cache_texture: present_cache_target.texture,
        present_cache_texture_view: present_cache_target.texture_view,
    })
}
