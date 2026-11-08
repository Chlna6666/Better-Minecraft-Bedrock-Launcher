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
use std::{collections::HashMap, sync::Arc, time::{Duration, Instant}};

/// Only the rounded portion of GPUI's ContentMask affects the shader.
/// The rectangular portion is enforced by the host's native ScissorRect.
/// For a purely rectangular clip, all masks produce the SAME zero key:
/// scrolling or changing an ancestor mask must not allocate a new
/// uniform/bind group for unchanged 3D viewport placement.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct RoundedMask {
    bounds: [f32; 4],
    radii: [f32; 4],
}

fn rounded_mask(mask: &gpui::ContentMask<gpui::ScaledPixels>) -> RoundedMask {
    let radii = [
        mask.corner_radii.top_left.0,
        mask.corner_radii.top_right.0,
        mask.corner_radii.bottom_right.0,
        mask.corner_radii.bottom_left.0,
    ];
    if radii == [0.0; 4] {
        return RoundedMask::default();
    }
    RoundedMask {
        bounds: [
            mask.corner_bounds.origin.x.0,
            mask.corner_bounds.origin.y.0,
            mask.corner_bounds.size.width.0,
            mask.corner_bounds.size.height.0,
        ],
        radii,
    }
}

/// Each SceneView gets a separate target: sharing one color image across
/// multiple painter-order elements would make every composite sample the last
/// view's content, because all producer passes submit before the GUI pass.
struct Target {
    color: TextureId,
    color_view: TextureViewId,
    depth: TextureId,
    depth_view: TextureViewId,
    binding: BindGroupId,
    placement: BufferId,
    extent: Extent2d,
    region: ScissorRect,
    host_extent: Extent2d,
    /// Only rounded clip data participates in placement identity. A change
    /// to rectangular clip geometry is already represented by 'region'.
    rounded_mask: RoundedMask,
    capture: CaptureEpoch,
}

/// Submission means the native device accepted the producer commands and
/// the host will order subsequent GUI sampling after that queue submission.
/// It does NOT imply GPU fence completion or permit immediate native reuse.
#[derive(Default, Debug)]
struct CaptureEpoch {
    submitted: Option<u64>,
    queued: Option<u64>,
}

impl CaptureEpoch {
    fn reusable(&self, revision: u64) -> bool {
        self.submitted == Some(revision)
    }
    fn enqueue(&mut self, revision: u64) {
        self.queued = Some(revision);
    }
    fn acknowledge(&mut self, revision: u64) {
        if self.queued == Some(revision) {
            self.submitted = Some(revision);
            self.queued = None;
        }
    }
}

/// Uniform/bind-group generations are immutable while an older GUI frame
/// may still be using them. Relocating a SceneView retires only this pair,
/// not the expensive color/depth attachments.
struct PlacementResources {
    binding: BindGroupId,
    buffer: BufferId,
}

impl PlacementResources {
    fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        device.destroy_bind_group(self.binding)?;
        device.destroy_buffer(self.buffer)?;
        Ok(())
    }
}

impl Target {
    /// A SceneView may be painted more than once before the batch is submitted.
    /// Do not overwrite its producer attachment while an earlier painter-order
    /// command in the same pending frame still refers to that image.
    fn can_relocate(&self, extent: Extent2d) -> bool {
        self.extent == extent && self.capture.queued.is_none()
    }

    fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        PlacementResources { binding: self.binding, buffer: self.placement }.destroy(device)?;
        device.destroy_texture_view(self.depth_view)?;
        device.destroy_texture(self.depth)?;
        device.destroy_texture_view(self.color_view)?;
        device.destroy_texture(self.color)?;
        Ok(())
    }
}

fn scissor_overlaps_viewport(clip: ScissorRect, viewport: Extent2d) -> bool {
    !clip.is_empty() && clip.x < viewport.width() && clip.y < viewport.height()
}

/// A per-element color/depth attachment covers only the visible pixel
/// rectangle, unlike the previous full-window allocation for every widget.
/// Clamp to the swapchain size even when an outer ScrollView clips the item.
fn visible_region(host: &RendererExtensionContext) -> ScissorRect {
    let viewport = host.viewport();
    let clip = host.scissor();
    let x = clip.x.min(viewport.width() - 1);
    let y = clip.y.min(viewport.height() - 1);
    let right = clip.x.saturating_add(clip.width).min(viewport.width());
    let bottom = clip.y.saturating_add(clip.height).min(viewport.height());
    ScissorRect {
        x,
        y,
        width: right.saturating_sub(x).max(1),
        height: bottom.saturating_sub(y).max(1),
    }
}

