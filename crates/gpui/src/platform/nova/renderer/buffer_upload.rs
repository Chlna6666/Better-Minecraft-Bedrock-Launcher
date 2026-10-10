use super::chunk_upload::QuadUploadPlan;
use super::retained_upload::StaticUploadMask;
use super::*;
use crate::diagnostics::performance_metrics::{
    BufferUploadMetrics, record_nova_buffer_upload_metrics, record_nova_buffer_upload_time,
};
use gfx_core::BufferUploadBatch;

#[derive(Clone, Copy)]
pub(super) struct FrameBufferTargets {
    global: BufferId,
    text_raster: BufferId,
    quad: BufferId,
    shadow: BufferId,
    path_rasterization_vertex: BufferId,
    path_sprite: BufferId,
    mono_sprite: BufferId,
    poly_sprite: BufferId,
    underline: BufferId,
    backdrop_blur_pass: BufferId,
    backdrop_blur: BufferId,
}

impl NovaRenderer {
    pub(super) fn frame_buffer_targets(&self) -> FrameBufferTargets {
        FrameBufferTargets {
            global: self.global_buffer,
            text_raster: self.text_raster_buffer,
            quad: self.quad_buffer,
            shadow: self.shadow_buffer,
            path_rasterization_vertex: self.path_rasterization_vertex_buffer,
            path_sprite: self.path_sprite_buffer,
            mono_sprite: self.mono_sprite_buffer,
            poly_sprite: self.poly_sprite_buffer,
            underline: self.underline_buffer,
            backdrop_blur_pass: self.backdrop_blur_pass_buffer,
            backdrop_blur: self.backdrop_blur_buffer,
        }
    }
}

pub(super) struct FrameBufferUpload<'a> {
    pub buffers: FrameBufferTargets,
    pub source: &'a FrameUpload,
    pub has_backdrop_blurs: bool,
    pub static_uploads: StaticUploadMask,
    pub quad_upload_plan: &'a QuadUploadPlan,
}

pub(super) fn upload_frame_buffers<D: BackendResources>(
    device: &mut D,
    upload: FrameBufferUpload<'_>,
) -> Result<()> {
    let plan_started = Instant::now(); // CPU dirty-range planning, not visual sampling.
    let mut batch = BufferUploadBatch::default();
    plan_static_buffers(&mut batch, &upload)?;
    plan_quad_buffer(
        &mut batch,
        upload.buffers.quad,
        &upload.source.quads,
        upload.quad_upload_plan,
    )?;
    plan_animated_buffers(&mut batch, &upload)?;
    record_nova_buffer_upload_time(plan_started.elapsed());
    upload_buffer_batch(device, &batch)
}

pub(super) fn upload_buffer_batch<D: BackendResources>(
    device: &mut D,
    batch: &BufferUploadBatch<'_>,
) -> Result<()> {
    let started_at = Instant::now(); // CPU upload profiling, not visual animation sampling.
    let writes = batch.writes().len() as u64;
    let bytes = batch.writes().map(|write| write.data.len() as u64).sum();
    let stats = gfx_core::ResourceDevice::write_buffer_batch(device, batch.writes())?;
    record_nova_buffer_upload_metrics(BufferUploadMetrics {
        requested_writes: batch.requested_writes(),
        requested_bytes: batch.requested_bytes(),
        writes,
        bytes,
        backend_calls: stats.calls,
        backend_bytes: stats.bytes,
    });
    record_nova_buffer_upload_time(started_at.elapsed());
    Ok(())
}

fn plan_static_buffers<'a>(
    batch: &mut BufferUploadBatch<'a>,
    upload: &FrameBufferUpload<'a>,
) -> Result<()> {
    let buffers = upload.buffers;
    let source = upload.source;
    let dirty = upload.static_uploads;
    for (buffer, bytes, selected) in [
        (buffers.global, &source.globals, dirty.global),
        (
            buffers.text_raster,
            &source.text_raster_params,
            dirty.text_raster,
        ),
        (buffers.shadow, &source.shadows, dirty.shadow),
        (
            buffers.path_rasterization_vertex,
            &source.path_rasterization_vertices,
            dirty.path_rasterization_vertex,
        ),
        (buffers.path_sprite, &source.path_sprites, dirty.path_sprite),
        (buffers.mono_sprite, &source.mono_sprites, dirty.mono_sprite),
        (buffers.poly_sprite, &source.poly_sprites, dirty.poly_sprite),
        (buffers.underline, &source.underlines, dirty.underline),
        (
            buffers.backdrop_blur_pass,
            &source.backdrop_blur_passes,
            upload.has_backdrop_blurs && dirty.backdrop_blur_pass,
        ),
        (
            buffers.backdrop_blur,
            &source.backdrop_blurs,
            upload.has_backdrop_blurs && dirty.backdrop_blur,
        ),
    ] {
        if selected {
            batch.push(buffer, bytes, 0..bytes.len())?;
        }
    }
    Ok(())
}

