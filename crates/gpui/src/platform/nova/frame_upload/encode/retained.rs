#[cfg(test)]
mod cache_tests;

use super::*;
use std::hash::Hasher;
use std::ops::Range;

const WORKING_SET_TRIM_MULTIPLIER: usize = 4;

/// Do not emit hundreds of native draws when static/animated Quads alternate.
/// The paint stream itself is the only source of truth: measure the split
/// heuristic in the encoding pass, then merge at most nine tentative batches
/// when splitting would be counterproductive.
const MIN_STATIC_QUAD_RUN: usize = 8;
const MAX_QUAD_ANIMATION_TRANSITIONS: usize = 8;

/// The display-list producer sorts and deduplicates retained spans into
/// non-overlapping Quad index ranges. Seek to the first possible overlap
/// instead of rescanning every retained chunk for each prepared Quad batch.
/// Keep the paint-order range check at the caller: chunks may not cross a
/// prepared batch's solid/non-solid draw-state boundary.
fn retained_chunks_overlapping(
    chunks: &[crate::PreparedRetainedQuadChunk],
    range: Range<usize>,
) -> &[crate::PreparedRetainedQuadChunk] {
    if range.is_empty() {
        return &chunks[..0];
    }
    let start = chunks.partition_point(|chunk| chunk.quad_range.end <= range.start);
    let end = start + chunks[start..].partition_point(|chunk| chunk.quad_range.start < range.end);
    &chunks[start..end]
}

fn retained_monochrome_sprite_chunks_overlapping(
    chunks: &[crate::window::retained::PreparedRetainedMonochromeSpriteChunk],
    range: Range<usize>,
) -> &[crate::window::retained::PreparedRetainedMonochromeSpriteChunk] {
    if range.is_empty() {
        return &chunks[..0];
    }
    let start = chunks.partition_point(|chunk| chunk.sprite_range.end <= range.start);
    let end = start + chunks[start..].partition_point(|chunk| chunk.sprite_range.start < range.end);
    &chunks[start..end]
}

fn retained_monochrome_sprite_bounds(
    sprites: &[crate::MonochromeSprite],
    atlas_placements: &collections::FxHashMap<crate::TileId, crate::AtlasTile>,
) -> Option<crate::Bounds<crate::ScaledPixels>> {
    let mut visual_bounds = None;
    let mut has_visible_sprite = false;
    for source in sprites {
        if clip_is_degenerate(&source.content_mask) {
            continue;
        }
        let mut sprite = *source;
        if let Some(tile) = atlas_placements.get(&sprite.tile.tile_id) {
            sprite.tile = *tile;
        }
        let next = static_raster_visual_bounds(
            sprite.bounds,
            sprite.content_mask.bounds,
            sprite.animation_id,
            sprite.transformation != crate::TransformationMatrix::unit(),
        );
        if has_visible_sprite {
            merge_batch_bounds(&mut visual_bounds, next);
        } else {
            visual_bounds = next;
            has_visible_sprite = true;
        }
    }
    visual_bounds
}

impl FrameUpload {
    pub(super) fn encode_retained_monochrome_sprites(
        &mut self,
        cache: &mut RetainedChunkCache,
        scene: &crate::Scene,
        retained_chunks: &[crate::window::retained::PreparedRetainedMonochromeSpriteChunk],
        range: Range<usize>,
        texture_id: crate::AtlasTextureId,
        sampling: crate::MonochromeSpriteSampling,
        summary: &mut FrameUploadSummary,
    ) {
        let mut cursor = range.start;
        for chunk in retained_monochrome_sprite_chunks_overlapping(retained_chunks, range.clone()) {
            if chunk.sprite_range.start < cursor
                || chunk.sprite_range.end > range.end
                || chunk.texture_id != texture_id
                || chunk.sampling != sampling
            {
                continue;
            }
            self.encode_monochrome_sprites(
                &scene.monochrome_sprites[cursor..chunk.sprite_range.start],
                summary,
            );
            if !self.reuse_retained_monochrome_sprite_chunk(cache, chunk, summary) {
                summary.retained_chunk_misses = summary.retained_chunk_misses.saturating_add(1);
                self.encode_retained_monochrome_sprite_chunk(cache, scene, chunk, summary);
            }
            cursor = chunk.sprite_range.end;
        }
        self.encode_monochrome_sprites(&scene.monochrome_sprites[cursor..range.end], summary);
    }

