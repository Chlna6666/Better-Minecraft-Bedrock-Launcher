//! Owned offscreen commands encoded by an extension and submitted by its host.
use gfx_core::*;
use std::sync::Arc;

/// Compute work inserted between two host-owned graphics passes.
///
/// The index is one-based and relative to the renderer's current drained
/// graphics passes: `after_graphics_pass = 1` means after its first graphics
/// pass, before its second. The host validates the index and preserves the
/// order of dispatches sharing one boundary. The last operation must remain
/// a graphics pass so its submission/fence covers the preceding compute.
///
/// This is a GPU-owner submission request, not a native command queue handle.
/// The device is responsible for resource transitions and GPU-side visibility.
#[derive(Clone, Copy, Debug)]
pub struct RendererExtensionComputePass {
    pub after_graphics_pass: usize,
    pub dispatch: ComputeBindGroupDispatch,
}

/// One ordered offscreen pass. All handles belong to the host's current device.
/// Extensions keep referenced handles valid until submission. Submitted resources may then be
/// destroyed through the device, which retains their native storage for outstanding GPU work.
#[derive(Clone, Debug)]
pub struct RendererExtensionPass {
    /// Output color attachment, distinct from every sampled input in this pass.
    pub target: TextureViewId,
    /// Ordered MRT attachments. Empty retains the single-target semantics.
    /// An explicit set includes `target` as its first element.
    pub color_attachments: Arc<[TextureViewId]>,
    /// Optional single-sample resolve destination when [`Self::target`] is
    /// multisampled. The MSAA color is resolved into this view after the pass.
    pub resolve_target: Option<TextureViewId>,
    /// Attachment-compatible render pass.
    pub render_pass: RenderPassId,
    /// Immutable draw plan shared between producer frames. A camera-only
    /// update can submit another offscreen capture without copying hundreds
    /// of thousands of otherwise unchanged DrawIndirect descriptors.
    /// Scissors are local to the target, and the GPU owner retains this Arc
    /// until its native queue accepts the producer submission.
    pub commands: Arc<[RenderCommand]>,
    /// Optional native GPU hardware timing tag for a pure indirect-draw window.
    /// Backends without the capability still execute normally.
    pub gpu_work_tag: Option<GpuTimestampTag>,
    /// Color load/clear operation.
    pub color_load_op: LoadOp<ClearColor>,
    /// Optional depth attachment.
    pub depth_attachment: Option<DepthAttachment>,
}
impl RendererExtensionPass {
    /// Borrows this owned plan for the host graphics submission.
    pub fn commands(&self) -> RenderPassCommands<'_> {
        RenderPassCommands {
            color_attachments: if self.color_attachments.is_empty() {
                std::slice::from_ref(&self.target)
            } else {
                &self.color_attachments
            },
            render_pass: self.render_pass,
            commands: &self.commands,
            color_load_op: self.color_load_op,
            depth_attachment: self.depth_attachment,
            clear_region: None,
            viewport: None,
            resolve_target: self.resolve_target,
        }
    }
}
