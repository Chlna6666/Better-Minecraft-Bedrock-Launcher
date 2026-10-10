use super::*;
use std::hash::Hasher;
use std::ops::Range;

const WORKING_SET_TRIM_MULTIPLIER: usize = 4;
const CHUNK_SCRATCH_MIN_CAPACITY: usize = 16;

/// Splitting every alternating animated/static primitive would increase
/// driver draw-call overhead. Split only when the run contains a substantial
/// static portion and has at most a few state transitions; this lets spatial
/// culling discard stable batches beside a small animation.
fn should_split_static_dynamic_quads(quads: &[crate::Quad]) -> bool {
    const MIN_STATIC_RUN: usize = 8;
    const MAX_TRANSITIONS: usize = 8;
    let mut previous = None;
    let mut transitions = 0usize;
    let mut static_run = 0usize;
    let mut longest_static_run = 0usize;
    for quad in quads {
        if clip_is_degenerate(&quad.content_mask) {
            continue;
        }
        let animated = quad.animation_id.is_some();
        if previous.is_some_and(|old| old != animated) {
            transitions += 1;
            if transitions > MAX_TRANSITIONS {
                return false;
            }
        }
        previous = Some(animated);
        if animated {
            static_run = 0;
        } else {
            static_run += 1;
            longest_static_run = longest_static_run.max(static_run);
        }
    }
    transitions > 0 && longest_static_run >= MIN_STATIC_RUN
}

impl FrameUpload {
    pub(super) fn encode_retained_quads(
        &mut self,
        scene: &crate::Scene,
        range: Range<usize>,
        is_solid: bool,
        summary: &mut FrameUploadSummary,
    ) {
        let mut cursor = range.start;
        for chunk in scene.prepared_retained_quad_chunks() {
            if chunk.quad_range.end <= range.start || chunk.quad_range.start >= range.end {
                continue;
            }
            if chunk.quad_range.start < cursor
                || chunk.quad_range.end > range.end
                || chunk.is_solid != is_solid
            {
                continue;
            }
            self.encode_quad_range(
                &scene.quads[cursor..chunk.quad_range.start],
                is_solid,
                summary,
            );
            if !self.reuse_retained_quad_chunk(chunk, summary) {
                summary.retained_chunk_misses = summary.retained_chunk_misses.saturating_add(1);
                self.encode_retained_quad_chunk(scene, chunk, summary);
            }
            cursor = chunk.quad_range.end;
        }
        self.encode_quad_range(&scene.quads[cursor..range.end], is_solid, summary);
    }

    fn encode_retained_quad_chunk(
        &mut self,
        scene: &crate::Scene,
        chunk: &crate::PreparedRetainedQuadChunk,
        summary: &mut FrameUploadSummary,
    ) {
        let (bytes, count) = self.encode_chunk_bytes(scene, chunk);
        if count == 0 {
            return;
        }
        let mut hasher = collections::FxHasher::default();
        hasher.write(&bytes);
        let byte_hash = hasher.finish();
        let bytes = Arc::new(bytes);
        let start = self.quads.len();
        self.quads.append_shared(Arc::clone(&bytes), byte_hash);
        self.append_quad_batch(
            (start / PACKED_QUAD_BYTES) as u32,
            count,
            chunk.is_solid,
            summary,
        );
        if count == chunk.quad_range.len() as u32 {
            self.resident_quad_spans.push(RetainedResidentSpan {
                id: chunk.id.clone(),
                range: start..self.quads.len(),
                byte_hash,
            });
            self.retained_quad_chunks.insert(
                chunk.id.clone(),
                PackedRetainedQuadChunk {
                    bytes,
                    byte_hash,
                    quad_count: count,
                    is_solid: chunk.is_solid,
                },
            );
        }
    }