    fn encode_retained_monochrome_sprite_chunk(
        &mut self,
        cache: &mut RetainedChunkCache,
        scene: &crate::Scene,
        chunk: &crate::window::retained::PreparedRetainedMonochromeSpriteChunk,
        summary: &mut FrameUploadSummary,
    ) {
        let sprites = &scene.monochrome_sprites[chunk.sprite_range.clone()];
        let visible_count = sprites
            .iter()
            .filter(|sprite| !clip_is_degenerate(&sprite.content_mask))
            .count();
        let available =
            MAX_MONO_SPRITES.saturating_sub(self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES);
        if visible_count > available {
            self.encode_monochrome_sprites(sprites, summary);
            return;
        }
        let byte_start = self.mono_sprites.len();
        let atlas_start = self.mono_atlas_tiles.len();
        let visual_bounds = retained_monochrome_sprite_bounds(sprites, &self.atlas_placements);
        let mut packed_bytes = Vec::with_capacity(visible_count * PACKED_MONO_SPRITE_BYTES);
        let first_sprite = (byte_start / PACKED_MONO_SPRITE_BYTES) as u32;
        let mut sprite_count = 0_u32;
        for source in sprites {
            let Some((sprite, sprite_bounds)) = self.prepare_monochrome_sprite(source) else {
                continue;
            };
            let primitive_index = first_sprite + sprite_count;
            write_monochrome_sprite(&mut packed_bytes, &sprite);
            self.record_monochrome_sprite(&sprite, primitive_index, sprite_bounds, summary);
            sprite_count = sprite_count.saturating_add(1);
        }
        debug_assert_eq!(sprite_count as usize, visible_count);
        if sprite_count == 0 {
            return;
        }
        let bytes = Arc::new(packed_bytes);
        let atlas_tiles = Arc::new(self.mono_atlas_tiles[atlas_start..].to_vec());
        let mut hasher = collections::FxHasher::default();
        hasher.write(&bytes);
        let byte_hash = hasher.finish();
        self.mono_sprites
            .append_shared(Arc::clone(&bytes), byte_hash);
        self.resident_monochrome_sprite_spans
            .push(RetainedResidentSpan {
                id: chunk.id.clone(),
                range: byte_start..byte_start + bytes.len(),
                byte_hash,
            });
        cache.retained_monochrome_sprite_chunks.insert(
            chunk.id.clone(),
            PackedRetainedMonochromeSpriteChunk {
                bytes: Arc::clone(&bytes),
                byte_hash,
                atlas_tiles,
                sprite_count,
                texture_id: chunk.texture_id,
                sampling: chunk.sampling,
                visual_bounds,
                atlas_placement_generation: cache.atlas_placement_generation,
                last_used_epoch: cache.frame_epoch,
            },
        );
    }

    fn reuse_retained_monochrome_sprite_chunk(
        &mut self,
        cache: &mut RetainedChunkCache,
        chunk: &crate::window::retained::PreparedRetainedMonochromeSpriteChunk,
        summary: &mut FrameUploadSummary,
    ) -> bool {
        if !chunk.replayed {
            return false;
        }
        let Some(cached) = cache.retained_monochrome_sprite_chunks.get_mut(&chunk.id) else {
            return false;
        };
        if cached.texture_id != chunk.texture_id
            || cached.sampling != chunk.sampling
            || cached.atlas_placement_generation != cache.atlas_placement_generation
            || cached.atlas_tiles.len() != cached.sprite_count as usize
            || cached.bytes.len() != cached.sprite_count as usize * PACKED_MONO_SPRITE_BYTES
            || self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES + cached.sprite_count as usize
                > MAX_MONO_SPRITES
        {
            return false;
        }
        cached.last_used_epoch = cache.frame_epoch;
        let first = (self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES) as u32;
        let byte_start = self.mono_sprites.len();
        self.mono_sprites
            .append_shared(Arc::clone(&cached.bytes), cached.byte_hash);
        self.resident_monochrome_sprite_spans
            .push(RetainedResidentSpan {
                id: chunk.id.clone(),
                range: byte_start..self.mono_sprites.len(),
                byte_hash: cached.byte_hash,
            });
        self.mono_atlas_tiles.extend_from_slice(&cached.atlas_tiles);
        self.append_monochrome_sprite_batch(
            chunk.texture_id,
            first,
            cached.sprite_count,
            cached.visual_bounds,
        );
        summary.mono_sprite_count = summary
            .mono_sprite_count
            .saturating_add(cached.sprite_count);
        summary.retained_chunk_hits = summary.retained_chunk_hits.saturating_add(1);
        summary.retained_chunk_reused_bytes = summary
            .retained_chunk_reused_bytes
            .saturating_add(cached.bytes.len());
        true
    }

