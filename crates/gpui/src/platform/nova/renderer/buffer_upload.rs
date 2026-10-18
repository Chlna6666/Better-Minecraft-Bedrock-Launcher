use super::chunk_upload::QuadUploadPlan;
use super::retained_upload::StaticUploadMask;
use super::*;
use crate::diagnostics::performance_metrics::{
    BufferUploadMetrics, record_nova_buffer_upload_metrics, record_nova_buffer_upload_time,
};
use gfx_core::BufferUploadBatch;

#[cfg(test)]
thread_local! {
    // Observe successful Quad writes submitted to the backend, rather than encoded byte equality.
    pub(super) static TEST_QUAD_UPLOAD_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

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
    pub has_compositor_descriptors: bool,
    pub static_uploads: StaticUploadMask,
    pub quad_upload_plan: &'a QuadUploadPlan,
}

pub(super) fn upload_frame_buffers<D: BackendResources>(
    device: &mut D,
    upload: FrameBufferUpload<'_>,
) -> Result<()> {
    #[cfg(test)]
    TEST_QUAD_UPLOAD_BYTES.set(0);
    let plan_started = Instant::now(); // CPU dirty-range planning, not visual sampling.
    let mut batch = BufferUploadBatch::default();
    // Static presentation and GPU-only animation frames can have no
    // device-buffer writes at all. Skip the eleven-stream planning walk and
    // its animated-kind scans; keep profiling through upload_buffer_batch.
    if !requires_buffer_upload_planning(
        upload.source,
        upload.static_uploads,
        upload.quad_upload_plan,
    ) {
        record_nova_buffer_upload_time(plan_started.elapsed());
        return upload_buffer_batch(device, &batch);
    }
    plan_static_buffers(&mut batch, &upload)?;
    plan_quad_buffer(
        &mut batch,
        upload.buffers.quad,
        &upload.source.quads,
        upload.quad_upload_plan,
    )?;
    plan_animated_buffers(&mut batch, &upload)?;
    record_nova_buffer_upload_time(plan_started.elapsed());
    upload_buffer_batch(device, &batch)?;
    #[cfg(test)]
    TEST_QUAD_UPLOAD_BYTES.set(batch.writes()
        .filter(|write| write.descriptor.buffer == upload.buffers.quad)
        .map(|write| write.data.len()).sum());
    Ok(())
}

fn requires_buffer_upload_planning(
    source: &FrameUpload,
    static_uploads: StaticUploadMask,
    quad_upload_plan: &QuadUploadPlan,
) -> bool {
    !static_uploads.is_empty()
        || !matches!(quad_upload_plan, QuadUploadPlan::None)
        || !source.animated_primitives.is_empty()
        || source.has_animated_backdrop_blurs()
}

/// Records an empty GPU upload without ever entering the shared backend mutex.
/// Also used by the normal batch API so both paths report identical zero-work
/// accounting; an empty batch has no backend ordering or submission effects.
pub(super) fn record_empty_buffer_batch(batch: &BufferUploadBatch<'_>, started_at: Instant) {
    debug_assert_eq!(batch.writes().len(), 0);
    record_nova_buffer_upload_metrics(BufferUploadMetrics {
        requested_writes: batch.requested_writes(),
        requested_bytes: batch.requested_bytes(),
        writes: 0,
        bytes: 0,
        backend_calls: 0,
        backend_bytes: 0,
    });
    record_nova_buffer_upload_time(started_at.elapsed());
}

