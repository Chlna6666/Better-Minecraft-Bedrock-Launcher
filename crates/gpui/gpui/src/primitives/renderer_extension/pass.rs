//! Owned offscreen commands encoded by an extension and submitted by its host.
use gfx_core::*;
use std::sync::Arc;

/// Compute work inserted between two host-owned graphics passes.
///
/// The index is relative to the renderer's current drained graphics
/// passes: zero means before its first Graphics pass, one means after the
/// first. The host preserves work order across stages sharing one boundary.
/// The final operation is always Graphics, so its submission/fence covers
/// the preceding Compute. Zero is necessary when cached shadow maps do not
/// need a Graphics refresh in the current frame.
///
/// This is a GPU-owner submission request, not a native command queue handle.
/// The device is responsible for resource transitions and GPU-side visibility.
#[derive(Clone, Copy, Debug)]
pub struct RendererExtensionComputePass {
    /// Boundary after which the host GPU owner schedules this compute dispatch.
    /// Zero is before the first graphics pass; N is after pass N - 1.
    pub after_graphics_pass: usize,
    /// Backend-neutral shader, resource bind group and workgroup dimensions.
    /// The host records and submits the command with the necessary GPU barriers.
    pub dispatch: ComputeBindGroupDispatch,
    /// Optional native GPU timer. No CPU-side substitute is generated.
    pub gpu_work_tag: Option<GpuTimestampTag>,
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
    /// Compact producer run metadata; the owner must forward this with commands.
    pub run_plan: Option<Arc<DrawRunPlan>>,
    /// Optional sampled image table at set one, shared by the entire pass.
    /// When present each command contains only its geometry set zero.
    pub sampled_table: Option<BindGroupId>,
    /// Optional native GPU hardware timing tag for a pure indirect-draw window.
    /// Backends without the capability still execute normally.
    pub gpu_work_tag: Option<GpuTimestampTag>,
    /// Color load/clear operation.
    pub color_load_op: LoadOp<ClearColor>,
    /// Optional depth attachment.
    pub depth_attachment: Option<DepthAttachment>,
}
impl RendererExtensionPass {
    /// Standalone/native test adapter. Preserve compact and shared-table
    /// metadata even when the caller appends a flat final composite pass.
    pub fn submit_batch<D: PresentationDevice>(
        device: &mut D, owned: &[Self], tail: &[RenderPassCommands<'_>],
    ) -> Result<Option<SubmissionId>> {
        let mut passes = owned.iter().map(Self::commands).collect::<Vec<_>>();
        if !owned.iter().any(|pass| pass.run_plan.is_some()) {
            passes.extend_from_slice(tail);
            if owned.iter().any(|pass| pass.sampled_table.is_some()) {
                let tables = owned.iter().map(|pass| pass.sampled_table)
                    .chain(std::iter::repeat_n(None, tail.len())).collect::<Vec<_>>();
                let tags = owned.iter().map(|pass| pass.gpu_work_tag)
                    .chain(std::iter::repeat_n(None, tail.len())).collect::<Vec<_>>();
                return device.render_to_textures_sampled_profiled(&passes, &tables, &[], &tags);
            }
            return device.render_to_textures(&passes);
        }
        let tail_owners = tail.iter().map(|pass| Arc::<[RenderCommand]>::from(pass.commands)).collect::<Vec<_>>();
        passes.extend(tail.iter().zip(&tail_owners).map(|(pass, owner)| RenderPassCommands {
            color_attachments: pass.color_attachments, render_pass: pass.render_pass,
            commands: owner.as_ref(), color_load_op: pass.color_load_op,
            depth_attachment: pass.depth_attachment, clear_region: pass.clear_region,
            viewport: pass.viewport, resolve_target: pass.resolve_target,
        }));
        let owners = owned.iter().map(|pass| Arc::clone(&pass.commands))
            .chain(tail_owners.iter().cloned()).collect::<Vec<_>>();
        let plans = owned.iter().map(|pass| pass.run_plan.clone())
            .chain(std::iter::repeat_n(None, tail.len())).collect::<Vec<_>>();
        let tables = owned.iter().map(|pass| pass.sampled_table)
            .chain(std::iter::repeat_n(None, tail.len())).collect::<Vec<_>>();
        let tags = owned.iter().map(|pass| pass.gpu_work_tag)
            .chain(std::iter::repeat_n(None, tail.len())).collect::<Vec<_>>();
        device.render_to_textures_runs_profiled(&passes, &owners, &plans, &tables, &tags)
    }

    /// Borrows this owned plan for the host graphics submission.
    /// The caller must forward both `sampled_table` and `run_plan` to the native batch API.
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