    pub(super) fn encode_retained_quads(
        &mut self,
        cache: &mut RetainedChunkCache,
        scene: &crate::Scene,
        retained_chunks: &[crate::PreparedRetainedQuadChunk],
        range: Range<usize>,
        is_solid: bool,
        summary: &mut FrameUploadSummary,
    ) {
        let mut cursor = range.start;
        for chunk in retained_chunks_overlapping(retained_chunks, range.clone()) {
            if chunk.quad_range.start < cursor
                || chunk.quad_range.end > range.end
                || chunk.is_solid != is_solid
            {
                continue;
            }
            self.encode_quad_range(
                scene.quads.slices(cursor..chunk.quad_range.start).flatten(),
                is_solid,
                summary,
            );
            if !self.reuse_retained_quad_chunk(cache, chunk, summary) {
                summary.retained_chunk_misses = summary.retained_chunk_misses.saturating_add(1);
                self.encode_retained_quad_chunk(cache, scene, chunk, summary);
            }
            cursor = chunk.quad_range.end;
        }
        self.encode_quad_range(
            scene.quads.slices(cursor..range.end).flatten(),
            is_solid,
            summary,
        );
    }

    fn encode_retained_quad_chunk(
        &mut self,
        cache: &mut RetainedChunkCache,
        scene: &crate::Scene,
        chunk: &crate::PreparedRetainedQuadChunk,
        summary: &mut FrameUploadSummary,
    ) {
        let (bytes, count, visual_bounds) = self.encode_chunk_bytes(cache, scene, chunk);
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
            visual_bounds,
        );
        summary.quad_count = summary.quad_count.saturating_add(count);
        if count == chunk.quad_range.len() as u32 {
            self.resident_quad_spans.push(RetainedResidentSpan {
                id: chunk.id.clone(),
                range: start..self.quads.len(),
                byte_hash,
            });
            cache.quad_payloads.insert(
                chunk.id.clone(),
                PackedQuadChunk {
                    bytes,
                    byte_hash,
                    quad_count: count,
                    is_solid: chunk.is_solid,
                    visual_bounds,
                    last_used_epoch: cache.frame_epoch,
                },
            );
        }
    }

    fn encode_chunk_bytes(
        &mut self,
        cache: &mut RetainedChunkCache,
        scene: &crate::Scene,
        chunk: &crate::PreparedRetainedQuadChunk,
    ) -> (Vec<u8>, u32, Option<crate::Bounds<crate::ScaledPixels>>) {
        // Reset cleared the previous frame's segment references. Reuse a uniquely-owned backing
        // for dirty content, but never copy an Arc still held by another occurrence.
        let mut bytes = cache
            .quad_payloads
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
        let mut visual_bounds = None;
        let mut cullable = true;
        for quad in scene.quads.slices(chunk.quad_range.clone()).flatten() {
            if count as usize == available {
                break;
            }
            if !clip_is_degenerate(&quad.content_mask) {
                debug_assert!(
                    quad.animation_id.is_none(),
                    "retained chunks must be static"
                );
                if cullable && !merge_static_quad_bounds(&mut visual_bounds, quad) {
                    cullable = false;
                    visual_bounds = None;
                }
                write_quad(&mut bytes, quad);
                count += 1;
            }
        }
        (bytes, count, visual_bounds)
    }

    fn reuse_retained_quad_chunk(
        &mut self,
        cache: &mut RetainedChunkCache,
        chunk: &crate::PreparedRetainedQuadChunk,
        summary: &mut FrameUploadSummary,
    ) -> bool {
        if !chunk.replayed {
            return false;
        }
        let Some(cached) = cache.quad_payloads.get_mut(&chunk.id) else {
            return false;
        };
        // Generation identity is a retained semantic proof, but the encoded
        // payload still must describe this exact primitive count. Reject an
        // incorrectly preserved generation instead of drawing stale bytes.
        if cached.is_solid != chunk.is_solid
            || cached.quad_count as usize != chunk.quad_range.len()
            || cached.bytes.len() != chunk.quad_range.len() * PACKED_QUAD_BYTES
            || self.quads.len() / PACKED_QUAD_BYTES + cached.quad_count as usize > MAX_QUADS
        {
            return false;
        }
        // Only a successful accepted replay marks this payload resident in
        // the current frame. Rejected/fully clipped chunks do not survive trim.
        cached.last_used_epoch = cache.frame_epoch;
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
        // The content version and conservative cull proof share the one GPU
        // owner cache record; replay never rescans the original Quad slice.
        self.batch_visual_bounds.push(cached.visual_bounds);
        summary.quad_count = summary.quad_count.saturating_add(cached.quad_count);
        summary.retained_chunk_hits = summary.retained_chunk_hits.saturating_add(1);
        summary.retained_chunk_reused_bytes = summary
            .retained_chunk_reused_bytes
            .saturating_add(cached.bytes.len());
        true
    }

    fn encode_quad_range<'a>(
        &mut self,
        quads: impl IntoIterator<Item = &'a crate::Quad>,
        is_solid: bool,
        summary: &mut FrameUploadSummary,
    ) -> u32 {
        let initial_batch = self.batches.len();
        let initial_quad = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
        let mut batch_first = initial_quad;
        let mut batch_count = 0_u32;
        let mut total_count = 0_u32;
        let mut previous_animated = None;
        let mut transitions = 0usize;
        let mut static_run = 0usize;
        let mut longest_static_run = 0usize;
        let mut batch_visual_bounds = None;
        let mut batch_cullable = true;

        // Pack and classify in the same pass. Split tentatively at the first
        // eight state changes; after the ninth we keep one continuing batch.
        // Invalid and clipped Quads do not enter GPU memory or the heuristic.
        for quad in quads {
            if self.quads.len() / PACKED_QUAD_BYTES >= MAX_QUADS {
                break;
            }
            if clip_is_degenerate(&quad.content_mask) {
                continue;
            }
            let animated = quad.animation_id.is_some();
            let primitive_index = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
            if previous_animated.is_some_and(|old| old != animated) {
                transitions = transitions.saturating_add(1);
                if transitions <= MAX_QUAD_ANIMATION_TRANSITIONS {
                    self.append_quad_batch(batch_first, batch_count, is_solid, batch_visual_bounds);
                    batch_first = primitive_index;
                    batch_count = 0;
                    batch_visual_bounds = None;
                    batch_cullable = true;
                }
            }
            previous_animated = Some(animated);
            if animated {
                static_run = 0;
            } else {
                static_run = static_run.saturating_add(1);
                longest_static_run = longest_static_run.max(static_run);
            }
            if batch_cullable && !merge_static_quad_bounds(&mut batch_visual_bounds, quad) {
                batch_cullable = false;
                batch_visual_bounds = None;
            }
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
            self.append_quad_batch(batch_first, batch_count, is_solid, batch_visual_bounds);
        }
        // Mixed runs are worth splitting only when they contain a substantial
        // static section and no more than eight state changes. Collapse the
        // tentative contiguous batches in-place otherwise; no new Scene,
        // Upload stream, or temporary batch vector is created.
        if transitions > 0
            && (transitions > MAX_QUAD_ANIMATION_TRANSITIONS
                || longest_static_run < MIN_STATIC_QUAD_RUN)
        {
            self.batches.truncate(initial_batch);
            self.batch_visual_bounds.truncate(initial_batch);
            self.append_quad_batch(initial_quad, total_count, is_solid, None);
        }
        // Count exactly the written instances once, independent of how many
        // candidate batches were retained or collapsed.
        summary.quad_count = summary.quad_count.saturating_add(total_count);
        total_count
    }

    fn append_quad_batch(
        &mut self,
        first: u32,
        count: u32,
        is_solid: bool,
        visual_bounds: Option<crate::Bounds<crate::ScaledPixels>>,
    ) {
        self.batches.push(if is_solid {
            UploadedBatch::SolidQuads { first, count }
        } else {
            UploadedBatch::Quads { first, count }
        });
        self.batch_visual_bounds.push(visual_bounds);
    }
}