pub(super) fn upload_buffer_batch<D: BackendResources>(
    device: &mut D,
    batch: &BufferUploadBatch<'_>,
) -> Result<()> {
    let started_at = Instant::now(); // CPU upload profiling, not visual animation sampling.
    if batch.writes().len() == 0 {
        record_empty_buffer_batch(batch, started_at);
        return Ok(());
    }
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
            upload.has_compositor_descriptors && dirty.backdrop_blur_pass,
        ),
        (
            buffers.backdrop_blur,
            &source.backdrop_blurs,
            upload.has_compositor_descriptors && dirty.backdrop_blur,
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

/// Emit only the animated Quad bytes not already covered by this slot's
/// static upload plan. A change in static animation topology does not prove
/// the retained Quad spans contain every sampled CPU-animated record.
///
/// Static dirty ranges are ordered, disjoint, and use half-open GPU offsets;
/// partition_point avoids repeatedly scanning their unrelated prefix.
/// Non-overlapping animation writes preserve byte-order within each primitive.
fn for_each_uncovered_animated_quad_range(
    static_plan: &QuadUploadPlan,
    animated: std::ops::Range<usize>,
    mut emit: impl FnMut(std::ops::Range<usize>) -> Result<()>,
) -> Result<()> {
    if animated.is_empty() || matches!(static_plan, QuadUploadPlan::Full) {
        return Ok(());
    }
    let QuadUploadPlan::Ranges(static_ranges) = static_plan else {
        return emit(animated);
    };
    let mut cursor = animated.start;
    let from = static_ranges.partition_point(|range| range.end <= cursor);
    for range in &static_ranges[from..] {
        if range.start >= animated.end {
            break;
        }
        if cursor < range.start {
            emit(cursor..range.start.min(animated.end))?;
        }
        cursor = cursor.max(range.end.min(animated.end));
        if cursor >= animated.end {
            return Ok(());
        }
    }
    if cursor < animated.end {
        emit(cursor..animated.end)?;
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
    // The static signature includes animation topology and may be dirty even
    // when a retained-span partial upload omits some CPU-sampled Quad bytes.
    // Union the two upload sets rather than assuming static "dirty" == Full.
    for primitive in source
        .animated_primitives
        .iter()
        .filter(|primitive| primitive.kind == AnimatedPrimitiveKind::Quad)
    {
        let start = usize::try_from(primitive.offset())?;
        let end = start.checked_add(primitive.bytes.len()).ok_or_else(|| {
            anyhow::anyhow!("nova animated quad range overflow at byte {start}")
        })?;
        for_each_uncovered_animated_quad_range(
            upload.quad_upload_plan,
            start..end,
            |range| plan_quad_range(batch, buffers.quad, &source.quads, range),
        )?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn uncovered_ranges(
        static_plan: &QuadUploadPlan,
        animated: std::ops::Range<usize>,
    ) -> Vec<std::ops::Range<usize>> {
        let mut result = Vec::new();
        for_each_uncovered_animated_quad_range(static_plan, animated, |range| {
            result.push(range);
            Ok(())
        }).unwrap();
        result
    }

    #[test]
    fn animated_quad_upload_fills_only_static_plan_gaps() {
        let static_plan = QuadUploadPlan::Ranges(
            smallvec::smallvec![0..32, 64..96, 128..160]
        );
        assert_eq!(uncovered_ranges(&static_plan, 40..88), vec![40..64]);
        assert_eq!(uncovered_ranges(&static_plan, 16..80), vec![32..64]);
        assert_eq!(uncovered_ranges(&static_plan, 32..64), vec![32..64]);
        assert_eq!(
            uncovered_ranges(&static_plan, 24..144),
            vec![32..64, 96..128]
        );
        assert!(uncovered_ranges(&static_plan, 70..84).is_empty());
        assert!(uncovered_ranges(&static_plan, 144..144).is_empty());
    }

    #[test]
    fn quad_full_or_none_plans_do_not_duplicate_or_lose_animation_bytes() {
        assert!(uncovered_ranges(&QuadUploadPlan::Full, 40..88).is_empty());
        assert_eq!(
            uncovered_ranges(&QuadUploadPlan::None, 40..88),
            vec![40..88]
        );
        assert!(uncovered_ranges(&QuadUploadPlan::None, 0..0).is_empty());
    }

    #[test]
    fn animated_quads_across_multiple_resident_islands_keep_byte_order() {
        let static_plan = QuadUploadPlan::Ranges(
            smallvec::smallvec![16..32, 48..64, 80..96, 112..128, 144..160]
        );
        assert_eq!(
            uncovered_ranges(&static_plan, 8..168),
            vec![8..16, 32..48, 64..80, 96..112, 128..144, 160..168]
        );
        assert_eq!(uncovered_ranges(&static_plan, 160..176), vec![160..176]);
        assert_eq!(uncovered_ranges(&static_plan, 0..8), vec![0..8]);
    }

    #[test]
    fn clean_static_and_gpu_only_animation_frames_skip_buffer_planning() {
        let upload = FrameUpload::default();
        assert!(!requires_buffer_upload_planning(
            &upload,
            StaticUploadMask::default(),
            &QuadUploadPlan::None,
        ));
    }

    #[test]
    fn new_or_dirty_slot_cannot_skip_upload_planning() {
        let upload = FrameUpload::default();
        assert!(requires_buffer_upload_planning(
            &upload,
            StaticUploadMask {
                global: true,
                ..Default::default()
            },
            &QuadUploadPlan::None,
        ));
        assert!(requires_buffer_upload_planning(
            &upload,
            StaticUploadMask::default(),
            &QuadUploadPlan::Full,
        ));
        assert!(requires_buffer_upload_planning(
            &upload,
            StaticUploadMask::default(),
            &QuadUploadPlan::Ranges(smallvec::smallvec![0..16]),
        ));
    }

    #[test]
    fn zero_write_upload_batch_preserves_empty_metrics_contract() {
        let batch = BufferUploadBatch::default();
        assert_eq!(batch.writes().len(), 0);
        assert_eq!(batch.requested_writes(), 0);
        assert_eq!(batch.requested_bytes(), 0);
        assert_eq!(gfx_core::BufferUploadStats::default().calls, 0);
        assert_eq!(gfx_core::BufferUploadStats::default().bytes, 0);
    }
}
