//! Device-owned, separately submitted 3D render target and UI texture bridge.
//!
//! Like Qt3D's RenderView -> FrameGraph sink, geometry commands render to an
//! independent color/depth attachment. GPUI then composites the *sampled*
//! color texture at the SceneView's painter-order position. `nova-gfx` submits
//! these producer passes before their GUI consumer and tracks the returned
//! SubmissionId/fence. No native graphics context is ever sent to a worker.
use super::{SceneViewId, resources::embedded_shader};
use gfx_core::*;
use gpui::{ExtensionDevice, RendererExtensionContext, RendererExtensionPass};
use std::collections::HashMap;

/// Each SceneView gets a separate target: sharing one color image across
/// multiple painter-order elements would make every composite sample the last
/// view's content, because all producer passes submit before the GUI pass.
struct Target {
    color: TextureId,
    color_view: TextureViewId,
    depth: TextureId,
    depth_view: TextureViewId,
    binding: BindGroupId,
    extent: Extent2d,
}

impl Target {
    fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        device.destroy_bind_group(self.binding)?;
        device.destroy_texture_view(self.depth_view)?;
        device.destroy_texture(self.depth)?;
        device.destroy_texture_view(self.color_view)?;
        device.destroy_texture(self.color)?;
        Ok(())
    }
}

/// Keep the source 3D pass inside the same widget region as the final GUI
/// composite. The offscreen target is full-window so projection coordinates
/// remain identical; clipping here avoids rasterizing hidden world pixels.
fn clipped_scissor(previous: Option<ScissorRect>, clip: ScissorRect) -> ScissorRect {
    let Some(previous) = previous else { return clip };
    let x = previous.x.max(clip.x);
    let y = previous.y.max(clip.y);
    let right = previous.x.saturating_add(previous.width)
        .min(clip.x.saturating_add(clip.width));
    let bottom = previous.y.saturating_add(previous.height)
        .min(clip.y.saturating_add(clip.height));
    ScissorRect {
        x, y,
        width: right.saturating_sub(x),
        height: bottom.saturating_sub(y),
    }
}

/// GPU resources stay on the graphics owner. Only the CPU render preparation
/// queue runs on other threads; this surface owns no swapchain or window.
pub(super) struct SidecarSurface {
    capture_pass: RenderPassId,
    capture_format: Format,
    host_pass: RenderPassId,
    host_format: Format,
    host_extent: Extent2d,
    vertex_shader: ShaderModuleId,
    fragment_shader: ShaderModuleId,
    sampler: SamplerId,
    bind_layout: BindGroupLayoutId,
    pipeline_layout: PipelineLayoutId,
    composite_pipeline: RenderPipelineId,
    targets: HashMap<SceneViewId, Target>,
}

enum Allocation {
    Pass(RenderPassId),
    Shader(ShaderModuleId),
    Sampler(SamplerId),
    BindLayout(BindGroupLayoutId),
    PipelineLayout(PipelineLayoutId),
    Pipeline(RenderPipelineId),
}

fn rollback(device: &mut dyn ExtensionDevice, allocations: Vec<Allocation>) {
    for resource in allocations.into_iter().rev() {
        let _ = match resource {
            Allocation::Pass(id) => device.destroy_render_pass(id),
            Allocation::Shader(id) => device.destroy_shader_module(id),
            Allocation::Sampler(id) => device.destroy_sampler(id),
            Allocation::BindLayout(id) => device.destroy_bind_group_layout(id),
            Allocation::PipelineLayout(id) => device.destroy_pipeline_layout(id),
            Allocation::Pipeline(id) => device.destroy_render_pipeline(id),
        };
    }
}

fn composite_pipeline(
    device: &mut dyn ExtensionDevice,
    render_pass: RenderPassId,
    format: Format,
    extent: Extent2d,
    layout: PipelineLayoutId,
    vertex: ShaderModuleId,
    fragment: ShaderModuleId,
) -> gpui::Result<RenderPipelineId> {
    Ok(device.create_render_pipeline(
        &RenderPipelineDescriptor {
            label: Some("gpui-3d sidecar UI texture composite".into()),
            vertex_shader: vertex,
            vertex_entry_point: "vs_sidecar_composite".into(),
            fragment_shader: fragment,
            fragment_entry_point: "fs_sidecar_composite".into(),
            vertex_buffers: vec![],
            render_pass,
            pipeline_layout: Some(layout),
            color_targets: vec![ColorTargetState::new(format, BlendMode::PremultipliedAlpha)],
            primitive_topology: PrimitiveTopology::TriangleList,
            raster: RasterState {
                front_face: FrontFace::Ccw,
                cull_mode: None,
            },
            // The 3D depth buffer belongs to the offscreen producer, not
            // GPUI's 2D GUI surface. Its composite never writes GUI depth.
            depth_state: None,
        },
        extent,
    )?)
}