impl RetainedChunkCache {
    pub(super) fn begin_frame(&mut self) {
        // Zero is reserved as the never-used marker. On wrap, retire all
        // map entries rather than accidentally validating an ancient epoch.
        self.frame_epoch = self.frame_epoch.wrapping_add(1);
        if self.frame_epoch == 0 {
            self.quad_payloads.clear();
            self.retained_monochrome_sprite_chunks.clear();
            self.layer_scene_encodings.clear();
            self.frame_epoch = 1;
        }
    }

    pub(super) fn prune_retained_chunks(&mut self) {
        let epoch = self.frame_epoch;
        self.quad_payloads
            .retain(|_, chunk| chunk.last_used_epoch == epoch);
        self.retained_monochrome_sprite_chunks
            .retain(|_, chunk| chunk.last_used_epoch == epoch);
        self.layer_scene_encodings
            .retain(|_, layer| layer.last_used_epoch == epoch);
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

        let mut upload = FrameUpload::default();
        let mut summary = FrameUploadSummary::default();
        let count = upload.encode_quad_range(&source, false, &mut summary);
        assert_eq!(count, 21);
        assert_eq!(summary.quad_count, 21);
        assert_eq!(upload.quads.len(), 21 * PACKED_QUAD_BYTES);
        assert_eq!(upload.batches.len(), 3);
        assert!(matches!(
            upload.batches[0],
            UploadedBatch::Quads { count: 10, .. }
        ));
        assert!(matches!(
            upload.batches[1],
            UploadedBatch::Quads { count: 1, .. }
        ));
        assert!(matches!(
            upload.batches[2],
            UploadedBatch::Quads { count: 10, .. }
        ));
    }

