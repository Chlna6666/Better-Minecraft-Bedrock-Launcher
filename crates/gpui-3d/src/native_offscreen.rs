//! Framework-independent native 3D graphics submission planning.
//!
//! A game or a GUI sidecar can share the same GPU-resident DrawIndirect
//! commands and native color/depth attachments. This module uses gfx-core
//! only: it does not construct GPUI paint primitives, acquire a swapchain,
//! wait for a CPU fence, or own a backend-specific device.

use gfx_core::{
    ClearColor, DepthAttachment, GpuTimestampTag, LoadOp, OffscreenQueuePreference,
    OffscreenSubmissionDevice, RenderCommand, RenderPassCommands, RenderPassId,
    Result, SubmissionId, TextureViewId,
};
use std::sync::Arc;

/// One immutable native 3D graphics pass, independent of window or GUI layout.
///
/// The command allocation is an Arc so camera-only SceneView updates reuse
/// stable GPU-resident draw descriptors. Each pass can own a depth attachment
/// and is encoded by gfx-core without conversion into UI geometry.
pub struct NativeOffscreenPass {
    /// Color image to render to; the presentation adapter may sample its view.
    pub target: TextureViewId,
    /// Backend-neutral native render-pass layout.
    pub render_pass: RenderPassId,
    /// Immutable draw commands, including native indirect draws.
    pub commands: Arc<[RenderCommand]>,
    /// Optional hardware GPU timestamp; never substitute host/API time.
    pub gpu_work_tag: Option<GpuTimestampTag>,
    /// How to initialize or preserve the color attachment.
    pub color_load_op: LoadOp<ClearColor>,
    /// Optional native depth/stencil attachment.
    pub depth_attachment: Option<DepthAttachment>,
}

impl NativeOffscreenPass {
    /// Borrows the existing command slice and native render targets without
    /// cloning or converting any DrawIndirect record.
    #[must_use]
    pub fn commands(&self) -> RenderPassCommands<'_> {
        RenderPassCommands {
            color_attachments: std::slice::from_ref(&self.target),
            render_pass: self.render_pass,
            commands: &self.commands,
            color_load_op: self.color_load_op,
            clear_region: None,
            viewport: None,
            resolve_target: None,
            depth_attachment: self.depth_attachment,
        }
    }
}

/// Submit an immutable native 3D pass batch through an arbitrary device owner.
///
/// The caller supplies the native queue submission callback: a standalone
/// game renderer can call `OffscreenSubmissionDevice` directly, while a GUI
/// sidecar forwards the same commands through the host's device owner. For
/// one SceneView the descriptors are stack-allocated. The native API is
/// responsible for GPU-side resource visibility and submission ordering.
///
/// The returned ID denotes command acceptance; never treat it as a completed
/// GPU fence for buffer/texture retirement.
pub fn submit_native_offscreen_passes(
    passes: &[NativeOffscreenPass],
    mut submit: impl FnMut(
        &[RenderPassCommands<'_>],
        &[Option<GpuTimestampTag>],
    ) -> Result<Option<SubmissionId>>,
) -> Result<Option<SubmissionId>> {
    let Some((first, rest)) = passes.split_first() else {
        return Ok(None);
    };
    if rest.is_empty() {
        let command = first.commands();
        return submit(std::slice::from_ref(&command), std::slice::from_ref(&first.gpu_work_tag));
    }
    let commands = passes.iter().map(NativeOffscreenPass::commands).collect::<Vec<_>>();
    let tags = passes.iter().map(|p| p.gpu_work_tag).collect::<Vec<_>>();
    submit(&commands, &tags)
}

/// Submit the 3D batch directly to gfx-core without a GPUI dependency.
///
/// This entry point is suitable for stand-alone games and offscreen engines
/// that already own a native device. GUI adapters may use
/// `submit_native_offscreen_passes` to borrow the host's GPU-owner device.
pub fn submit_native_graphics<D: OffscreenSubmissionDevice>(
    device: &mut D,
    passes: &[NativeOffscreenPass],
    queue: OffscreenQueuePreference,
) -> Result<Option<SubmissionId>> {
    submit_native_offscreen_passes(passes, |commands, tags| {
        device.submit_offscreen_graphics(commands, tags, queue)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx_core::{Error, GpuTimestampStage};

    fn pass(id: u32, tag: bool) -> NativeOffscreenPass {
        NativeOffscreenPass {
            target: TextureViewId::from_parts(id, 1),
            render_pass: RenderPassId::from_parts(1, 1),
            commands: Vec::<RenderCommand>::new().into(),
            gpu_work_tag: tag.then_some(GpuTimestampTag {
                batch_id: u64::from(id),
                stage: GpuTimestampStage::DrawIndirect,
            }),
            color_load_op: LoadOp::Load,
            depth_attachment: None,
        }
    }

    #[test]
    fn empty_work_never_submits_any_native_commands() {
        let mut calls = 0;
        let submission = submit_native_offscreen_passes(&[], |_, _| {
            calls += 1;
            Ok(None)
        }).unwrap();
        assert!(submission.is_none());
        assert_eq!(calls, 0);
    }

    #[test]
    fn one_view_borrows_original_draw_records() {
        let pass = pass(10, true);
        let original_ptr = pass.commands.as_ptr();
        let submitted = submit_native_offscreen_passes(&[pass], |commands, tags| {
            assert_eq!(commands.len(), 1);
            assert!(std::ptr::eq(commands[0].commands.as_ptr(), original_ptr));
            assert_eq!(tags.len(), 1);
            assert_eq!(tags[0].unwrap().batch_id, 10);
            Ok(None)
        });
        assert!(submitted.is_ok());
    }

    #[test]
    fn many_views_keep_painter_order_and_native_tags() {
        let passes = [pass(2, false), pass(3, true), pass(4, false)];
        let mut called = false;
        submit_native_offscreen_passes(&passes, |commands, tags| {
            called = true;
            assert_eq!(commands.len(), 3);
            assert_eq!(tags.len(), 3);
            assert_eq!(commands[0].color_attachments, &[passes[0].target]);
            assert_eq!(commands[1].color_attachments, &[passes[1].target]);
            assert_eq!(commands[2].color_attachments, &[passes[2].target]);
            assert!(tags[0].is_none());
            assert_eq!(tags[1].unwrap().batch_id, 3);
            assert!(tags[2].is_none());
            Ok(None)
        }).unwrap();
        assert!(called);
    }

    #[test]
    fn submission_error_does_not_claim_a_gpu_fence() {
        let err = submit_native_offscreen_passes(&[pass(1, false)], |_, _| {
            Err(Error::Unavailable("native queue unavailable".into()))
        });
        assert!(matches!(err, Err(Error::Unavailable(_))));
    }
}
