//! True opaque-depth hierarchy for SSR. No screen approximation or CPU readback.
//!
//! Each mip is a separate R32F attachment. Vulkan tracks image state at whole
//! texture granularity, so sampling mip N while rendering mip N+1 of the SAME
//! image would create a layout conflict. These independent attachments keep
//! DX11/DX12/Vulkan/GL state transitions legal without an implicit GPU wait.
//! Four levels also keep the compositor within 16 portable fragment texture
//! units. Every level conservatively stores its footprint's nearest depth.
use super::{construction::Construction, targets::Target};
use gfx_core::*;
use gpui::{ExtensionDevice, RendererExtensionContext, RendererExtensionPass};
use std::sync::Arc;

pub(super) const SSR_HIZ_LEVELS: usize = 4;

pub(super) struct SsrHiZ {
    targets: Vec<Target>,
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
    Extent2d::new((base.width() >> mip).max(1), (base.height() >> mip).max(1))
        .map_err(Into::into)
}

impl SsrHiZ {
    pub(super) fn sampled_views(&self) -> [TextureViewId; SSR_HIZ_LEVELS] {
        let fallback = self.targets[0].view;
        std::array::from_fn(|i| self.targets.get(i).map_or(fallback, |t| t.view))
    }

    pub(super) fn new(
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
        extent: Extent2d,
        opaque_depth: TextureViewId,
        multisampled: bool,
    ) -> gpui::Result<Self> {
        let mut cleanup = Construction::default();
        let result = (|| {
            let base = Extent2d::new(
                extent.width().div_ceil(2),
                extent.height().div_ceil(2),
            )?;
            let mut targets = Vec::new();
            for mip in 0..SSR_HIZ_LEVELS as u32 {
                let size = mip_extent(base,mip)?;
                if mip > 0 && size == mip_extent(base,mip-1)? {
                    break;
                }
                let target = Target::create(device,size,Format::R32Float,false,1)?;
                target.record(&mut cleanup);
                targets.push(target);
            }
            let levels = targets.len() as u32;
            let render_pass = cleanup.record(device.create_render_pass(&RenderPassDescriptor {
                label: Some("gpui-3d SSR Hi-Z nearest depth".into()),
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
                label: Some("gpui-3d SSR pyramid vertex".into()),
                binary: shader(ShaderStage::Vertex,"vs_ssr_hiz")?,
            })?, |d, id| { let _ = d.destroy_shader_module(id); });
            let fragment = cleanup.record(device.create_shader_module(&ShaderModuleDescriptor {
                label: Some("gpui-3d SSR depth reduction".into()),
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
                let shader = if level == 0 { fragment_ms.unwrap_or(fragment) } else { fragment };
                pipelines.push(cleanup.record(device.create_render_pipeline(
                    &RenderPipelineDescriptor {
                        label: Some(format!("gpui-3d SSR Hi-Z level {level}")),
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
                    mip_extent(base,level)?,
                )?, |d, id| { let _ = d.destroy_render_pipeline(id); }));
                let source = if level == 0 {
                    opaque_depth
                } else {
                    targets[(level-1) as usize].view
                };
                groups.push(cleanup.record(device.create_bind_group(&BindGroupDescriptor {
                    label: Some(format!("gpui-3d SSR depth source {level}")),
                    layout,
                    entries: vec![BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::Texture(TextureBinding { texture_view: source }),
                    }],
                })?, |d, id| { let _ = d.destroy_bind_group(id); }));
            }
            Ok(Self {
                targets, levels, render_pass, layout, pipeline_layout,
                vertex, fragment, fragment_ms, pipelines, groups,
            })
        })();
        cleanup.finish(device,result)
    }

    pub(super) fn enqueue(&self, passes: &mut Vec<RendererExtensionPass>) {
        for (target, (&pipeline, &group)) in self.targets.iter()
            .zip(self.pipelines.iter().zip(self.groups.iter()))
        {
            passes.push(RendererExtensionPass {
                target: target.view,
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
        for target in self.targets { target.destroy(device)?; }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separate_mip_targets_round_down_without_zero_sized_levels() {
        let full=Extent2d::new(961,541).unwrap();
        assert_eq!(mip_extent(full,0).unwrap(),full);
        assert_eq!(mip_extent(full,1).unwrap(),Extent2d::new(480,270).unwrap());
        assert_eq!(mip_extent(full,3).unwrap(),Extent2d::new(120,67).unwrap());
    }
}