    #[test]
    fn overlapping_retained_chunks_matches_full_scan_for_sliced_batches() {
        let chunks: Vec<_> = (0..48)
            .map(|index| crate::PreparedRetainedQuadChunk {
                id: RetainedChunkId::new(
                    crate::GlobalElementId::from_path(&[("span", index).into()]),
                    1,
                ),
                quad_range: index * 8..index * 8 + 8,
                paint_range: index * 8..index * 8 + 8,
                replayed: true,
                is_solid: index % 2 == 0,
            })
            .collect();
        // Window retention finalizes sorted, disjoint Chunk ranges after Painter::finish.
        // Compare against the previous full scan, including adjacent boundaries
        // and ranges which cross multiple solid/non-solid prepared batches.
        for start in 0..=(48 * 8) {
            for width in [1, 2, 7, 8, 9, 31, 64] {
                let end = (start + width).min(48 * 8);
                let expected: Vec<_> = chunks
                    .iter()
                    .filter(|chunk| chunk.quad_range.end > start && chunk.quad_range.start < end)
                    .map(|chunk| &chunk.id)
                    .collect();
                let actual: Vec<_> = retained_chunks_overlapping(&chunks, start..end)
                    .iter()
                    .map(|chunk| &chunk.id)
                    .collect();
                assert_eq!(actual, expected, "batch={start}..{end}");
            }
        }
        assert!(retained_chunks_overlapping(&chunks, 16..16).is_empty());
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
        let mut upload = FrameUpload::default();
        let mut summary = FrameUploadSummary::default();
        upload.encode_quad_range(&source, false, &mut summary);
        assert_eq!(upload.batches.len(), 1);
        assert_eq!(upload.batch_visual_bounds.len(), 1);
        assert!(upload.batch_visual_bounds[0].is_none());
        assert_eq!(summary.quad_count, 32);
    }

    #[test]
    fn short_static_runs_merge_without_paint_order_or_bounds_loss() {
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
        let source = [plain, plain, animated, plain, plain];
        let mut upload = FrameUpload::default();
        let mut summary = FrameUploadSummary::default();
        assert_eq!(upload.encode_quad_range(&source, false, &mut summary), 5);
        assert_eq!(summary.quad_count, 5);
        assert_eq!(upload.quads.len(), 5 * PACKED_QUAD_BYTES);
        assert_eq!(upload.batches.len(), 1);
        assert_eq!(upload.batch_visual_bounds, vec![None]);
        assert!(matches!(
            upload.batches[0],
            UploadedBatch::Quads { first: 0, count: 5 }
        ));
    }

