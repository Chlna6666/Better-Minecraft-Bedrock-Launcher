use super::*;
use std::hash::Hasher;
use std::ops::Range;

const WORKING_SET_TRIM_MULTIPLIER: usize = 4;
const CHUNK_SCRATCH_MIN_CAPACITY: usize = 16;

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
                let byte_start = self.quads.len();
                let count = self.encode_quad_range(
                    &scene.quads[chunk.quad_range.clone()],
                    is_solid,
                    summary,
                );
                if count == chunk.quad_range.len() as u32 {
                    let byte_end = self.quads.len();
                    self.cache_retained_quad_chunk(chunk, is_solid, byte_start..byte_end, count);
                }
            }
            cursor = chunk.quad_range.end;
        }
        self.encode_quad_range(&scene.quads[cursor..range.end], is_solid, summary);
    }

    fn cache_retained_quad_chunk(
        &mut self,
        chunk: &crate::PreparedRetainedQuadChunk,
        is_solid: bool,
        range: Range<usize>,
        quad_count: u32,
    ) {
        let encoded_bytes = &self.quads[range.clone()];
        let mut hasher = collections::FxHasher::default();
        hasher.write(encoded_bytes);
        let byte_hash = hasher.finish();
        self.resident_quad_spans.push(RetainedResidentSpan {
            id: chunk.id.clone(),
            range,
            byte_hash,
        });

        let byte_target = PACKED_QUAD_BYTES.max(encoded_bytes.len());
        let cached = self
            .retained_quad_chunks
            .entry(chunk.id.clone())
            .or_insert_with(|| PackedRetainedQuadChunk {
                bytes: Vec::with_capacity(encoded_bytes.len()),
                byte_hash,
                quad_count,
                is_solid,
            });
        cached.bytes.clear();
        if cached.bytes.capacity() > byte_target.saturating_mul(WORKING_SET_TRIM_MULTIPLIER) {
            cached.bytes.shrink_to(byte_target);
        }
        cached.bytes.extend_from_slice(encoded_bytes);
        cached.byte_hash = byte_hash;
        cached.quad_count = quad_count;
        cached.is_solid = is_solid;
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
        self.quads.extend_from_slice(&cached.bytes);
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
        let first = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
        let mut count = 0_u32;
        for quad in quads {
            if self.quads.len() / PACKED_QUAD_BYTES >= MAX_QUADS {
                break;
            }
            if clip_is_degenerate(&quad.content_mask) {
                continue;
            }
            let primitive_index = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
            write_quad(&mut self.quads, quad);
            register_scene_animated_primitive(
                self,
                summary,
                quad.animation_id.map(|_| crate::Primitive::Quad(*quad)),
                AnimatedPrimitiveKind::Quad,
                primitive_index,
            );
            count = count.saturating_add(1);
        }
        if count > 0 {
            self.batches.push(if is_solid {
                UploadedBatch::SolidQuads { first, count }
            } else {
                UploadedBatch::Quads { first, count }
            });
            summary.quad_count = summary.quad_count.saturating_add(count);
        }
        count
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
