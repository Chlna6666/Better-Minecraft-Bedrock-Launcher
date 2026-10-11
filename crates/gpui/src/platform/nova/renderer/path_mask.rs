//! Pixel residency is independent of frame-slot draw-step descriptor caches.

use super::retained_upload::StaticStreamToken;
use super::*;
use std::{hash::Hasher, time::Duration};

/// Hashes the ordered path draw plan separately from packed vertex bytes. Two
/// frames may upload identical geometry but draw a different subset/order of
/// path batches, so the source token alone cannot prove the mask is resident.
/// All supported steps use the same raster pipeline and current slot's resource
/// set. Slot resource IDs are deliberately not part of the key: the persistent
/// mask contains pixels, not a pointer to its old source buffer.
pub(super) fn ordered_draw_plan_token(
    steps: &[DrawStepDescriptor],
    pipeline: RenderPipelineId,
    resource_set: ResourceSetId,
    total_vertices: u32,
    packed_vertex_bytes: usize,
) -> Option<u64> {
    if steps.is_empty()
        || usize::try_from(total_vertices)
            .ok()?
            .checked_mul(PACKED_PATH_RASTERIZATION_VERTEX_BYTES)?
            != packed_vertex_bytes
    {
        return None;
    }
    let mut hasher = collections::FxHasher::default();
    hasher.write_usize(steps.len());
    for step in steps {
        if step.pipeline != pipeline
            || step.resource_sets.as_slice() != [resource_set]
            || step.scissor.is_some()
            || step.instance_count != 1
            || step.first_instance != 0
            || step.vertex_count == 0
            || step.first_vertex.checked_add(step.vertex_count)? > total_vertices
        {
            return None;
        }
        hasher.write_u32(step.first_vertex);
        hasher.write_u32(step.vertex_count);
    }
    Some(hasher.finish())
}


#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Key {
    pub(super) content: Option<StaticStreamToken>,
    /// Exact draw-subset/order fingerprint, independent of rotating frame slots.
    pub(super) draw_plan: Option<u64>,
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
        if key.content.is_some() && key.draw_plan.is_some() && self.resident == Some(key) {
            return false;
        }
        // Clear can change the texture before a later pass or presentation fails.
        self.invalidate();
        true
    }

    pub(super) fn commit(&mut self, key: Key) {
        self.resident = (key.content.is_some() && key.draw_plan.is_some()).then_some(key);
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