fn plan_quad_buffer<'a>(
    batch: &mut BufferUploadBatch<'a>,
    buffer: BufferId,
    source: &'a PackedQuadStream,
    plan: &QuadUploadPlan,
) -> Result<()> {
    match plan {
        QuadUploadPlan::None => {}
        QuadUploadPlan::Full => plan_quad_range(batch, buffer, source, 0..source.len())?,
        QuadUploadPlan::Ranges(ranges) => {
            for range in ranges {
                plan_quad_range(batch, buffer, source, range.clone())?;
            }
        }
    }
    Ok(())
}

fn plan_quad_range<'a>(
    batch: &mut BufferUploadBatch<'a>,
    buffer: BufferId,
    source: &'a PackedQuadStream,
    range: std::ops::Range<usize>,
) -> Result<()> {
    for (offset, bytes) in source.slices(range) {
        batch.push_at(buffer, offset as u64, bytes)?;
    }
    Ok(())
}

fn plan_animated_buffer_kind<'a>(
    batch: &mut BufferUploadBatch<'a>,
    buffer: BufferId,
    source: &'a [u8],
    frame_upload: &FrameUpload,
    kind: AnimatedPrimitiveKind,
) -> Result<()> {
    for primitive in frame_upload
        .animated_primitives
        .iter()
        .filter(|primitive| primitive.kind == kind)
    {
        let start = usize::try_from(primitive.offset()).map_err(|_| {
            anyhow::anyhow!("nova animated buffer offset does not fit usize: kind={kind:?}")
        })?;
        let end = start
            .checked_add(primitive.bytes.len())
            .ok_or_else(|| anyhow::anyhow!("nova animated buffer range overflow: kind={kind:?}"))?;
        batch.push(buffer, source, start..end)?;
    }
    Ok(())
}

fn plan_animated_buffers<'a>(
    batch: &mut BufferUploadBatch<'a>,
    upload: &FrameBufferUpload<'a>,
) -> Result<()> {
    let buffers = upload.buffers;
    let source = upload.source;
    let dirty = upload.static_uploads;
    if source.has_animated_backdrop_blurs() && !dirty.backdrop_blur_pass {
        batch.push(
            buffers.backdrop_blur_pass,
            &source.backdrop_blur_passes,
            0..source.backdrop_blur_passes.len(),
        )?;
    }
    // Full static writes already contain sampled bytes. Clean streams retain their static gaps;
    // the common planner merges only touching animated ranges from the same immutable snapshot.
    if !dirty.quad {
        for primitive in source
            .animated_primitives
            .iter()
            .filter(|primitive| primitive.kind == AnimatedPrimitiveKind::Quad)
        {
            let start = usize::try_from(primitive.offset())?;
            plan_quad_range(
                batch,
                buffers.quad,
                &source.quads,
                start..start + primitive.bytes.len(),
            )?;
        }
    }
    for (buffer, bytes, kind, selected) in [
        (
            buffers.shadow,
            &source.shadows,
            AnimatedPrimitiveKind::Shadow,
            !dirty.shadow,
        ),
        (
            buffers.mono_sprite,
            &source.mono_sprites,
            AnimatedPrimitiveKind::MonochromeSprite,
            !dirty.mono_sprite,
        ),
        (
            buffers.poly_sprite,
            &source.poly_sprites,
            AnimatedPrimitiveKind::PolychromeSprite,
            !dirty.poly_sprite,
        ),
        (
            buffers.backdrop_blur,
            &source.backdrop_blurs,
            AnimatedPrimitiveKind::BackdropBlur,
            !dirty.backdrop_blur,
        ),
    ] {
        if selected {
            plan_animated_buffer_kind(batch, buffer, bytes, source, kind)?;
        }
    }
    Ok(())
}