    fn encode_chunk_bytes(
        &mut self,
        scene: &crate::Scene,
        chunk: &crate::PreparedRetainedQuadChunk,
    ) -> (Vec<u8>, u32) {
        // Reset cleared the previous frame's segment references. Reuse a uniquely-owned backing
        // for dirty content, but never copy an Arc still held by another occurrence.
        let mut bytes = self
            .retained_quad_chunks
            .remove(&chunk.id)
            .and_then(|cached| Arc::try_unwrap(cached.bytes).ok())
            .unwrap_or_default();
        bytes.clear();
        let available = MAX_QUADS - self.quads.len() / PACKED_QUAD_BYTES;
        let byte_target =
            PACKED_QUAD_BYTES.max(chunk.quad_range.len().min(available) * PACKED_QUAD_BYTES);
        if bytes.capacity() > byte_target.saturating_mul(WORKING_SET_TRIM_MULTIPLIER) {
            bytes.shrink_to(byte_target);
        }
        bytes.reserve(byte_target);
        let mut count = 0_u32;
        for quad in &scene.quads[chunk.quad_range.clone()] {
            if count as usize == available {
                break;
            }
            if !clip_is_degenerate(&quad.content_mask) {
                debug_assert!(
                    quad.animation_id.is_none(),
                    "retained chunks must be static"
                );
                write_quad(&mut bytes, quad);
                count += 1;
            }
        }
        (bytes, count)
    }

    fn reuse_retained_quad_chunk(
        &mut self,
        chunk: &crate::PreparedRetainedQuadChunk,
        summary: &mut FrameUploadSummary,
    ) -> bool {
        if !chunk.replayed {
            return false;
        }
        let Some(cached) = self.retained_quad_chunks.get(&chunk.id) else {
            return false;
        };
        if cached.is_solid != chunk.is_solid
            || self.quads.len() / PACKED_QUAD_BYTES + cached.quad_count as usize > MAX_QUADS
        {
            return false;
        }
        let byte_start = self.quads.len();
        let first = (byte_start / PACKED_QUAD_BYTES) as u32;
        self.quads
            .append_shared(Arc::clone(&cached.bytes), cached.byte_hash);
        self.resident_quad_spans.push(RetainedResidentSpan {
            id: chunk.id.clone(),
            range: byte_start..self.quads.len(),
            byte_hash: cached.byte_hash,
        });
        self.batches.push(if cached.is_solid {
            UploadedBatch::SolidQuads {
                first,
                count: cached.quad_count,
            }
        } else {
            UploadedBatch::Quads {
                first,
                count: cached.quad_count,
            }
        });
        summary.quad_count = summary.quad_count.saturating_add(cached.quad_count);
        summary.retained_chunk_hits = summary.retained_chunk_hits.saturating_add(1);
        summary.retained_chunk_reused_bytes = summary
            .retained_chunk_reused_bytes
            .saturating_add(cached.bytes.len());
        true
    }

    fn encode_quad_range(
        &mut self,
        quads: &[crate::Quad],
        is_solid: bool,
        summary: &mut FrameUploadSummary,
    ) -> u32 {
        let split_static_runs = should_split_static_dynamic_quads(quads);
        let mut batch_first = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
        let mut batch_count = 0_u32;
        let mut total_count = 0_u32;
        let mut batch_animated = None;
        for quad in quads {
            if self.quads.len() / PACKED_QUAD_BYTES >= MAX_QUADS {
                break;
            }
            if clip_is_degenerate(&quad.content_mask) {
                continue;
            }
            let animated = quad.animation_id.is_some();
            let primitive_index = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
            if split_static_runs && batch_animated.is_some_and(|old| old != animated) {
                self.append_quad_batch(batch_first, batch_count, is_solid, summary);
                batch_first = primitive_index;
                batch_count = 0;
            }
            batch_animated = Some(animated);
            self.quads.write(|bytes| write_quad(bytes, quad));
            register_scene_animated_primitive(
                self,
                summary,
                quad.animation_id.map(|_| crate::Primitive::Quad(*quad)),
                AnimatedPrimitiveKind::Quad,
                primitive_index,
            );
            batch_count = batch_count.saturating_add(1);
            total_count = total_count.saturating_add(1);
        }
        if batch_count != 0 {
            self.append_quad_batch(batch_first, batch_count, is_solid, summary);
        }
        total_count
    }