impl SidecarSurface {
    pub(super) fn new(
        device: &mut dyn ExtensionDevice,
        host: &RendererExtensionContext,
    ) -> gpui::Result<Self> {
        let mut allocations = Vec::<Allocation>::new();
        let result: gpui::Result<Self> = (|| -> gpui::Result<Self> {
            let capture_format = host.color_format();
            let capture_pass = device.create_render_pass(&RenderPassDescriptor {
                label: Some("gpui-3d independently submitted scene pass".into()),
                color_attachments: vec![ColorAttachmentDescriptor {
                    format: capture_format,
                }],
                depth_attachment: Some(DepthAttachmentDescriptor {
                    format: Format::Depth32Float,
                    stencil_format: None,
                }),
                sample_count: 1,
                resolve_target: None,
            })?;
            allocations.push(Allocation::Pass(capture_pass));
            let vertex_shader = device.create_shader_module(&ShaderModuleDescriptor {
                label: Some("gpui-3d sidecar composite vertex".into()),
                binary: embedded_shader(host, ShaderStage::Vertex, "vs_sidecar_composite")?,
            })?;
            allocations.push(Allocation::Shader(vertex_shader));
            let fragment_shader = device.create_shader_module(&ShaderModuleDescriptor {
                label: Some("gpui-3d sidecar composite fragment".into()),
                binary: embedded_shader(host, ShaderStage::Fragment, "fs_sidecar_composite")?,
            })?;
            allocations.push(Allocation::Shader(fragment_shader));
            let sampler = device.create_sampler(&SamplerDescriptor::default())?;
            allocations.push(Allocation::Sampler(sampler));
            let bind_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("gpui-3d sidecar sampled output".into()),
                entries: vec![
                    BindGroupLayoutEntry {
                        binding: 0,
                        binding_type: BindingType::SampledTexture,
                        stages: ShaderStages::FRAGMENT,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        binding_type: BindingType::Sampler,
                        stages: ShaderStages::FRAGMENT,
                    },
                ],
            })?;
            allocations.push(Allocation::BindLayout(bind_layout));
            let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
                label: Some("gpui-3d sidecar composite layout".into()),
                bind_group_layouts: vec![bind_layout],
            })?;
            allocations.push(Allocation::PipelineLayout(pipeline_layout));
            let composite_pipeline = composite_pipeline(
                device, host.render_pass(), host.color_format(), host.viewport(),
                pipeline_layout, vertex_shader, fragment_shader,
            )?;
            allocations.push(Allocation::Pipeline(composite_pipeline));
            Ok(Self {
                capture_pass,
                capture_format,
                host_pass: host.render_pass(),
                host_format: host.color_format(),
                host_extent: host.viewport(),
                vertex_shader,
                fragment_shader,
                sampler,
                bind_layout,
                pipeline_layout,
                composite_pipeline,
                targets: HashMap::new(),
            })
        })();
        if result.is_err() {
            rollback(device, allocations);
        }
        result
    }

    pub(super) fn capture_context(
        &self,
        host: &RendererExtensionContext,
    ) -> RendererExtensionContext {
        host.offscreen_preserving_bounds(self.capture_pass, self.capture_format)
    }

    fn create_target(
        &self,
        device: &mut dyn ExtensionDevice,
        extent: Extent2d,
    ) -> gpui::Result<Target> {
        let color = device.create_texture(&TextureDescriptor {
            label: Some("gpui-3d sidecar color".into()),
            size: extent,
            mip_level_count: 1,
            format: self.capture_format,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
            sample_count: 1,
        })?;
        let color_view = match device.create_texture_view(&TextureViewDescriptor {
            label: None, texture: color, base_mip_level: 0,
            mip_level_count: 1, format: self.capture_format,
        }) {
            Ok(view) => view,
            Err(error) => {
                let _ = device.destroy_texture(color);
                return Err(error.into());
            }
        };
        let depth = match device.create_texture(&TextureDescriptor {
            label: Some("gpui-3d sidecar depth".into()),
            size: extent, mip_level_count: 1,
            format: Format::Depth32Float,
            usage: TextureUsage::DEPTH_ATTACHMENT,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
            sample_count: 1,
        }) {
            Ok(texture) => texture,
            Err(error) => {
                let _ = device.destroy_texture_view(color_view);
                let _ = device.destroy_texture(color);
                return Err(error.into());
            }
        };
        let depth_view = match device.create_texture_view(&TextureViewDescriptor {
            label: None, texture: depth,
            base_mip_level: 0, mip_level_count: 1,
            format: Format::Depth32Float,
        }) {
            Ok(view) => view,
            Err(error) => {
                let _ = device.destroy_texture(depth);
                let _ = device.destroy_texture_view(color_view);
                let _ = device.destroy_texture(color);
                return Err(error.into());
            }
        };
        let binding = match device.create_bind_group(&BindGroupDescriptor {
            label: Some("gpui-3d sidecar sampled target".into()),
            layout: self.bind_layout,
            entries: vec![
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Texture(TextureBinding {
                        texture_view: color_view,
                    }),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(SamplerBinding {
                        sampler: self.sampler,
                    }),
                },
            ],
        }) {
            Ok(binding) => binding,
            Err(error) => {
                let _ = device.destroy_texture_view(depth_view);
                let _ = device.destroy_texture(depth);
                let _ = device.destroy_texture_view(color_view);
                let _ = device.destroy_texture(color);
                return Err(error.into());
            }
        };
        Ok(Target { color, color_view, depth, depth_view, binding, extent })
    }

    pub(super) fn sync(
        &mut self,
        device: &mut dyn ExtensionDevice,
        host: &RendererExtensionContext,
    ) -> gpui::Result<()> {
        if self.host_pass == host.render_pass()
            && self.host_format == host.color_format()
            && self.host_extent == host.viewport()
        {
            return Ok(());
        }
        let next = composite_pipeline(
            device, host.render_pass(), host.color_format(), host.viewport(),
            self.pipeline_layout, self.vertex_shader, self.fragment_shader,
        )?;
        let old = std::mem::replace(&mut self.composite_pipeline, next);
        self.host_pass = host.render_pass();
        self.host_format = host.color_format();
        self.host_extent = host.viewport();
        device.destroy_render_pipeline(old)?;
        Ok(())
    }

    pub(super) fn enqueue(
        &mut self,
        device: &mut dyn ExtensionDevice,
        id: SceneViewId,
        host: &RendererExtensionContext,
        mut geometry: Vec<RenderCommand>,
        passes: &mut Vec<RendererExtensionPass>,
        gui_steps: &mut Vec<RenderCommand>,
    ) -> gpui::Result<()> {
        let clip = host.scissor();
        for draw in &mut geometry {
            draw.set_scissor(Some(clipped_scissor(draw.scissor(), clip)));
        }
        geometry.retain(|draw| draw.scissor().is_none_or(|rect| !rect.is_empty()));
        let recreate = self.targets.get(&id)
            .is_none_or(|target| target.extent != host.viewport());
        if recreate {
            let new_target = self.create_target(device, host.viewport())?;
            if let Some(previous) = self.targets.insert(id, new_target) {
                previous.destroy(device)?;
            }
        }
        let target = self.targets.get(&id)
            .ok_or_else(|| anyhow::anyhow!("missing 3D offscreen target"))?;
        passes.push(RendererExtensionPass {
            target: target.color_view,
            resolve_target: None,
            render_pass: self.capture_pass,
            commands: geometry,
            // Unlike the ordinary swapchain clear (alpha=1), this is a
            // composited *producer* texture. Pixels outside the 3D widget
            // must stay transparent or they would obscure the GPUI scene.
            color_load_op: LoadOp::Clear(ClearColor {
                red: 0.0,
                green: 0.0,
                blue: 0.0,
                alpha: 0.0,
            }),
            depth_attachment: Some(DepthAttachment {
                target: target.depth_view,
                depth_load_op: LoadOp::Clear(1.0),
                stencil_load_op: None,
            }),
        });
        gui_steps.push(RenderCommand::Draw(DrawCommand {
            pipeline: self.composite_pipeline,
            bind_groups: bind_group_list([target.binding]),
            vertex_count: 3,
            first_vertex: 0,
            instance_count: 1,
            first_instance: 0,
            scissor: None, // the GPUI extension applies the element scissor
        }));
        Ok(())
    }

    pub(super) fn retire(
        &mut self,
        device: &mut dyn ExtensionDevice,
        id: SceneViewId,
    ) -> gpui::Result<()> {
        if let Some(target) = self.targets.remove(&id) {
            target.destroy(device)?;
        }
        Ok(())
    }

    pub(super) fn destroy(mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        for (_, target) in self.targets.drain() {
            target.destroy(device)?;
        }
        device.destroy_render_pipeline(self.composite_pipeline)?;
        device.destroy_pipeline_layout(self.pipeline_layout)?;
        device.destroy_bind_group_layout(self.bind_layout)?;
        device.destroy_sampler(self.sampler)?;
        device.destroy_shader_module(self.fragment_shader)?;
        device.destroy_shader_module(self.vertex_shader)?;
        device.destroy_render_pass(self.capture_pass)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offscreen_geometry_obeys_original_gui_clip() {
        let element = ScissorRect { x: 40, y: 20, width: 200, height: 150 };
        let source = ScissorRect { x: 100, y: 0, width: 150, height: 100 };
        assert_eq!(clipped_scissor(Some(source), element),
            ScissorRect { x: 100, y: 20, width: 140, height: 80 });
        assert_eq!(clipped_scissor(None, element), element);
        let outside = ScissorRect { x: 400, y: 300, width: 40, height: 40 };
        assert!(clipped_scissor(Some(outside), element).is_empty());
    }
}
