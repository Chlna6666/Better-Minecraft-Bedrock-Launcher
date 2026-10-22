use super::*;
use std::hash::Hasher;
use std::ops::Range;

impl FrameUpload {
    pub(super) fn layer_scene_encoding_is_reusable(scene: &crate::Scene) -> bool {
        scene.shadows.iter().next().is_none()
            && scene.paths.is_empty()
            && scene.underlines.is_empty()
            && scene.polychrome_sprites.is_empty()
            && scene.surfaces.is_empty()
            && scene.backdrop_blurs.is_empty()
            && scene.blurs.is_empty()
            && scene.renderer_extensions.is_empty()
            && scene.animation_values.is_empty()
            && scene.quads.iter().all(|quad| quad.animation_id.is_none())
            && scene
                .monochrome_sprites
                .iter()
                .all(|sprite| sprite.animation_id.is_none())
            && scene.prepared_batches().iter().all(|batch| {
                matches!(
                    batch,
                    crate::PreparedSceneBatch::Quads(_)
                        | crate::PreparedSceneBatch::MonochromeSprites { .. }
                )
            })
    }

    pub(super) fn layer_scene_encoding_is_complete(
        scene: &crate::Scene,
        summary: FrameUploadSummary,
    ) -> bool {
        let visible_quads = scene
            .quads
            .iter()
            .filter(|quad| !super::clip_is_degenerate(&quad.content_mask))
            .count();
        let visible_monochrome_sprites = scene
            .monochrome_sprites
            .iter()
            .filter(|sprite| !super::clip_is_degenerate(&sprite.content_mask))
            .count();
        summary.quad_count as usize == visible_quads
            && summary.mono_sprite_count as usize == visible_monochrome_sprites
    }

    pub(super) fn append_cached_layer_scene(
        &mut self,
        layer_id: &crate::GlobalElementId,
        cached: &CachedLayerSceneEncoding,
        summary: &mut FrameUploadSummary,
    ) {
        let quad_first = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
        let mono_first = (self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES) as u32;
        if let Some(bytes) = &cached.quads {
            let start = self.quads.len();
            self.quads
                .append_shared(Arc::clone(bytes), cached.quad_hash);
            self.resident_quad_spans.push(RetainedResidentSpan {
                id: RetainedChunkId::new(layer_id.clone(), cached.scene_revision),
                range: start..self.quads.len(),
                byte_hash: cached.quad_hash,
            });
        }
        if let Some(bytes) = &cached.mono_sprites {
            let start = self.mono_sprites.len();
            self.mono_sprites
                .append_shared(Arc::clone(bytes), cached.mono_sprite_hash);
            self.resident_monochrome_sprite_spans
                .push(RetainedResidentSpan {
                    id: RetainedChunkId::new(layer_id.clone(), cached.scene_revision),
                    range: start..self.mono_sprites.len(),
                    byte_hash: cached.mono_sprite_hash,
                });
        }
        self.mono_atlas_tiles
            .extend(cached.mono_atlas_tiles.iter().copied());

        for batch in &cached.batches {
            match *batch {
                CachedLayerBatch::SolidQuads {
                    relative_first,
                    count,
                    ..
                } => self.batches.push(UploadedBatch::SolidQuads {
                    first: quad_first.saturating_add(relative_first),
                    count,
                }),
                CachedLayerBatch::Quads {
                    relative_first,
                    count,
                    ..
                } => self.batches.push(UploadedBatch::Quads {
                    first: quad_first.saturating_add(relative_first),
                    count,
                }),
                CachedLayerBatch::MonoSprites {
                    texture_id,
                    relative_first,
                    count,
                    ..
                } => self.batches.push(UploadedBatch::MonoSprites {
                    texture_id,
                    first: mono_first.saturating_add(relative_first),
                    count,
                }),
            }
            self.batch_visual_bounds.push(match *batch {
                CachedLayerBatch::SolidQuads { visual_bounds, .. }
                | CachedLayerBatch::Quads { visual_bounds, .. }
                | CachedLayerBatch::MonoSprites { visual_bounds, .. } => visual_bounds,
            });
        }

        summary.quad_count = summary.quad_count.saturating_add(cached.quad_count);
        summary.mono_sprite_count = summary
            .mono_sprite_count
            .saturating_add(cached.mono_sprite_count);
        summary.retained_chunk_hits = summary.retained_chunk_hits.saturating_add(1);
        summary.retained_chunk_reused_bytes = summary
            .retained_chunk_reused_bytes
            .saturating_add(cached.quads.as_ref().map_or(0, |bytes| bytes.len()))
            .saturating_add(cached.mono_sprites.as_ref().map_or(0, |bytes| bytes.len()));
    }

