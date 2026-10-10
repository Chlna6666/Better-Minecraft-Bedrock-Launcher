//! Pixel residency is independent of frame-slot draw-step descriptor caches.

use super::*;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Key {
    pub(super) scene_revision: u64,
    pub(super) texture_view: TextureViewId,
    pub(super) target_size: Extent2d,
    pub(super) viewport: DrawableSize,
    pub(super) format: Format,
    pub(super) pipeline: RenderPipelineId,
}

#[derive(Default)]
pub(super) struct Residency {
    resident: Option<Key>,
}

impl Residency {
    pub(super) fn begin(&mut self, key: Key) -> bool {
        if key.scene_revision != 0 && self.resident == Some(key) {
            return false;
        }
        // Clear can change the texture before a later pass or presentation fails.
        self.invalidate();
        true
    }

    pub(super) fn commit(&mut self, key: Key) {
        self.resident = (key.scene_revision != 0).then_some(key);
    }

    pub(super) fn invalidate(&mut self) {
        self.resident = None;
    }
}

pub(super) struct Pass<'a> {
    pub(super) texture_view: TextureViewId,
    pub(super) render_pass: RenderPassId,
    pub(super) steps: &'a [DrawStepDescriptor],
    pub(super) depth_attachment: RenderPassDepthAttachment,
}

pub(super) fn render<D: BackendPresentationCompat>(
    device: &mut D,
    pass: Pass<'_>,
    cpu_elapsed: &mut Duration,
) -> Result<()> {
    let started = Instant::now();
    let result = device.render_step_list_to_texture(
        pass.texture_view,
        pass.render_pass,
        RenderStepList::from_draw_steps(pass.steps),
        LoadOp::Clear(clear_color()),
        Some(pass.depth_attachment),
    );
    *cpu_elapsed = started.elapsed();
    result.map_err(Into::into)
}

#[cfg(test)]
mod tests;

#[cfg(all(test, target_os = "windows", feature = "nova-gfx-dx12"))]
mod native_tests;
