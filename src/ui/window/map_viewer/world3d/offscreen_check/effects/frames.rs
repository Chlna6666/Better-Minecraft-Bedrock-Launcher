//! Native frame submission and diagnostic pass timing for the ignored world fixture.
use super::*;
use gpui::{RendererExtensionPass, RendererExtensionRenderer};

pub(super) struct Attachment {
    pub(super) texture: TextureId,
    pub(super) view: TextureViewId,
}
impl Attachment {
    pub(super) fn new<D: Device>(device: &mut D, size: Extent2d, format: Format) -> Self {
        let usage = if format == Format::Depth32Float {
            TextureUsage::DEPTH_ATTACHMENT
        } else {
            TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC
        };
        let texture = ResourceDevice::create_texture(
            device,
            &TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                format,
                usage,
                memory_location: MemoryLocation::GpuOnly,
                dimension: TextureDimension::D2,
                sample_count: 1,
            },
        )
        .unwrap();
        let view = ResourceDevice::create_texture_view(
            device,
            &TextureViewDescriptor {
                label: None,
                texture,
                base_mip_level: 0,
                mip_level_count: 1,
                format,
            },
        )
        .unwrap();
        Self { texture, view }
    }
    pub(super) fn destroy<D: Device>(self, device: &mut D) {
        ResourceDevice::destroy_texture_view(device, self.view).unwrap();
        ResourceDevice::destroy_texture(device, self.texture).unwrap();
    }
}

pub(super) fn frame<D: Device + TextureTransferDevice>(
    device: &mut D,
    renderer: &mut dyn RendererExtensionRenderer,
    view: &SceneView,
    context: RendererExtensionContext,
    color: &Attachment,
    depth: &Attachment,
) -> (f64, f64, f64) {
    let start = Instant::now();
    let mut commands = Vec::new();
    renderer
        .render(view, device, context.clone(), &mut commands)
        .unwrap();
    let mut owned = Vec::new();
    renderer.drain_render_passes(&mut owned);
    let encode = start.elapsed().as_secs_f64() * 1000.;
    let mut passes = owned
        .iter()
        .map(RendererExtensionPass::commands)
        .collect::<Vec<_>>();
    passes.push(RenderPassCommands {
        color_attachments: std::slice::from_ref(&color.view),
        render_pass: context.render_pass(),
        commands: &commands,
        color_load_op: LoadOp::Clear(ClearColor::default()),
        depth_attachment: Some(DepthAttachment {
            target: depth.view,
            depth_load_op: LoadOp::Clear(1.),
            stencil_load_op: None,
        }),
        clear_region: None,
        viewport: None,
            resolve_target: None,

        });
    if std::env::var_os("GPUI_3D_TRACE_TIMINGS").is_some() {
        let command_count: usize = passes.iter().map(|pass| pass.commands.len()).sum();
        let indirect = passes
            .iter()
            .flat_map(|pass| pass.commands)
            .filter(|c| matches!(c, RenderCommand::DrawIndirect(_)))
            .count();
        eprintln!(
            "world3d frame_commands passes={} commands={} indirect={} path={:?}",
            passes.len(),
            command_count,
            indirect,
            device.compute_capabilities().voxel_execution_path(),
        );
    }
    let submitted = Instant::now();
    let submission = PresentationDevice::render_to_textures(device, &passes).unwrap();
    let record = submitted.elapsed().as_secs_f64() * 1000.;
    let waiting = Instant::now();
    if let Some(submission) = submission {
        SubmissionDevice::wait_submission(device, submission).unwrap();
    } else {
        SubmissionDevice::wait_queue_idle(device).unwrap();
    }
    renderer.render_passes_submitted();
    (encode, record, waiting.elapsed().as_secs_f64() * 1000.)
}

pub(super) fn inspect_passes<D: Device>(
    device: &mut D,
    renderer: &mut dyn RendererExtensionRenderer,
    view: &SceneView,
    context: RendererExtensionContext,
    color: &Attachment,
    depth: &Attachment,
) {
    let mut commands = Vec::new();
    renderer
        .render(view, device, context.clone(), &mut commands)
        .unwrap();
    let mut owned = Vec::new();
    renderer.drain_render_passes(&mut owned);
    // Diagnostic-only drains separate queued compute from graphics; never used by the editor.
    let start = Instant::now();
    SubmissionDevice::wait_queue_idle(device).unwrap();
    eprintln!(
        "world3d isolated queued_compute_wait_ms={:.3}",
        start.elapsed().as_secs_f64() * 1000.
    );
    let mut passes = owned
        .iter()
        .map(RendererExtensionPass::commands)
        .collect::<Vec<_>>();
    passes.push(RenderPassCommands {
        color_attachments: std::slice::from_ref(&color.view),
        render_pass: context.render_pass(),
        commands: &commands,
        color_load_op: LoadOp::Clear(ClearColor::default()),
        depth_attachment: Some(DepthAttachment {
            target: depth.view,
            depth_load_op: LoadOp::Clear(1.),
            stencil_load_op: None,
        }),
        clear_region: None,
        viewport: None,
            resolve_target: None,

        });
    for (index, pass) in passes.iter().enumerate() {
        let start = Instant::now();
        let submission =
            PresentationDevice::render_to_textures(device, std::slice::from_ref(pass)).unwrap();
        let record = start.elapsed().as_secs_f64() * 1000.;
        let waiting = Instant::now();
        if let Some(submission) = submission {
            SubmissionDevice::wait_submission(device, submission).unwrap();
        } else {
            SubmissionDevice::wait_queue_idle(device).unwrap();
        }
        eprintln!(
            "world3d isolated pass={index} commands={} record_ms={:.3} fence_wait_ms={:.3}",
            pass.commands.len(),
            record,
            waiting.elapsed().as_secs_f64() * 1000.
        );
    }
    renderer.render_passes_submitted();
}