    pub(super) fn cached_layer_scene_fits(&self, cached: &CachedLayerSceneEncoding) -> bool {
        let quad_count = cached.quad_count as usize;
        let mono_sprite_count = cached.mono_sprite_count as usize;
        let current_quad_count = self.quads.len() / PACKED_QUAD_BYTES;
        let current_mono_sprite_count = self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES;
        cached.quads.as_ref().map_or(0, |bytes| bytes.len())
            == quad_count.saturating_mul(PACKED_QUAD_BYTES)
            && cached.mono_sprites.as_ref().map_or(0, |bytes| bytes.len())
                == mono_sprite_count.saturating_mul(PACKED_MONO_SPRITE_BYTES)
            && cached.mono_atlas_tiles.len() == mono_sprite_count
            && current_quad_count.saturating_add(quad_count) <= MAX_QUADS
            && current_mono_sprite_count.saturating_add(mono_sprite_count) <= MAX_MONO_SPRITES
            && cached.batches.iter().all(|batch| match *batch {
                CachedLayerBatch::SolidQuads {
                    relative_first,
                    count,
                    ..
                }
                | CachedLayerBatch::Quads {
                    relative_first,
                    count,
                    ..
                } => (relative_first as usize).saturating_add(count as usize) <= quad_count,
                CachedLayerBatch::MonoSprites {
                    relative_first,
                    count,
                    ..
                } => (relative_first as usize).saturating_add(count as usize) <= mono_sprite_count,
            })
    }

    pub(super) fn retain_layer_scene_encoding(
        cache: &mut RetainedChunkCache,
        layer_id: &crate::GlobalElementId,
        scene: &Arc<crate::Scene>,
        quad_range: Range<usize>,
        mono_sprite_range: Range<usize>,
        mono_atlas_range: Range<usize>,
        batch_range: Range<usize>,
        upload: &FrameUpload,
        summary: FrameUploadSummary,
    ) {
        let quads = (!quad_range.is_empty()).then(|| {
            let mut bytes = Vec::with_capacity(quad_range.len());
            for (_, segment) in upload.quads.slices(quad_range.clone()) {
                bytes.extend_from_slice(segment);
            }
            Arc::new(bytes)
        });
        let mono_sprites = (!mono_sprite_range.is_empty()).then(|| {
            let mut bytes = Vec::with_capacity(mono_sprite_range.len());
            for (_, segment) in upload.mono_sprites.slices(mono_sprite_range.clone()) {
                bytes.extend_from_slice(segment);
            }
            Arc::new(bytes)
        });
        let quad_base = (quad_range.start / PACKED_QUAD_BYTES) as u32;
        let mono_base = (mono_sprite_range.start / PACKED_MONO_SPRITE_BYTES) as u32;
        let mut batches = Vec::with_capacity(batch_range.len());
        for batch_index in batch_range {
            let Some(batch) = upload.batches.get(batch_index).copied() else {
                continue;
            };
            let visual_bounds = upload
                .batch_visual_bounds
                .get(batch_index)
                .copied()
                .flatten();
            let cached = match batch {
                UploadedBatch::SolidQuads { first, count } => CachedLayerBatch::SolidQuads {
                    relative_first: first.saturating_sub(quad_base),
                    count,
                    visual_bounds,
                },
                UploadedBatch::Quads { first, count } => CachedLayerBatch::Quads {
                    relative_first: first.saturating_sub(quad_base),
                    count,
                    visual_bounds,
                },
                UploadedBatch::MonoSprites {
                    texture_id,
                    first,
                    count,
                } => CachedLayerBatch::MonoSprites {
                    texture_id,
                    relative_first: first.saturating_sub(mono_base),
                    count,
                    visual_bounds,
                },
                _ => continue,
            };
            batches.push(cached);
        }
        let mut quad_hasher = collections::FxHasher::default();
        if let Some(bytes) = &quads {
            quad_hasher.write(bytes);
        }
        let mut mono_hasher = collections::FxHasher::default();
        if let Some(bytes) = &mono_sprites {
            mono_hasher.write(bytes);
        }
        cache.layer_scene_encodings.insert(
            layer_id.clone(),
            CachedLayerSceneEncoding {
                source: Arc::clone(scene),
                scene_revision: scene.revision,
                atlas_placement_generation: cache.atlas_placement_generation,
                quads,
                quad_hash: quad_hasher.finish(),
                mono_sprites,
                mono_sprite_hash: mono_hasher.finish(),
                mono_atlas_tiles: Arc::new(upload.mono_atlas_tiles[mono_atlas_range].to_vec()),
                batches,
                quad_count: summary.quad_count,
                mono_sprite_count: summary.mono_sprite_count,
                last_used_epoch: cache.frame_epoch,
            },
        );
    }
}
