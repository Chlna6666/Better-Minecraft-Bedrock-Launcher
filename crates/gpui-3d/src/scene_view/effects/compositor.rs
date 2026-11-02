use super::super::resources::embedded_shader;
use super::targets::Targets;
use gfx_core::*;
use gpui::{ExtensionDevice, RendererExtensionContext};

pub(super) struct Compositor {
    pub layout: BindGroupLayoutId,
    pipeline_layout: PipelineLayoutId,
    shaders: [ShaderModuleId; 3],
    pub sampler: SamplerId,
    cloud_sampler: SamplerId,
    pub post_pass: RenderPassId,
    pub post_pipeline: RenderPipelineId,
    post_extent: Extent2d,
    pipeline: Option<(RenderPassId, Extent2d, RenderPipelineId)>,
}
impl Compositor {
    pub(super) fn new(
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
    ) -> gpui::Result<Self> {
        let mut cleanup = super::construction::Construction::default();
        let result = (|| {
            let layout = cleanup.record(
                device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                    label: Some("gpui-3d screen inputs".into()),
                    entries: (0..14)
                        .map(|binding| BindGroupLayoutEntry {
                            binding,
                            binding_type: match binding {
                                5 | 11 => BindingType::Sampler,
                                6 => BindingType::UniformBuffer,
                                9 | 10 => BindingType::StorageBuffer,
                                _ => BindingType::SampledTexture,
                            },
                            stages: if binding == 6 {
                                ShaderStages::VERTEX | ShaderStages::FRAGMENT
                            } else {
                                ShaderStages::FRAGMENT
                            },
                        })
                        .collect(),
                })?,
                |d, id| {
                    let _ = d.destroy_bind_group_layout(id);
                },
            );
            let pipeline_layout = cleanup.record(
                device.create_pipeline_layout(&PipelineLayoutDescriptor {
                    label: None,
                    bind_group_layouts: vec![layout],
                })?,
                |d, id| {
                    let _ = d.destroy_pipeline_layout(id);
                },
            );
            let mut shaders = Vec::new();
            for (stage, entry) in [
                (ShaderStage::Vertex, "vs_effects"),
                (ShaderStage::Fragment, "fs_effects"),
                (ShaderStage::Fragment, "fs_composite"),
            ] {
                shaders.push(cleanup.record(
                    device.create_shader_module(&ShaderModuleDescriptor {
                        label: Some(entry.into()),
                        binary: embedded_shader(context, stage, entry)?,
                    })?,
                    |d, id| {
                        let _ = d.destroy_shader_module(id);
                    },
                ));
            }
            let shaders: [ShaderModuleId; 3] = shaders
                .try_into()
                .map_err(|_| anyhow::anyhow!("missing screen shaders"))?;
            let sampler = cleanup.record(
                device.create_sampler(&SamplerDescriptor::default())?,
                |d, id| {
                    let _ = d.destroy_sampler(id);
                },
            );
            let cloud_sampler = cleanup.record(
                device.create_sampler(&SamplerDescriptor {
                    address_mode_u: AddressMode::Repeat,
                    address_mode_v: AddressMode::Repeat,
                    ..SamplerDescriptor::default()
                })?,
                |d, id| {
                    let _ = d.destroy_sampler(id);
                },
            );
            let post_pass = cleanup.record(
                device.create_render_pass(&RenderPassDescriptor {
                    label: Some("gpui-3d screen resolve".into()),
                    color_attachment: ColorAttachmentDescriptor {
                        format: Format::Rgba8Unorm,
                    },
                    depth_attachment: None,
                })?,
                |d, id| {
                    let _ = d.destroy_render_pass(id);
                },
            );
            let post_pipeline = cleanup.record(
                create_pipeline(
                    device,
                    pipeline_layout,
                    &shaders,
                    context.offscreen(post_pass, Format::Rgba8Unorm, context.viewport()),
                    false,
                )?,
                |d, id| {
                    let _ = d.destroy_render_pipeline(id);
                },
            );
            Ok(Self {
                layout,
                pipeline_layout,
                shaders,
                sampler,
                cloud_sampler,
                post_pass,
                post_pipeline,
                pipeline: None,
                post_extent: context.viewport(),
            })
        })();
        cleanup.finish(device, result)
    }
    pub(super) fn sync_post(
        &mut self,
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
    ) -> gpui::Result<()> {
        if self.post_extent == context.viewport() {
            return Ok(());
        }
        let pipeline = create_pipeline(
            device,
            self.pipeline_layout,
            &self.shaders,
            context.offscreen(self.post_pass, Format::Rgba8Unorm, context.viewport()),
            false,
        )?;
        device.destroy_render_pipeline(std::mem::replace(&mut self.post_pipeline, pipeline))?;
        self.post_extent = context.viewport();
        Ok(())
    }
    pub(super) fn pipeline(
        &mut self,
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
    ) -> gpui::Result<RenderPipelineId> {
        if let Some((pass, extent, pipeline)) = self.pipeline
            && pass == context.render_pass()
            && extent == context.viewport()
        {
            return Ok(pipeline);
        }
        let pipeline = create_pipeline(
            device,
            self.pipeline_layout,
            &self.shaders,
            context.clone(),
            true,
        )?;
        if let Some((_, _, old)) =
            self.pipeline
                .replace((context.render_pass(), context.viewport(), pipeline))
        {
            device.destroy_render_pipeline(old)?;
        }
        Ok(pipeline)
    }
    pub(super) fn bind(
        &self,
        device: &mut dyn ExtensionDevice,
        inputs: Inputs<'_>,
    ) -> gpui::Result<BindGroupId> {
        let t = inputs.targets;
        let mut entries = Vec::new();
        for (binding, view) in [
            (0, inputs.color.unwrap_or(t.color.view)),
            (1, t.depth.view),
            (2, t.surface.view),
            (3, t.surface_depth.view),
            (4, t.albedo.view),
            (7, inputs.shadow),
            (8, inputs.cloud),
            (12, inputs.water),
            (13, inputs.water_flow),
        ] {
            entries.push(BindGroupEntry {
                binding,
                resource: BindingResource::Texture(TextureBinding { texture_view: view }),
            });
        }
        entries.push(BindGroupEntry {
            binding: 5,
            resource: BindingResource::Sampler(SamplerBinding {
                sampler: self.sampler,
            }),
        });
        entries.push(BindGroupEntry {
            binding: 11,
            resource: BindingResource::Sampler(SamplerBinding {
                sampler: self.cloud_sampler,
            }),
        });
        for (binding, buffer, size) in [
            (6, inputs.uniform, super::FRAME_BYTES),
            (9, inputs.lights.0, inputs.lights.1),
            (10, inputs.tiles.0, inputs.tiles.1),
        ] {
            entries.push(BindGroupEntry {
                binding,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer,
                    offset: 0,
                    size,
                    stride: None,
                }),
            });
        }
        Ok(device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: self.layout,
            entries,
        })?)
    }
    pub(super) fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        if let Some((_, _, pipeline)) = self.pipeline.take() {
            device.destroy_render_pipeline(pipeline)?;
        }
        device.destroy_render_pipeline(self.post_pipeline)?;
        device.destroy_render_pass(self.post_pass)?;
        for shader in self.shaders {
            device.destroy_shader_module(shader)?;
        }
        device.destroy_sampler(self.sampler)?;
        device.destroy_sampler(self.cloud_sampler)?;
        device.destroy_pipeline_layout(self.pipeline_layout)?;
        device.destroy_bind_group_layout(self.layout)?;
        Ok(())
    }
}
pub(super) struct Inputs<'a> {
    pub targets: &'a Targets,
    pub uniform: BufferId,
    pub lights: (BufferId, u64),
    pub tiles: (BufferId, u64),
    pub shadow: TextureViewId,
    pub cloud: TextureViewId,
    pub water: TextureViewId,
    pub water_flow: TextureViewId,
    pub color: Option<TextureViewId>,
}
fn create_pipeline(
    device: &mut dyn ExtensionDevice,
    layout: PipelineLayoutId,
    shaders: &[ShaderModuleId; 3],
    context: RendererExtensionContext,
    composite: bool,
) -> gpui::Result<RenderPipelineId> {
    Ok(device.create_render_pipeline(
        &RenderPipelineDescriptor {
            label: Some("gpui-3d screen pipeline".into()),
            vertex_shader: shaders[0],
            vertex_entry_point: "vs_effects".into(),
            fragment_shader: shaders[if composite { 2 } else { 1 }],
            fragment_entry_point: if composite {
                "fs_composite"
            } else {
                "fs_effects"
            }
            .into(),
            vertex_buffers: vec![],
            render_pass: context.render_pass(),
            pipeline_layout: Some(layout),
            color_format: context.color_format(),
            blend_mode: BlendMode::Replace,
            primitive_topology: PrimitiveTopology::TriangleList,
            raster: RasterState {
                front_face: FrontFace::Ccw,
                cull_mode: None,
            },
            depth_state: composite.then_some(DepthState {
                compare: CompareFunction::Always,
                write_enabled: true,
            }),
        },
        context.viewport(),
    )?)
}
