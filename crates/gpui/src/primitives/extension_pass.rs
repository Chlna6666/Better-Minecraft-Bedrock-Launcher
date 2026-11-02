//! Owned offscreen commands encoded by an extension and submitted by its host.
use gfx_core::*;

/// One ordered offscreen pass. All handles belong to the host's current device.
/// Extensions keep referenced handles valid until submission. Submitted resources may then be
/// destroyed through the device, which retains their native storage for outstanding GPU work.
#[derive(Clone, Debug)]
pub struct RendererExtensionPass {
    /// Output color attachment, distinct from every sampled input in this pass.
    pub target: TextureViewId,
    /// Attachment-compatible render pass.
    pub render_pass: RenderPassId,
    /// Commands in draw order. Scissors are local to this offscreen target.
    pub commands: Vec<RenderCommand>,
    /// Color load/clear operation.
    pub color_load_op: LoadOp<ClearColor>,
    /// Optional depth attachment.
    pub depth_attachment: Option<DepthAttachment>,
}
impl RendererExtensionPass {
    /// Borrows this owned plan for the host graphics submission.
    pub fn commands(&self) -> RenderPassCommands<'_> {
        RenderPassCommands {
            texture_view: self.target,
            render_pass: self.render_pass,
            commands: &self.commands,
            color_load_op: self.color_load_op,
            depth_attachment: self.depth_attachment,
            clear_region: None,
            viewport: None,
        }
    }
}
