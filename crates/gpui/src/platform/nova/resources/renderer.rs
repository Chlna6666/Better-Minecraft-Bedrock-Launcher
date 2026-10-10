use super::super::*;
use super::FrameResources;

/// Renderer resources that depend only on the device and the surface color format.
///
/// Resource and pipeline layouts, the render pass, the compiled shader modules, and the render
/// pipelines are all sized to a device rather than to a window, so every window rendering
/// through the same device with the same color format shares one core. Only
/// [`RendererResources`] carries the parts that belong to a single window.
#[derive(Clone, Copy)]
pub(in crate::platform::nova) struct RendererCore {
    pub(in crate::platform::nova) layouts: ResourceLayouts,
    pub(in crate::platform::nova) render_pass: RenderPassId,
    pub(in crate::platform::nova) pipelines: Pipelines,
}

pub(in crate::platform::nova) struct RendererResources {
    pub(in crate::platform::nova) render_pass: RenderPassId,
    pub(in crate::platform::nova) pipelines: Pipelines,
    pub(in crate::platform::nova) depth_texture: TextureId,
    pub(in crate::platform::nova) depth_texture_view: TextureViewId,
    pub(in crate::platform::nova) frame_resources: Vec<FrameResources>,
    pub(in crate::platform::nova) quad_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) shadow_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) underline_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) path_rasterization_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) path_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) mono_sprite_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) poly_sprite_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) backdrop_blur_pass_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) backdrop_blur_resource_set_layout: ResourceSetLayoutId,
    pub(in crate::platform::nova) backdrop_blur_targets: Option<BackdropBlurTargets>,
    pub(in crate::platform::nova) atlas_texture: TextureId,
    pub(in crate::platform::nova) atlas_texture_view: TextureViewId,
    pub(in crate::platform::nova) atlas_sampler: SamplerId,
    pub(in crate::platform::nova) path_texture: TextureId,
    pub(in crate::platform::nova) path_texture_view: TextureViewId,
    pub(in crate::platform::nova) path_texture_size: Extent2d,
}