fn region_extent(rect: ScissorRect) -> Extent2d {
    // visible_region always returns positive extents.
    Extent2d::new(rect.width.max(1), rect.height.max(1))
        .expect("cropped sidecar region must be nonzero")
}

/// Three 16-byte aligned vectors, shared with sidecar_composite.wgsl:
/// NDC top-left/delta, GPUI rounded content-mask bounds and corner radii.
/// The mask is expressed in host-window pixels because @builtin(position)
/// on the composite fragment is also in host-window pixels.
fn placement_bytes(
    rect: ScissorRect,
    viewport: Extent2d,
    mask: &gpui::ContentMask<gpui::ScaledPixels>,
) -> [u8; 48] {
    let width = viewport.width() as f32;
    let height = viewport.height() as f32;
    let rounded = rounded_mask(mask);
    let values = [
        -1.0 + 2.0 * rect.x as f32 / width,
        1.0 - 2.0 * rect.y as f32 / height,
        2.0 * rect.width as f32 / width,
        -2.0 * rect.height as f32 / height,
        rounded.bounds[0],
        rounded.bounds[1],
        rounded.bounds[2],
        rounded.bounds[3],
        rounded.radii[0],
        rounded.radii[1],
        rounded.radii[2],
        rounded.radii[3],
    ];
    let mut bytes = [0u8; 48];
    for (chunk, value) in bytes.chunks_exact_mut(4).zip(values) {
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// SceneResources uses the cropped capture context: command scissors are
/// already in target-local pixels. Intersect them with a local (0,0) extent,
/// NEVER with the absolute window-space region a second time.
fn clipped_scissor(previous: Option<ScissorRect>, extent: Extent2d) -> ScissorRect {
    let clip = ScissorRect {
        x: 0, y: 0, width: extent.width(), height: extent.height(),
    };
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

/// Native render-pass bounds already clip commands with no explicit scissor.
/// Preserve that fast path: rewriting every DrawIndirect to a full-target
/// scissor prevents some backends from retaining/coalescing its native state.
fn clip_local_commands(
    commands: &mut Arc<[RenderCommand]>,
    extent: Extent2d,
    authored_scissors: bool,
) {
    // CachedCommands computed this bit once when it published the revision.
    // The common GPU-generated path now performs O(1) work here, retaining
    // the original Arc, instead of a full per-frame O(draws) scissor scan.
    if !authored_scissors {
        return;
    }
    // A rare explicitly clipped pass gets a private, compacted snapshot:
    // never mutate a command list that other in-flight passes may borrow.
    let mut clipped: Vec<RenderCommand> = commands.iter().cloned().collect();
    for command in &mut clipped {
        if let Some(scissor) = command.scissor() {
            command.set_scissor(Some(clipped_scissor(Some(scissor), extent)));
        }
    }
    clipped.retain(|command| command.scissor().is_none_or(|rect| !rect.is_empty()));
    *commands = clipped.into();
}

/// Capture and allocation counters are only emitted when the existing 3D
/// timing diagnostics are enabled. These count REAL attachment reallocations,
/// not changes of a view's absolute window coordinates.
#[derive(Default)]
struct TargetTrace {
    window_start: Option<Instant>,
    allocated: u64,
    relocated: u64,
    captured: u64,
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
    /// Previously submitted color/depth attachments cannot be destroyed on
    /// resize/scene eviction until the host proves GPU queue completion.
    awaiting_gpu: Vec<Target>,
    /// Fence-proven resources are destroyed by the next owner-thread device
    /// access. The completion callback intentionally has no ExtensionDevice.
    ready_to_release: Vec<Target>,
    /// Placement-only generations are fence-retired independently. Moving an
    /// unchanged-size view must not evict its color/depth GPU textures.
    awaiting_placement_gpu: Vec<PlacementResources>,
    ready_placement_release: Vec<PlacementResources>,
    /// A presentation pass/format switch must not synchronously destroy the
    /// previous compositor PSO while an earlier GUI submission still uses it.
    awaiting_pipeline_gpu: Vec<RenderPipelineId>,
    ready_pipeline_release: Vec<RenderPipelineId>,
    trace: TargetTrace,
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
                    BindGroupLayoutEntry {
                        binding: 2,
                        binding_type: BindingType::UniformBuffer,
                        stages: ShaderStages::VERTEX,
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
                awaiting_gpu: Vec::new(),
                ready_to_release: Vec::new(),
                awaiting_placement_gpu: Vec::new(),
                ready_placement_release: Vec::new(),
                awaiting_pipeline_gpu: Vec::new(),
                ready_pipeline_release: Vec::new(),
                trace: TargetTrace::default(),
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
        let region = visible_region(host);
        host.offscreen_region(
            self.capture_pass,
            self.capture_format,
            (region.x, region.y),
            region_extent(region),
        )
    }

    fn create_target(
        &self,
        device: &mut dyn ExtensionDevice,
        region: ScissorRect,
        host: &RendererExtensionContext,
    ) -> gpui::Result<Target> {
        let extent = region_extent(region);
        let host_extent = host.viewport();
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
        let PlacementResources { binding, buffer: placement } =
            match self.create_placement(device, color_view, region, host) {
                Ok(placement) => placement,
                Err(error) => {
                    let _ = device.destroy_texture_view(depth_view);
                    let _ = device.destroy_texture(depth);
                    let _ = device.destroy_texture_view(color_view);
                    let _ = device.destroy_texture(color);
                    return Err(error);
                }
            };
        Ok(Target {
            color, color_view, depth, depth_view, binding, placement, extent,
            region, host_extent, rounded_mask: rounded_mask(host.content_mask()),
            capture: CaptureEpoch::default(),
        })
    }

    /// Allocate a new immutable placement without reallocating an existing
    /// render target. The old binding/buffer remain live until a host fence.
    fn create_placement(
        &self,
        device: &mut dyn ExtensionDevice,
        color_view: TextureViewId,
        region: ScissorRect,
        host: &RendererExtensionContext,
    ) -> gpui::Result<PlacementResources> {
        let bytes = placement_bytes(region, host.viewport(), host.content_mask());
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("gpui-3d sidecar crop placement and rounded mask".into()),
            size: bytes.len() as u64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::GpuOnly,
        })?;
        if let Err(error) = device.write_buffer(buffer, 0, &bytes) {
            let _ = device.destroy_buffer(buffer);
            return Err(error.into());
        }
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
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Buffer(BufferBinding {
                        buffer,
                        offset: 0,
                        size: 48,
                        stride: None,
                    }),
                },
            ],
        }) {
            Ok(binding) => binding,
            Err(error) => {
                let _ = device.destroy_buffer(buffer);
                return Err(error.into());
            }
        };
        Ok(PlacementResources { binding, buffer })
    }

    /// An element completely outside the host viewport must not allocate a
    /// synthetic one-pixel color/depth target or schedule any 3D work.
    pub(super) fn visible(host: &RendererExtensionContext) -> bool {
        scissor_overlaps_viewport(host.scissor(), host.viewport())
    }

    /// Exposes only resources which are awaiting a real GPU fence.
    /// A submission acknowledgement alone is not enough to release them.
    pub(super) fn has_retired_targets(&self) -> bool {
        !self.awaiting_gpu.is_empty()
            || !self.awaiting_placement_gpu.is_empty()
            || !self.awaiting_pipeline_gpu.is_empty()
    }

    /// Called only after GPUI verifies all earlier graphics submissions
    /// completed. Destruction still occurs on the native device owner.
    pub(super) fn gpu_work_completed(&mut self) {
        self.ready_to_release.append(&mut self.awaiting_gpu);
        self.ready_placement_release.append(&mut self.awaiting_placement_gpu);
        self.ready_pipeline_release.append(&mut self.awaiting_pipeline_gpu);
    }

    fn release_completed(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        // Retired descriptor generations can reference still-active color
        // targets, so release them separately after verified completion.
        while let Some(placement) = self.ready_placement_release.pop() {
            placement.destroy(device)?;
        }
        // Destroy only fence-proven targets on the native owner. A backend
        // error propagates to GPUI rather than being silently ignored.
        while let Some(target) = self.ready_to_release.pop() {
            target.destroy(device)?;
        }
        // PSO destruction on Vulkan can wait for pending work. By reaching
        // this queue only after the host fence proof, a host format change
        // cannot force that wait during the new frame's render preparation.
        while let Some(pipeline) = self.ready_pipeline_release.pop() {
            device.destroy_render_pipeline(pipeline)?;
        }
        Ok(())
    }

    /// Estimate only color/depth texel storage. Actual driver allocation
    /// overhead, row alignment and bind-group memory are not included.
    fn trace_targets_if_due(&mut self) {
        if !super::renderer::trace_timings() {
            return;
        }
        let now = Instant::now();
        let start = self.trace.window_start.get_or_insert(now);
        if now.saturating_duration_since(*start) < Duration::from_secs(1) {
            return;
        }
        let bpp = u64::from(self.capture_format.bytes_per_pixel())
            + u64::from(Format::Depth32Float.bytes_per_pixel());
        let sum_pixels = |width: u32, height: u32| {
            u64::from(width).saturating_mul(u64::from(height))
        };
        let cropped = self.targets.values().fold(0u64, |sum, target| {
            sum.saturating_add(sum_pixels(target.extent.width(), target.extent.height()))
        });
        let full_window = self.targets.values().fold(0u64, |sum, target| {
            sum.saturating_add(sum_pixels(
                target.host_extent.width(), target.host_extent.height(),
            ))
        });
        let retired = self.awaiting_gpu.iter()
            .chain(self.ready_to_release.iter())
            .fold(0u64, |sum, target| {
                sum.saturating_add(sum_pixels(target.extent.width(), target.extent.height()))
            });
        eprintln!(
            "gpui3d sidecar_targets active={} created={} moved_without_recreate={} captured={} retired_targets={} retired_bindings={} retired_pipelines={} cropped_bytes={} full_window_equivalent_bytes={} retired_bytes={} estimated_saved_percent={:.1}",
            self.targets.len(), self.trace.allocated, self.trace.relocated, self.trace.captured,
            self.awaiting_gpu.len() + self.ready_to_release.len(),
            self.awaiting_placement_gpu.len() + self.ready_placement_release.len(),
            self.awaiting_pipeline_gpu.len() + self.ready_pipeline_release.len(),
            cropped.saturating_mul(bpp),
            full_window.saturating_mul(bpp),
            retired.saturating_mul(bpp),
            100.0 * (1.0 - cropped as f64 / full_window.max(1) as f64),
        );
        self.trace = TargetTrace {
            window_start: Some(now),
            ..TargetTrace::default()
        };
    }

    pub(super) fn sync(
        &mut self,
        device: &mut dyn ExtensionDevice,
        host: &RendererExtensionContext,
    ) -> gpui::Result<()> {
        self.release_completed(device)?;
        self.trace_targets_if_due();
        if self.host_pass == host.render_pass()
            && self.host_format == host.color_format()
        {
            // Native texture-pass viewport/scissor are dynamic. A window
            // resize changes placement, not the composite pipeline PSO.
            self.host_extent = host.viewport();
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
        // The old pipeline may still be referenced by native GUI command
        // buffers; the host retains it until a verified queue completion.
        self.awaiting_pipeline_gpu.push(old);
        Ok(())
    }

    /// Move the composited image without reallocating its color/depth
    /// textures. The scene's capture revision already includes projection
    /// changes: an unchanged revision can reuse the last producer pixels
    /// even if this widget's absolute window position has moved.
    ///
    /// The previous immutable uniform/bind-group pair is fence-retired so
    /// an older in-flight compositor draw cannot observe a new placement.
    pub(super) fn sync_placement(
        &mut self,
        device: &mut dyn ExtensionDevice,
        id: SceneViewId,
        host: &RendererExtensionContext,
    ) -> gpui::Result<()> {
        let region = visible_region(host);
        let extent = region_extent(region);
        let Some(target) = self.targets.get(&id) else {
            return Ok(());
        };
        if !target.can_relocate(extent)
            || (target.region == region
                && target.host_extent == host.viewport()
                && target.rounded_mask == rounded_mask(host.content_mask()))
        {
            // Resizes and still-pending painter-order producers require a new
            // attachment; enqueue owns the transactional replacement.
            return Ok(());
        }
        let updated = self.create_placement(
            device, target.color_view, region, host,
        )?;
        let target = self.targets.get_mut(&id).expect("checked above");
        let previous = PlacementResources {
            binding: std::mem::replace(&mut target.binding, updated.binding),
            buffer: std::mem::replace(&mut target.placement, updated.buffer),
        };
        target.region = region;
        target.host_extent = host.viewport();
        target.rounded_mask = rounded_mask(host.content_mask());
        self.awaiting_placement_gpu.push(previous);
        self.trace.relocated = self.trace.relocated.saturating_add(1);
        Ok(())
    }

    /// A successful 3D scene prepare did not change anything visible.
    /// Reusing the already submitted producer texture avoids an entire
    /// offscreen GPU pass and avoids rebuilding native Draw commands for an
    /// unrelated GUI-only frame.
    pub(super) fn can_reuse(
        &self,
        id: SceneViewId,
        host: &RendererExtensionContext,
        capture_revision: u64,
    ) -> bool {
        let region = visible_region(host);
        self.targets.get(&id).is_some_and(|target| {
            target.region == region
                && target.host_extent == host.viewport()
                && target.rounded_mask == rounded_mask(host.content_mask())
                && target.capture.reusable(capture_revision)
        })
    }

    /// Composite a completed previous producer texture without scheduling
    /// another 3D draw. May also be used while a newer CPU render job is
    /// preparing a Scene replacement. This is intentionally non-blocking.
    pub(super) fn composite_existing(
        &self,
        id: SceneViewId,
        host: &RendererExtensionContext,
        gui_steps: &mut Vec<RenderCommand>,
    ) -> bool {
        let Some(target) = self.targets.get(&id) else {
            return false;
        };
        if target.region != visible_region(host)
            || target.host_extent != host.viewport()
            || target.rounded_mask != rounded_mask(host.content_mask())
            || target.capture.submitted.is_none()
        {
            return false;
        }
        gui_steps.push(RenderCommand::Draw(DrawCommand {
            pipeline: self.composite_pipeline,
            bind_groups: bind_group_list([target.binding]),
            vertex_count: 3,
            first_vertex: 0,
            instance_count: 1,
            first_instance: 0,
            scissor: None,
        }));
        true
    }

    /// Accept only capture epochs whose producer passes were actually
    /// drained and acknowledged by the host. Never treat a queued but failed
    /// native submission as a published texture.
    pub(super) fn render_passes_submitted(
        &mut self,
        submitted: &[(SceneViewId, u64)],
    ) {
        for &(id, revision) in submitted {
            if let Some(target) = self.targets.get_mut(&id) {
                target.capture.acknowledge(revision);
            }
        }
    }

    pub(super) fn enqueue(
        &mut self,
        device: &mut dyn ExtensionDevice,
        id: SceneViewId,
        host: &RendererExtensionContext,
        capture_revision: u64,
        gpu_work_tag: Option<gfx_core::GpuTimestampTag>,
        mut geometry: Arc<[RenderCommand]>,
        authored_scissors: bool,
        indirect_only: bool,
        passes: &mut Vec<RendererExtensionPass>,
        gui_steps: &mut Vec<RenderCommand>,
    ) -> gpui::Result<()> {
        let region = visible_region(host);
        // SceneResources already prepared commands for capture_context's
        // local viewport. The native target clips unscissored commands for
        // free; only intersect authored local scissors when they are present.
        let extent = region_extent(region);
        clip_local_commands(&mut geometry, extent, authored_scissors);
        let recreate = self.targets.get(&id)
            .is_none_or(|target| !target.can_relocate(extent));
        if recreate {
            let new_target = self.create_target(device, region, host)?;
            if let Some(previous) = self.targets.insert(id, new_target) {
                self.awaiting_gpu.push(previous);
            }
            self.trace.allocated = self.trace.allocated.saturating_add(1);
        } else {
            self.sync_placement(device, id, host)?;
        }
        let target = self.targets.get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("missing 3D offscreen target"))?;
        target.capture.enqueue(capture_revision);
        self.trace.captured = self.trace.captured.saturating_add(1);
        passes.push(RendererExtensionPass {
            target: target.color_view,
            // Eligibility is cached with this immutable command revision.
            // Hardware timestamp tagging must never rescan thousands of
            // native DrawIndirect records on each camera-only capture.
            gpu_work_tag: gpu_work_tag.filter(|_| indirect_only),
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
        // Capture is queued, but only becomes reusable after the host
        // acknowledges native submission. GUI sampling this same frame is
        // ordered by the host's producer -> consumer dependency.
        gui_steps.push(RenderCommand::Draw(DrawCommand {
            pipeline: self.composite_pipeline,
            bind_groups: bind_group_list([target.binding]),
            vertex_count: 3,
            first_vertex: 0,
            instance_count: 1,
            first_instance: 0,
            scissor: None,
        }));
        Ok(())
    }

    pub(super) fn retire(
        &mut self,
        device: &mut dyn ExtensionDevice,
        id: SceneViewId,
    ) -> gpui::Result<()> {
        self.release_completed(device)?;
        if let Some(target) = self.targets.remove(&id) {
            self.awaiting_gpu.push(target);
        }
        Ok(())
    }

    pub(super) fn destroy(mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        // Final teardown is owned by the host, which drains its native
        // pending submissions before disposing of the renderer. Destroy all
        // active/fence-ready resources through that same native device.
        for (_, target) in self.targets.drain() {
            target.destroy(device)?;
        }
        for target in self.awaiting_gpu.drain(..).chain(self.ready_to_release.drain(..)) {
            target.destroy(device)?;
        }
        for placement in self.awaiting_placement_gpu.drain(..)
            .chain(self.ready_placement_release.drain(..))
        {
            placement.destroy(device)?;
        }
        for pipeline in self.awaiting_pipeline_gpu.drain(..)
            .chain(self.ready_pipeline_release.drain(..))
        {
            device.destroy_render_pipeline(pipeline)?;
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
    fn detached_targets_are_reclaimed_only_after_completion_proof() {
        // No hardware device is required: the fake handles are never used or
        // destroyed; this validates the ownership transition only.
        let handle = |id| id;
        let mut surface = SidecarSurface {
            capture_pass: RenderPassId::from_parts(1, 1),
            capture_format: Format::Rgba8Unorm,
            host_pass: RenderPassId::from_parts(2, 1),
            host_format: Format::Rgba8Unorm,
            host_extent: Extent2d::new(32, 32).unwrap(),
            vertex_shader: ShaderModuleId::from_parts(1, 1),
            fragment_shader: ShaderModuleId::from_parts(2, 1),
            sampler: SamplerId::from_parts(1, 1),
            bind_layout: BindGroupLayoutId::from_parts(1, 1),
            pipeline_layout: PipelineLayoutId::from_parts(1, 1),
            composite_pipeline: RenderPipelineId::from_parts(1, 1),
            targets: HashMap::new(),
            awaiting_gpu: vec![Target {
                color: TextureId::from_parts(handle(1), 1),
                color_view: TextureViewId::from_parts(1, 1),
                depth: TextureId::from_parts(handle(2), 1),
                depth_view: TextureViewId::from_parts(2, 1),
                binding: BindGroupId::from_parts(1, 1),
                placement: BufferId::from_parts(1, 1),
                extent: Extent2d::new(32, 32).unwrap(),
                region: ScissorRect { x: 0, y: 0, width: 32, height: 32 },
                host_extent: Extent2d::new(32, 32).unwrap(),
                rounded_mask: RoundedMask::default(),
                capture: CaptureEpoch::default(),
            }],
            ready_to_release: Vec::new(),
            awaiting_placement_gpu: Vec::new(),
            ready_placement_release: Vec::new(),
            awaiting_pipeline_gpu: Vec::new(),
            ready_pipeline_release: Vec::new(),
            trace: TargetTrace::default(),
        };
        assert!(surface.has_retired_targets());
        assert_eq!(surface.ready_to_release.len(), 0);
        let extent = Extent2d::new(32, 32).unwrap();
        assert!(surface.awaiting_gpu[0].can_relocate(extent));
        surface.awaiting_gpu[0].capture.enqueue(10);
        assert!(!surface.awaiting_gpu[0].can_relocate(extent));
        surface.awaiting_gpu[0].capture.acknowledge(10);
        assert!(surface.awaiting_gpu[0].can_relocate(extent));
        assert!(!surface.awaiting_gpu[0].can_relocate(
            Extent2d::new(64, 32).unwrap(),
        ));
        surface.awaiting_placement_gpu.push(PlacementResources {
            binding: BindGroupId::from_parts(7, 1),
            buffer: BufferId::from_parts(8, 1),
        });
        surface.awaiting_pipeline_gpu.push(RenderPipelineId::from_parts(11, 1));
        surface.gpu_work_completed();
        assert!(!surface.has_retired_targets());
        assert_eq!(surface.awaiting_gpu.len(), 0);
        assert_eq!(surface.ready_to_release.len(), 1);
        assert_eq!(surface.awaiting_placement_gpu.len(), 0);
        assert_eq!(surface.ready_placement_release.len(), 1);
        assert_eq!(surface.awaiting_pipeline_gpu.len(), 0);
        assert_eq!(surface.ready_pipeline_release.len(), 1);
    }

    #[test]
    fn capture_is_reused_only_after_matching_gpu_submission_acknowledgement() {
        let mut capture = CaptureEpoch::default();
        assert!(!capture.reusable(42));
        capture.enqueue(42);
        assert!(!capture.reusable(42),
            "recorded commands are not evidence that the GPU accepted them");
        capture.acknowledge(41);
        assert!(!capture.reusable(42), "stale submission must not publish");
        capture.acknowledge(42);
        assert!(capture.reusable(42));
        assert!(!capture.reusable(43));
        capture.enqueue(43);
        assert!(capture.reusable(42),
            "a previously submitted texture remains readable until replaced");
        assert!(!capture.reusable(43));
        capture.acknowledge(43);
        assert!(capture.reusable(43));
        assert!(!capture.reusable(42));
    }

    #[test]
    fn cropped_region_saves_color_and_depth_memory_and_preserves_placement() {
        let viewport = Extent2d::new(1920, 1080).unwrap();
        let bounds = gpui::Bounds {
            origin: gpui::point(gpui::ScaledPixels::from(400.0), gpui::ScaledPixels::from(200.0)),
            size: gpui::size(gpui::ScaledPixels::from(480.0), gpui::ScaledPixels::from(320.0)),
        };
        let host = RendererExtensionContext::for_target(
            BackendKind::Vulkan,
            RenderPassId::from_parts(1, 1),
            Format::Rgba8Unorm,
            viewport,
        );
        // Visible crop is based on the real host scissor, not the frame
        // texture size. Verify the math using the target rectangle below.
        let rect = ScissorRect { x: 400, y: 200, width: 480, height: 320 };
        let extent = region_extent(rect);
        assert_eq!((extent.width(), extent.height()), (480, 320));
        let before = u64::from(viewport.width()) * u64::from(viewport.height()) * 8;
        let after = u64::from(extent.width()) * u64::from(extent.height()) * 8;
        assert!(after * 10 < before, "two RGBA/depth 32-bit targets should be ~13x smaller");
        let bytes = placement_bytes(rect, viewport, &gpui::ContentMask::default());
        let values = std::array::from_fn::<_, 4, _>(|i| {
            f32::from_le_bytes(bytes[i * 4..(i + 1) * 4].try_into().unwrap())
        });
        assert!((values[0] - (-1.0 + 800.0 / 1920.0)).abs() < 1.0e-5);
        assert!((values[1] - (1.0 - 400.0 / 1080.0)).abs() < 1.0e-5);
        assert!((values[2] - (960.0 / 1920.0)).abs() < 1.0e-5);
        assert!((values[3] - (-640.0 / 1080.0)).abs() < 1.0e-5);
        assert_eq!(host.viewport(), viewport);
        assert_eq!(bounds.size.width, gpui::ScaledPixels::from(480.0));
    }

    #[test]
    fn rounded_content_mask_has_separate_premultiplied_composite_uniforms() {
        let viewport = Extent2d::new(1920, 1080).unwrap();
        let rect = ScissorRect { x: 400, y: 200, width: 480, height: 320 };
        let mask = gpui::ContentMask {
            bounds: gpui::bounds(
                gpui::point(gpui::ScaledPixels(400.0), gpui::ScaledPixels(200.0)),
                gpui::size(gpui::ScaledPixels(480.0), gpui::ScaledPixels(320.0)),
            ),
            corner_bounds: gpui::bounds(
                gpui::point(gpui::ScaledPixels(405.0), gpui::ScaledPixels(205.0)),
                gpui::size(gpui::ScaledPixels(470.0), gpui::ScaledPixels(310.0)),
            ),
            corner_radii: gpui::Corners {
                top_left: gpui::ScaledPixels(12.0),
                top_right: gpui::ScaledPixels(8.0),
                bottom_right: gpui::ScaledPixels(16.0),
                bottom_left: gpui::ScaledPixels(4.0),
            },
        };
        let bytes = placement_bytes(rect, viewport, &mask);
        assert_eq!(bytes.len(), 3 * 16);
        let values: Vec<f32> = bytes.chunks_exact(4)
            .map(|word| f32::from_le_bytes(word.try_into().unwrap()))
            .collect();
        assert_eq!(&values[4..8], &[405.0, 205.0, 470.0, 310.0]);
        assert_eq!(&values[8..12], &[12.0, 8.0, 16.0, 4.0]);
        let rectangular = placement_bytes(rect, viewport, &gpui::ContentMask::default());
        assert_eq!(&rectangular[32..48], &[0u8; 16],
            "rectangular widgets must take the shader's zero-mask fast path");
    }

    #[test]
    fn rectangular_mask_changes_do_not_recreate_composite_bindings() {
        let plain = gpui::ContentMask::<gpui::ScaledPixels>::default();
        let shifted = gpui::ContentMask::new(gpui::bounds(
            gpui::point(gpui::ScaledPixels(72.0), gpui::ScaledPixels(90.0)),
            gpui::size(gpui::ScaledPixels(160.0), gpui::ScaledPixels(240.0)),
        ));
        assert_eq!(rounded_mask(&plain), rounded_mask(&shifted));
        let rounded = gpui::ContentMask {
            corner_bounds: shifted.bounds,
            corner_radii: gpui::Corners::from(gpui::ScaledPixels(14.0)),
            ..shifted
        };
        assert_ne!(rounded_mask(&plain), rounded_mask(&rounded));
    }

    #[test]
    fn entirely_clipped_scene_does_not_request_an_offscreen_allocation() {
        let viewport = Extent2d::new(1920, 1080).unwrap();
        let host = RendererExtensionContext::for_target(
            BackendKind::Vulkan, RenderPassId::from_parts(1, 1),
            Format::Rgba8Unorm, viewport,
        );
        assert!(SidecarSurface::visible(&host));
        let cropped = host.offscreen_region(
            host.render_pass(), host.color_format(), (0, 0),
            Extent2d::new(1, 1).unwrap(),
        );
        assert!(SidecarSurface::visible(&cropped));
        assert!(!scissor_overlaps_viewport(
            ScissorRect { x: 1920, y: 1080, width: 10, height: 10 }, viewport,
        ));
        assert!(!scissor_overlaps_viewport(
            ScissorRect { x: 0, y: 0, width: 0, height: 500 }, viewport,
        ));
        assert!(scissor_overlaps_viewport(
            ScissorRect { x: 1919, y: 1079, width: 10, height: 10 }, viewport,
        ));
    }

    #[test]
    fn unscissored_indirect_draws_keep_the_native_full_target_fast_path() {
        let extent = Extent2d::new(480, 320).unwrap();
        let mut commands: Arc<[RenderCommand]> = vec![RenderCommand::DrawIndirect(DrawIndirectCommand {
            pipeline: RenderPipelineId::from_parts(1, 1),
            bind_groups: bind_group_list([]),
            arguments: BufferId::from_parts(1, 1),
            offset: 0,
            scissor: None,
        })].into();
        let original = Arc::clone(&commands);
        clip_local_commands(&mut commands, extent, false);
        assert_eq!(commands.len(), 1);
        assert!(commands[0].scissor().is_none());
        assert!(Arc::ptr_eq(&commands, &original), "no clipping must preserve the original GPU command allocation");
    }

    #[test]
    fn explicit_scissors_use_a_private_clipped_command_snapshot() {
        let extent = Extent2d::new(480, 320).unwrap();
        let mut commands: Arc<[RenderCommand]> = vec![RenderCommand::DrawIndirect(
            DrawIndirectCommand {
                pipeline: RenderPipelineId::from_parts(1, 1),
                bind_groups: bind_group_list([]),
                arguments: BufferId::from_parts(1, 1),
                offset: 0,
                scissor: Some(ScissorRect { x: 450, y: 20, width: 100, height: 40 }),
            }
        )].into();
        let original = Arc::clone(&commands);
        clip_local_commands(&mut commands, extent, true);
        assert!(!Arc::ptr_eq(&commands, &original));
        assert_eq!(original[0].scissor().unwrap().width, 100);
        assert_eq!(commands[0].scissor().unwrap().width, 30);
    }

    #[test]
    fn offscreen_geometry_obeys_original_gui_clip() {
        // The absolute widget origin may be (400, 200), but its producer
        // commands always operate on a (0, 0) local 480x320 attachment.
        let local = Extent2d::new(480, 320).unwrap();
        let source = ScissorRect { x: 100, y: 0, width: 450, height: 100 };
        assert_eq!(clipped_scissor(Some(source), local),
            ScissorRect { x: 100, y: 0, width: 380, height: 100 });
        assert_eq!(clipped_scissor(None, local),
            ScissorRect { x: 0, y: 0, width: 480, height: 320 });
        let outside = ScissorRect { x: 500, y: 300, width: 40, height: 40 };
        assert!(clipped_scissor(Some(outside), local).is_empty());
    }
}
