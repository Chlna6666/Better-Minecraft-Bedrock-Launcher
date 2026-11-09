//! Native depth hierarchy for SSR, built from real opaque Z after geometry.
//!
//! The DrawIndirect culling Hi-Z is a conservative, coarse approximation of
//! projected Section AABBs; it cannot answer SSR depth queries. This separate
//! R32F nearest-depth mip chain is produced entirely in GPU raster passes,
//! ordered after the opaque capture and before the effects compositor. No
//! host readback, forced queue drain, or extra CPU geometry is involved.
use super::construction::Construction;
use gfx_core::*;
use gpui::{ExtensionDevice, RendererExtensionContext, RendererExtensionPass};
use std::sync::Arc;

/// Full resolution would add more bandwidth without helping the coarse ray
/// traversal. The base level contains the nearest depth from each 2x2 block.
/// Its downsample is conservative: it can never miss a nearer occluder.
const MAX_LEVELS: u32 = 8;

pub(super) struct SsrHiZ {
    texture: TextureId,
    pub(super) sample_view: TextureViewId,
    mip_views: Vec<TextureViewId>,
    pub(super) levels: u32,
    render_pass: RenderPassId,
    layout: BindGroupLayoutId,
    pipeline_layout: PipelineLayoutId,
    vertex: ShaderModuleId,
    fragment: ShaderModuleId,
    fragment_ms: Option<ShaderModuleId>,
    pipelines: Vec<RenderPipelineId>,
    groups: Vec<BindGroupId>,
}

fn mip_extent(base: Extent2d, mip: u32) -> gpui::Result<Extent2d> {
    Extent2d::new(
        (base.width() >> mip).max(1),
        (base.height() >> mip).max(1),
    ).map_err(Into::into)
}