    #[test]
    fn split_policy_matches_the_previous_full_scan_for_visible_quads() {
        let rect = crate::bounds(
            crate::point(crate::ScaledPixels(5.0), crate::ScaledPixels(5.0)),
            crate::size(crate::ScaledPixels(8.0), crate::ScaledPixels(8.0)),
        );
        let plain = crate::Quad {
            bounds: rect,
            content_mask: crate::ContentMask::new(rect),
            ..Default::default()
        };
        let mut animated = plain;
        animated.animation_id = Some(crate::SceneAnimationId(11));
        let mut clipped = plain;
        clipped.content_mask = crate::ContentMask::default();
        let mut seed = 0x9e37_79b9_u32;
        for _ in 0..400 {
            let len = (seed as usize % 80) + 1;
            let mut source = Vec::with_capacity(len);
            for _ in 0..len {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                source.push(match seed % 9 {
                    0 | 1 | 2 => animated,
                    3 => clipped,
                    _ => plain,
                });
            }
            let visible: Vec<_> = source
                .iter()
                .filter(|quad| !clip_is_degenerate(&quad.content_mask))
                .collect();
            let transitions = visible
                .windows(2)
                .filter(|pair| pair[0].animation_id.is_some() != pair[1].animation_id.is_some())
                .count();
            let mut longest_static_run = 0usize;
            let mut static_run = 0usize;
            for quad in &visible {
                if quad.animation_id.is_some() {
                    static_run = 0;
                } else {
                    static_run += 1;
                    longest_static_run = longest_static_run.max(static_run);
                }
            }
            let should_split = transitions > 0
                && transitions <= MAX_QUAD_ANIMATION_TRANSITIONS
                && longest_static_run >= MIN_STATIC_QUAD_RUN;
            let mut expected_counts = Vec::new();
            if should_split {
                for quad in &visible {
                    let animated = quad.animation_id.is_some();
                    if expected_counts
                        .last()
                        .is_none_or(|(state, _)| *state != animated)
                    {
                        expected_counts.push((animated, 0u32));
                    }
                    expected_counts.last_mut().unwrap().1 += 1;
                }
            } else if !visible.is_empty() {
                expected_counts.push((
                    visible.iter().any(|q| q.animation_id.is_some()),
                    visible.len() as u32,
                ));
            }
            let mut upload = FrameUpload::default();
            let mut summary = FrameUploadSummary::default();
            let emitted = upload.encode_quad_range(&source, false, &mut summary);
            assert_eq!(emitted as usize, visible.len());
            assert_eq!(summary.quad_count, emitted);
            assert_eq!(upload.quads.len(), emitted as usize * PACKED_QUAD_BYTES);
            assert_eq!(upload.quads.owned_len(), upload.quads.len());
            assert_eq!(upload.batches.len(), expected_counts.len());
            assert_eq!(upload.batch_visual_bounds.len(), expected_counts.len());
            let mut first = 0u32;
            for (index, (animated, count)) in expected_counts.into_iter().enumerate() {
                assert!(matches!(
                    upload.batches[index],
                    UploadedBatch::Quads { first: actual_first, count: actual_count }
                        if actual_first == first && actual_count == count
                ));
                assert_eq!(upload.batch_visual_bounds[index].is_none(), animated);
                first += count;
            }
        }
    }
}

#[cfg(test)]
mod retained_monochrome_sprite_tests {
    use super::*;

