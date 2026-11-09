use super::super::resources::embedded_shader;
use super::targets::Targets;
use gfx_core::*;
use gpui::{ExtensionDevice, RendererExtensionContext};

pub(super) struct Compositor {
    pub layout: BindGroupLayoutId,
    pipeline_layout: PipelineLayoutId,
    /// Single-sampled shader set: `[vs_effects, fs_effects, fs_composite]`.
    shaders: [ShaderModuleId; 3],
    /// Multisampled shader set: `[vs_effects, fs_effects_ms, fs_composite_ms]`.
    ///
    /// Present only when the backend can build the multisampled depth variant, which
    /// is the single capability that decides whether the effect path may render 4x.
    ms_shaders: Option<[ShaderModuleId; 3]>,
    pub sampler: SamplerId,
    cloud_sampler: SamplerId,
    pub post_pass: RenderPassId,
    post_pipeline: RenderPipelineId,
    ms_post_pipeline: Option<RenderPipelineId>,
    post_extent: Extent2d,
    pipeline: Option<(RenderPassId, Extent2d, RenderPipelineId)>,
    ms_pipeline: Option<(RenderPassId, Extent2d, RenderPipelineId)>,
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
            // The multisampled variant is optional: a backend whose shader pipeline cannot
            // express multisampled sampled textures simply keeps the effect path single-sampled.
            let mut ms_modules = Vec::with_capacity(2);
            for entry in ["fs_effects_ms", "fs_composite_ms"] {
                let Ok(binary) = embedded_shader(&context, ShaderStage::Fragment, entry) else {
                    break;
                };
                match device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some(entry.into()),
                    binary,
                }) {
                    Ok(module) => ms_modules.push(cleanup.record(module, |d, id| {
                        let _ = d.destroy_shader_module(id);
                    })),
                    Err(_) => break,
                }
            }
            let ms_shaders = match ms_modules.as_slice() {
                [effects, composite] => Some([shaders[0], *effects, *composite]),
                _ => None,
            };
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
                    color_attachments: vec![ColorAttachmentDescriptor {
                        format: Format::Rgba8Unorm,
                    }],
                    depth_attachment: None,

                    sample_count: 1,
                    resolve_target: None,
                })?,
                |d, id| {
                    let _ = d.destroy_render_pass(id);
                },
            );
            let post_extent = context.viewport();
            let post_pipeline = cleanup.record(
                create_pipeline(
                    device,
                    pipeline_layout,
                    &shaders,
                    context.offscreen(post_pass, Format::Rgba8Unorm, post_extent),
                    false,
                    false,
                )?,
                |d, id| {
                    let _ = d.destroy_render_pipeline(id);
                },
            );
            // An embedded multisample shader can compile as a module yet fail
            // native pipeline validation on a particular driver. This feature
            // is optional: keep the normal screen compositor instead of
            // disabling all screen effects when only the MSAA variant fails.
            let ms_post_pipeline = if let Some(ms) = &ms_shaders {
                match create_pipeline(
                    device,
                    pipeline_layout,
                    ms,
                    context.offscreen(post_pass, Format::Rgba8Unorm, post_extent),
                    false,
                    true,
                ) {
                    Ok(pipeline) => Some(cleanup.record(pipeline, |d, id| {
                        let _ = d.destroy_render_pipeline(id);
                    })),
                    Err(error) => {
                        eprintln!("gpui-3d: disabling MSAA screen effects: {error}");
                        None
                    }
                }
            } else {
                None
            };
            Ok(Self {
                layout,
                pipeline_layout,
                shaders,
                ms_shaders,
                sampler,
                cloud_sampler,
                post_pass,
                post_pipeline,
                ms_post_pipeline,
                pipeline: None,
                ms_pipeline: None,
                post_extent,
            })
        })();
        cleanup.finish(device, result)
    }
    /// Whether the screen shaders can read multisampled geometry depth.
    pub(super) fn supports_multisampled_depth(&self) -> bool {
        self.ms_shaders.is_some() && self.ms_post_pipeline.is_some()
    }
    /// Resolve pipeline that writes the single-sampled display target.
    pub(super) fn post_pipeline(&self, multisampled: bool) -> RenderPipelineId {
        match (multisampled, self.ms_post_pipeline) {
            (true, Some(pipeline)) => pipeline,
            _ => self.post_pipeline,
        }
    }
    pub(super) fn sync_post(
        &mut self,
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
    ) -> gpui::Result<()> {
        if self.post_extent == context.viewport() {
            return Ok(());
        }
        let target = context.offscreen(self.post_pass, Format::Rgba8Unorm, context.viewport());
        let pipeline = create_pipeline(
            device,
            self.pipeline_layout,
            &self.shaders,
            target.clone(),
            false,
            false,
        )?;
        device.destroy_render_pipeline(std::mem::replace(&mut self.post_pipeline, pipeline))?;
        if let (Some(ms), Some(ms_pipeline)) = (&self.ms_shaders, self.ms_post_pipeline) {
            match create_pipeline(device, self.pipeline_layout, ms, target, false, true) {
                Ok(pipeline) => {
                    self.ms_post_pipeline = Some(pipeline);
                    device.destroy_render_pipeline(ms_pipeline)?;
                }
                Err(error) => {
                    // Keep the previous-size multisampled pipeline; the caller reports the
                    // failure and the next frame retries with the same extent.
                    self.ms_post_pipeline = Some(ms_pipeline);
                    return Err(error);
                }
            }
        }
        self.post_extent = context.viewport();
        Ok(())
    }
    pub(super) fn pipeline(
        &mut self,
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
        multisampled: bool,
    ) -> gpui::Result<RenderPipelineId> {
        let multisampled = multisampled && self.ms_shaders.is_some();
        let cached = if multisampled {
            self.ms_pipeline
        } else {
            self.pipeline
        };
        if let Some((pass, extent, pipeline)) = cached
            && pass == context.render_pass()
            && extent == context.viewport()
        {
            return Ok(pipeline);
        }
        let shaders = match (multisampled, &self.ms_shaders) {
            (true, Some(ms)) => ms,
            _ => &self.shaders,
        };
        let pipeline = create_pipeline(
            device,
            self.pipeline_layout,
            shaders,
            context.clone(),
            true,
            multisampled,
        )?;
        let slot = if multisampled {
            &mut self.ms_pipeline
        } else {
            &mut self.pipeline
        };
        if let Some((_, _, old)) =
            slot.replace((context.render_pass(), context.viewport(), pipeline))
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
            (1, inputs.depth),
            (2, t.surface.view),
            (3, inputs.surface_depth),
            (4, if inputs.multisampled {
                t.msaa.as_ref().map_or(t.albedo.view, |m| m.albedo.view)
            } else {
                t.albedo.view
            }),
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
        if let Some((_, _, pipeline)) = self.ms_pipeline.take() {
            device.destroy_render_pipeline(pipeline)?;
        }
        device.destroy_render_pipeline(self.post_pipeline)?;
        if let Some(pipeline) = self.ms_post_pipeline.take() {
            device.destroy_render_pipeline(pipeline)?;
        }
        device.destroy_render_pass(self.post_pass)?;
        for shader in self.shaders {
            device.destroy_shader_module(shader)?;
        }
        // The vertex shader is shared with the single-sampled set and destroyed above.
        if let Some(ms) = self.ms_shaders.take() {
            for shader in [ms[1], ms[2]] {
                device.destroy_shader_module(shader)?;
            }
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
    /// Opaque depth sampled by the screen shader (single- or multisampled).
    pub depth: TextureViewId,
    /// Surface-pass depth sampled by the screen shader (single- or multisampled).
    pub surface_depth: TextureViewId,
    /// Unresolved multisample albedo is read directly by the MSAA screen shader.
    pub multisampled: bool,
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
    multisampled: bool,
) -> gpui::Result<RenderPipelineId> {
    Ok(device.create_render_pipeline(
        &RenderPipelineDescriptor {
            label: Some("gpui-3d screen pipeline".into()),
            vertex_shader: shaders[0],
            vertex_entry_point: "vs_effects".into(),
            fragment_shader: shaders[if composite { 2 } else { 1 }],
            // The multisampled set mirrors this layout with its `_ms` entry points.
            fragment_entry_point: match (composite, multisampled) {
                (false, false) => "fs_effects",
                (true, false) => "fs_composite",
                (false, true) => "fs_effects_ms",
                (true, true) => "fs_composite_ms",
            }
            .into(),
            vertex_buffers: vec![],
            render_pass: context.render_pass(),
            pipeline_layout: Some(layout),
            color_targets: vec![gfx_core::ColorTargetState::new(
                context.color_format(),
                BlendMode::Replace,
            )],
            primitive_topology: PrimitiveTopology::TriangleList,
            raster: RasterState {
                front_face: FrontFace::Ccw,
                cull_mode: None,
            },
            depth_state: composite.then_some(DepthState {
                compare: CompareFunction::Always,
                write_enabled: true,

                stencil: None,
            }),
        },
        context.viewport(),
    )?)
}