impl SsrHiZ {
    pub(super) fn new(
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
        extent: Extent2d,
        opaque_depth: TextureViewId,
        multisampled: bool,
    ) -> gpui::Result<Self> {
        let mut cleanup = Construction::default();
        let result = (|| {
            let size = Extent2d::new(
                extent.width().div_ceil(2),
                extent.height().div_ceil(2),
            )?;
            let mut levels = 1;
            let mut dimension = size.width().max(size.height());
            while dimension > 1 && levels < MAX_LEVELS {
                dimension = dimension.div_ceil(2);
                levels += 1;
            }
            let texture = cleanup.record(device.create_texture(&TextureDescriptor {
                label: Some("gpui-3d SSR nearest depth pyramid".into()),
                size,
                mip_level_count: levels,
                format: Format::R32Float,
                usage: TextureUsage::SAMPLED | TextureUsage::COLOR_ATTACHMENT,
                memory_location: MemoryLocation::GpuOnly,
                dimension: TextureDimension::D2,
                sample_count: 1,
            })?, |d, id| { let _ = d.destroy_texture(id); });
            let sample_view = cleanup.record(device.create_texture_view(&TextureViewDescriptor {
                label: Some("gpui-3d SSR full depth hierarchy".into()),
                texture,
                base_mip_level: 0,
                mip_level_count: levels,
                format: Format::R32Float,
            })?, |d, id| { let _ = d.destroy_texture_view(id); });
            let mut mip_views = Vec::with_capacity(levels as usize);
            for mip in 0..levels {
                mip_views.push(cleanup.record(device.create_texture_view(&TextureViewDescriptor {
                    label: Some(format!("gpui-3d SSR depth mip {mip}")),
                    texture,
                    base_mip_level: mip,
                    mip_level_count: 1,
                    format: Format::R32Float,
                })?, |d, id| { let _ = d.destroy_texture_view(id); }));
            }
            let render_pass = cleanup.record(device.create_render_pass(&RenderPassDescriptor {
                label: Some("gpui-3d SSR Hi-Z depth reduction".into()),
                color_attachments: vec![ColorAttachmentDescriptor { format: Format::R32Float }],
                depth_attachment: None,
                sample_count: 1,
                resolve_target: None,
            })?, |d, id| { let _ = d.destroy_render_pass(id); });
            let layout = cleanup.record(device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("gpui-3d SSR depth source".into()),
                entries: vec![BindGroupLayoutEntry {
                    binding: 0,
                    binding_type: BindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                }],
            })?, |d, id| { let _ = d.destroy_bind_group_layout(id); });
            let pipeline_layout = cleanup.record(device.create_pipeline_layout(&PipelineLayoutDescriptor {
                label: Some("gpui-3d SSR Hi-Z layout".into()),
                bind_group_layouts: vec![layout],
            })?, |d, id| { let _ = d.destroy_pipeline_layout(id); });
            let shader = |stage, entry| super::super::resources::embedded_shader(context,stage,entry);
            let vertex = cleanup.record(device.create_shader_module(&ShaderModuleDescriptor {
                label: Some("gpui-3d SSR reduction vertex".into()),
                binary: shader(ShaderStage::Vertex,"vs_ssr_hiz")?,
            })?, |d, id| { let _ = d.destroy_shader_module(id); });
            let fragment = cleanup.record(device.create_shader_module(&ShaderModuleDescriptor {
                label: Some("gpui-3d SSR single-sample depth reduction".into()),
                binary: shader(ShaderStage::Fragment,"fs_ssr_hiz")?,
            })?, |d, id| { let _ = d.destroy_shader_module(id); });
            let fragment_ms = if multisampled {
                Some(cleanup.record(device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("gpui-3d SSR MSAA depth reduction".into()),
                    binary: shader(ShaderStage::Fragment,"fs_ssr_hiz_ms")?,
                })?, |d, id| { let _ = d.destroy_shader_module(id); }))
            } else { None };
            let mut pipelines = Vec::with_capacity(levels as usize);
            let mut groups = Vec::with_capacity(levels as usize);
            for level in 0..levels {
                let target_extent = mip_extent(size, level)?;
                let shader = if level == 0 { fragment_ms.unwrap_or(fragment) } else { fragment };
                pipelines.push(cleanup.record(device.create_render_pipeline(
                    &RenderPipelineDescriptor {
                        label: Some(format!("gpui-3d SSR depth pyramid level {level}")),
                        vertex_shader: vertex,
                        vertex_entry_point: "vs_ssr_hiz".into(),
                        fragment_shader: shader,
                        fragment_entry_point: if level == 0 && multisampled {
                            "fs_ssr_hiz_ms"
                        } else {
                            "fs_ssr_hiz"
                        }.into(),
                        vertex_buffers: vec![],
                        render_pass,
                        pipeline_layout: Some(pipeline_layout),
                        color_targets: vec![ColorTargetState::new(Format::R32Float,BlendMode::Replace)],
                        primitive_topology: PrimitiveTopology::TriangleList,
                        raster: RasterState { front_face: FrontFace::Ccw, cull_mode: None },
                        depth_state: None,
                    },
                    target_extent,
                )?, |d, id| { let _ = d.destroy_render_pipeline(id); }));
                // Every mip is generated using the completed, previous mip.
                // This ensures correct hazards/order for DX11/12/Vulkan/GL.
                let source = if level == 0 {
                    opaque_depth
                } else {
                    mip_views[(level-1) as usize]
                };
                groups.push(cleanup.record(device.create_bind_group(&BindGroupDescriptor {
                    label: Some(format!("gpui-3d SSR source level {level}")),
                    layout,
                    entries: vec![BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::Texture(TextureBinding { texture_view: source }),
                    }],
                })?, |d, id| { let _ = d.destroy_bind_group(id); }));
            }
            Ok(Self {
                texture, sample_view, mip_views, levels, render_pass, layout,
                pipeline_layout, vertex, fragment, fragment_ms, pipelines, groups,
            })
        })();
        cleanup.finish(device,result)
    }

    pub(super) fn enqueue(&self, passes: &mut Vec<RendererExtensionPass>) {
        for (level, (&target, (&pipeline, &group))) in self.mip_views.iter()
            .zip(self.pipelines.iter().zip(self.groups.iter())).enumerate()
        {
            passes.push(RendererExtensionPass {
                target,
                color_attachments: Arc::from([]),
                gpu_work_tag: None,
                render_pass: self.render_pass,
                commands: vec![RenderCommand::Draw(DrawCommand {
                    pipeline,
                    bind_groups: bind_group_list([group]),
                    vertex_count: 3,
                    first_vertex: 0,
                    instance_count: 1,
                    first_instance: 0,
                    scissor: None,
                })].into(),
                color_load_op: LoadOp::Clear(ClearColor {
                    red: 1.0, green: 1.0, blue: 1.0, alpha: 1.0,
                }),
                depth_attachment: None,
                resolve_target: None,
            });
            debug_assert!(level < self.levels as usize);
        }
    }

    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        for group in self.groups { device.destroy_bind_group(group)?; }
        for pipeline in self.pipelines { device.destroy_render_pipeline(pipeline)?; }
        if let Some(ms) = self.fragment_ms { device.destroy_shader_module(ms)?; }
        device.destroy_shader_module(self.fragment)?;
        device.destroy_shader_module(self.vertex)?;
        device.destroy_pipeline_layout(self.pipeline_layout)?;
        device.destroy_bind_group_layout(self.layout)?;
        device.destroy_render_pass(self.render_pass)?;
        device.destroy_texture_view(self.sample_view)?;
        for view in self.mip_views { device.destroy_texture_view(view)?; }
        device.destroy_texture(self.texture)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_extent_keeps_non_power_of_two_borders() {
        let full = Extent2d::new(961, 541).unwrap();
        assert_eq!(mip_extent(full,0).unwrap(), full);
        assert_eq!(mip_extent(full,1).unwrap(), Extent2d::new(480,270).unwrap());
        assert_eq!(mip_extent(full,7).unwrap(), Extent2d::new(7,4).unwrap());
    }
}