    fn sprite(x: f32) -> crate::MonochromeSprite {
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(x), crate::ScaledPixels(4.)),
            crate::size(crate::ScaledPixels(8.), crate::ScaledPixels(8.)),
        );
        crate::MonochromeSprite {
            order: 0,
            pad: crate::MonochromeSpriteSampling::Glyph as u32,
            animation_id: None,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            color: crate::rgb(0xffffff),
            tile: crate::AtlasTile {
                texture_id: crate::AtlasTextureId {
                    index: 0,
                    kind: crate::AtlasTextureKind::Monochrome,
                },
                tile_id: crate::TileId(0),
                padding: 1,
                bounds: crate::bounds(
                    crate::point(crate::DevicePixels(1), crate::DevicePixels(1)),
                    crate::size(crate::DevicePixels(8), crate::DevicePixels(8)),
                ),
            },
            transformation: crate::TransformationMatrix::unit(),
        }
    }

    fn retained_fixture() -> crate::window::retained::PaintFixture {
        let mut fixture = crate::window::retained::PaintFixture::default();
        let start = fixture.len();
        for index in 0..16 {
            fixture.insert_primitive(sprite(index as f32 * 8.));
        }
        let end = fixture.len();
        fixture.record(
            crate::GlobalElementId::from_path(&["retained-glyphs".into()]),
            3,
            start..end,
        );
        fixture.finish();
        fixture
    }

    fn encode_fixture(
        upload: &mut FrameUpload,
        cache: &mut RetainedChunkCache,
        fixture: &crate::window::retained::PaintFixture,
    ) -> FrameUploadSummary {
        upload.encode(
            cache,
            &fixture.paint,
            &fixture.retention,
            &[],
            DrawableSize {
                width: 128,
                height: 64,
            },
            &RenderingParameters::system(),
            false,
            BackdropBlurQuality::Full,
        )
    }

    #[test]
    fn replayed_static_glyph_chunk_reuses_packed_bytes_and_rejects_relocated_atlas() {
        let previous = retained_fixture();
        assert_eq!(
            previous
                .retention
                .monochrome_sprite_chunks_for(&previous.paint.scene)
                .len(),
            1
        );
        let mut cache = RetainedChunkCache::default();
        cache.set_atlas_placement_generation(4);
        let mut upload = FrameUpload::default();
        let first = encode_fixture(&mut upload, &mut cache, &previous);
        assert_eq!(first.mono_sprite_count, 16);
        assert_eq!(first.retained_chunk_hits, 0);
        assert_eq!(first.retained_chunk_misses, 1);
        let first_bytes = upload.mono_sprites.to_vec();
        assert_eq!(upload.resident_monochrome_sprite_spans.len(), 1);
        assert_eq!(
            upload.resident_monochrome_sprite_spans[0].range,
            0..first_bytes.len()
        );
        assert_eq!(upload.mono_sprites.owned_len(), 0);
        assert!(
            upload
                .mono_sprites
                .segments()
                .all(|(_, _, cached_hash)| cached_hash.is_some())
        );
        let first_batch_count = upload.batches.len();

        let mut current = crate::window::retained::PaintFixture::default();
        current.replay(0..previous.len(), &previous);
        current.finish();
        let replayed_chunks = current
            .retention
            .monochrome_sprite_chunks_for(&current.paint.scene);
        assert_eq!(replayed_chunks.len(), 1);
        assert!(replayed_chunks[0].replayed);

        let second = encode_fixture(&mut upload, &mut cache, &current);
        assert_eq!(second.mono_sprite_count, 16);
        assert_eq!(second.retained_chunk_hits, 1);
        assert_eq!(second.retained_chunk_misses, 0);
        assert_eq!(second.retained_chunk_reused_bytes, first_bytes.len());
        assert_eq!(upload.mono_sprites.to_vec(), first_bytes);
        assert_eq!(upload.resident_monochrome_sprite_spans.len(), 1);
        assert_eq!(upload.mono_sprites.owned_len(), 0);
        assert!(
            upload
                .mono_sprites
                .segments()
                .all(|(_, _, cached_hash)| cached_hash.is_some())
        );
        let cached_chunk = cache
            .retained_monochrome_sprite_chunks
            .get(&replayed_chunks[0].id)
            .expect("replayed glyph chunk is cached");
        assert_eq!(
            upload.mono_sprites.shared_bytes().next().unwrap().as_ptr(),
            cached_chunk.bytes.as_ptr()
        );
        assert_eq!(upload.batches.len(), first_batch_count);
        assert!(matches!(
            upload.batches.as_slice(),
            [UploadedBatch::MonoSprites { texture_id, first: 0, count: 16 }]
                if *texture_id == previous.paint.scene.monochrome_sprites[0].tile.texture_id
        ));

        cache.set_atlas_placement_generation(5);
        let relocated = encode_fixture(&mut upload, &mut cache, &current);
        assert_eq!(relocated.mono_sprite_count, 16);
        assert_eq!(relocated.retained_chunk_hits, 0);
        assert_eq!(relocated.retained_chunk_misses, 1);
    }
}