    fn append_quad_batch(
        &mut self,
        first: u32,
        count: u32,
        is_solid: bool,
        summary: &mut FrameUploadSummary,
    ) {
        self.batches.push(if is_solid {
            UploadedBatch::SolidQuads { first, count }
        } else {
            UploadedBatch::Quads { first, count }
        });
        summary.quad_count = summary.quad_count.saturating_add(count);
    }

    pub(super) fn prune_retained_quads(&mut self, scene: &crate::Scene) {
        self.active_retained_chunk_ids_scratch.clear();
        self.active_retained_chunk_ids_scratch.extend(
            scene
                .prepared_retained_quad_chunks()
                .iter()
                .map(|chunk| chunk.id.clone()),
        );
        let active_chunk_count = self.active_retained_chunk_ids_scratch.len();
        {
            let active_chunks = &self.active_retained_chunk_ids_scratch;
            self.retained_quad_chunks
                .retain(|id, _| active_chunks.contains(id));
        }
        self.active_retained_chunk_ids_scratch.clear();
        let scratch_target = CHUNK_SCRATCH_MIN_CAPACITY.max(active_chunk_count);
        if self.active_retained_chunk_ids_scratch.capacity()
            > scratch_target.saturating_mul(WORKING_SET_TRIM_MULTIPLIER)
        {
            self.active_retained_chunk_ids_scratch
                .shrink_to(scratch_target);
        }
    }
}

#[cfg(test)]
mod static_dynamic_batch_tests {
    use super::*;

    #[test]
    fn long_static_runs_near_one_animation_become_separately_cullable_batches() {
        let rect = crate::bounds(
            crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(15.0)),
            crate::size(crate::ScaledPixels(20.0), crate::ScaledPixels(25.0)),
        );
        let plain = crate::Quad {
            bounds: rect,
            content_mask: crate::ContentMask::new(rect),
            ..Default::default()
        };
        let mut animated = plain;
        animated.animation_id = Some(crate::SceneAnimationId(7));
        let mut source = vec![plain; 10];
        source.push(animated);
        source.extend([plain; 10]);
        assert!(should_split_static_dynamic_quads(&source));

        let mut upload = FrameUpload::default();
        let mut summary = FrameUploadSummary::default();
        let count = upload.encode_quad_range(&source, false, &mut summary);
        assert_eq!(count, 21);
        assert_eq!(summary.quad_count, 21);
        assert_eq!(upload.batches.len(), 3);
        assert!(matches!(upload.batches[0], UploadedBatch::Quads { count: 10, .. }));
        assert!(matches!(upload.batches[1], UploadedBatch::Quads { count: 1, .. }));
        assert!(matches!(upload.batches[2], UploadedBatch::Quads { count: 10, .. }));
    }

    #[test]
    fn heavily_interleaved_animations_remain_a_single_gpu_draw_batch() {
        let rect = crate::bounds(
            crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(15.0)),
            crate::size(crate::ScaledPixels(20.0), crate::ScaledPixels(25.0)),
        );
        let plain = crate::Quad {
            bounds: rect,
            content_mask: crate::ContentMask::new(rect),
            ..Default::default()
        };
        let mut animated = plain;
        animated.animation_id = Some(crate::SceneAnimationId(7));
        let source: Vec<_> = (0..32)
            .map(|index| if index % 2 == 0 { plain } else { animated })
            .collect();
        assert!(!should_split_static_dynamic_quads(&source));
        let mut upload = FrameUpload::default();
        let mut summary = FrameUploadSummary::default();
        upload.encode_quad_range(&source, false, &mut summary);
        assert_eq!(upload.batches.len(), 1);
    }
}
