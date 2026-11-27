use super::{PreparedChunks, RetainedChunkId, RetainedPaint};
use crate::paint::retention::{MIN_RETAINED_OPERATIONS, RetainedChunkPlanner, RetainedChunkProof};
use crate::{GlobalElementId, Scene};
use std::{ops::Range, sync::Arc};

#[derive(Clone, Debug)]
pub(super) struct RetainedChunkCandidate {
    id: RetainedChunkId,
    scene_range: Range<usize>,
    replayed: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedRetainedQuadChunk {
    pub(crate) id: RetainedChunkId,
    pub(crate) quad_range: Range<usize>,
    pub(crate) paint_range: Range<usize>,
    pub(crate) replayed: bool,
    pub(crate) is_solid: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedRetainedMonochromeSpriteChunk {
    pub(crate) id: RetainedChunkId,
    pub(crate) sprite_range: Range<usize>,
    pub(crate) replayed: bool,
    pub(crate) texture_id: crate::AtlasTextureId,
    pub(crate) sampling: crate::MonochromeSpriteSampling,
}

impl RetainedPaint {
    pub(crate) fn finish(&mut self, scene: &mut Scene) {
        self.finish_with_previous(scene, None, false);
    }

    /// The source Scene must be the publication paired with previous retention.
    /// Whole-Paint equality permits sharing the entire table; changed scenes
    /// may carry individual immutable Quad qualifications instead.
    pub(crate) fn finish_against(
        &mut self,
        scene: &mut Scene,
        previous_scene: &Scene,
        previous: &Self,
        same_paint: bool,
    ) {
        self.finish_with_previous(scene, Some((previous_scene, previous)), same_paint);
    }

    fn finish_with_previous(
        &mut self,
        scene: &mut Scene,
        previous: Option<(&Scene, &Self)>,
        same_paint: bool,
    ) {
        // Preserve one candidate authority and sort it in place. Recording
        // remains cheap append-only during Paint; finished retention permits
        // O(log chunks + locally overlapping chunks) replay lookups.
        self.retained_chunk_candidates
            .sort_unstable_by(|left, right| {
                left.scene_range
                    .start
                    .cmp(&right.scene_range.start)
                    .then_with(|| right.scene_range.end.cmp(&left.scene_range.end))
                    .then_with(|| left.id.depth.cmp(&right.id.depth))
            });
        // Validate the once-per-finish ordering contract here, not on every
        // retained replay. A debug-build full candidate scan per fragment
        // defeats the binary-search fast path with many retained siblings.
        debug_assert!(
            self.retained_chunk_candidates
                .windows(2)
                .all(|pair| pair[0].scene_range.start <= pair[1].scene_range.start)
        );
        let previous = previous.filter(|(source, retention)| {
            retention.finished && source.revision != 0 && source.revision == retention.revision
        });
        if !(same_paint && previous.is_some_and(|(_, old)| self.reuse_prepared_chunks(scene, old))) {
            self.prepare_retained_chunks(scene, previous);
        }
        self.finished = true;
        self.revision = scene.revision;
    }

    /// Whole-Paint equality is not enough on its own: new candidate
    /// ownership or a different split can change exclusive chunk selection.
    /// Reuse the finished proof only when every candidate was carried at
    /// the same position with the same identity.
    fn reuse_prepared_chunks(&mut self, scene: &Scene, previous: &Self) -> bool {
        if !previous.finished
            || scene.revision == 0
            || scene.revision != previous.revision
            || self.retained_chunk_candidates.len() != previous.retained_chunk_candidates.len()
            || !self.retained_chunk_candidates.iter().zip(&previous.retained_chunk_candidates)
                .all(|(current, old)| current.replayed
                    && current.id == old.id
                    && current.scene_range == old.scene_range)
        {
            return false;
        }

        let Some(old) = previous.prepared.as_ref() else {
            self.prepared = None;
            return true;
        };
        if old.all_replayed {
            self.prepared = Some(Arc::clone(old));
        } else {
            // GPU upload consumers distinguish freshly painted chunks from
            // replayed ones. Promote once, then share this immutable table.
            let table = self.prepared_mut();
            table.quads.clone_from(&old.quads);
            for chunk in &mut table.quads {
                chunk.replayed = true;
            }
            table.sprites.clone_from(&old.sprites);
            for chunk in &mut table.sprites {
                chunk.replayed = true;
            }
            table.replay_quad_indices.clone_from(&old.replay_quad_indices);
            table.all_replayed = true;
        }
        true
    }

    /// Borrows ranges for the paired Paint generation only.
    /// Independently captured or unpaired scenes use ordinary drawing.
    pub(crate) fn chunks_for(&self, scene: &Scene) -> &[PreparedRetainedQuadChunk] {
        if self.finished && self.revision == scene.revision {
            self.prepared.as_deref().map_or(&[], |table| table.quads.as_slice())
        } else {
            &[]
        }
    }

    pub(crate) fn monochrome_sprite_chunks_for(
        &self,
        scene: &Scene,
    ) -> &[PreparedRetainedMonochromeSpriteChunk] {
        if self.finished && self.revision == scene.revision {
            self.prepared.as_deref().map_or(&[], |table| table.sprites.as_slice())
        } else {
            &[]
        }
    }

    /// Select only closed, qualified static spans belonging to the paired source generation.
    pub(crate) fn replay_quads(
        &self,
        scene: &Scene,
        range: Range<usize>,
    ) -> impl Iterator<Item = crate::scene::QuadReplay> + '_ {
        let chunks = self.prepared.as_deref().map_or(&[][..], |table| table.quads.as_slice());
        let indices = self.prepared.as_deref().map_or(&[][..], |table| table.replay_quad_indices.as_slice());
        let (first, end) = if self.finished && self.revision == scene.revision {
            (
                indices
                    .partition_point(|index| chunks[*index].paint_range.start < range.start),
                indices
                    .partition_point(|index| chunks[*index].paint_range.start < range.end),
            )
        } else {
            (0, 0)
        };
        let source_revision = scene.revision;
        indices[first..end]
            .iter()
            .map(move |index| &chunks[*index])
            .filter(move |chunk| chunk.paint_range.end <= range.end)
            .map(move |chunk| crate::scene::QuadReplay {
                source_revision,
                paint_range: chunk.paint_range.clone(),
                quad_range: chunk.quad_range.clone(),
            })
    }

    /// Carries only candidates fully contained in a draw-order-preserving Paint replay.
    pub(crate) fn replay(&mut self, range: Range<usize>, previous: &Self, replay_start: usize) {
        let candidates = &previous.retained_chunk_candidates;
        // Completed retention sorts its existing candidate storage by
        // paint-operation start. Retained fragment replays only inspect
        // candidates starting inside their own source range rather than
        // linearly scanning all siblings for every replay.
        let candidates = if previous.finished {
            let first = candidates.partition_point(|chunk| chunk.scene_range.start < range.start);
            let end = candidates.partition_point(|chunk| chunk.scene_range.start < range.end);
            &candidates[first..end]
        } else {
            // Unfinished retention has not reached finish() yet and
            // may still have candidates in their original append order.
            candidates.as_slice()
        };
        for chunk in candidates {
            if chunk.scene_range.start < range.start || chunk.scene_range.end > range.end {
                continue;
            }
            let start = replay_start + chunk.scene_range.start - range.start;
            let end = replay_start + chunk.scene_range.end - range.start;
            self.retained_chunk_candidates.push(RetainedChunkCandidate {
                id: chunk.id,
                scene_range: start..end,
                replayed: true,
            });
        }

        self.finished = false;
    }

    pub(crate) fn record(
        &mut self,
        identity: GlobalElementId,
        _generation: u64,
        scene_range: Range<usize>,
    ) {
        if scene_range.len() < MIN_RETAINED_OPERATIONS {
            return;
        }
        self.finished = false;
        self.retained_chunk_candidates.push(RetainedChunkCandidate {
            id: RetainedChunkId::fresh(identity.0.len()),
            scene_range,
            replayed: false,
        });
    }

    /// Reuse a static qualification only for an exactly shared operation
    /// range with the same candidate identity. The destination may have
    /// introduced additional orders, so global coverage is proved again.
    fn shared_quad_proof(
        candidate: &RetainedChunkCandidate,
        scene: &Scene,
        previous: Option<(&Scene, &Self)>,
        planner: &mut RetainedChunkPlanner<'_>,
    ) -> Option<RetainedChunkProof> {
        if !candidate.replayed {
            return None;
        }
        let (source, old) = previous?;
        let table = old.prepared.as_deref()?;
        let range = candidate.scene_range.clone();
        let first = table.replay_quad_indices.partition_point(|index| {
            table.quads[*index].paint_range.start < range.start
        });
        let chunk = table.replay_quad_indices[first..].iter()
            .map(|index| &table.quads[*index])
            .take_while(|chunk| chunk.paint_range.start == range.start)
            .find(|chunk| chunk.id == candidate.id && chunk.paint_range == range)?;
        if !scene.paint_operations.get_range(range.clone())?
            .matches_views(source.paint_operations.get_range(range.clone())?, |_, _| false)
        {
            return None;
        }
        let first = source.quads[chunk.quad_range.start].order;
        let last = source.quads[chunk.quad_range.end - 1].order;
        planner.prove_static_quad(range, first, last, chunk.is_solid)
    }

    fn prepare_retained_chunks(&mut self, scene: &Scene, previous: Option<(&Scene, &Self)>) {
        if self.retained_chunk_candidates.is_empty() {
            self.prepared = None;
            return;
        }
        let table = self.prepared_mut();
        let mut quads = std::mem::take(&mut table.quads);
        quads.clear();
        let mut sprites = std::mem::take(&mut table.sprites);
        sprites.clear();
        // Candidates below the minimum are rejected before recording. A
        // single qualified enclosing run owns its exclusive primitive-order
        // interval, so nested candidates cannot improve the final selection.
        let mut qualified_until = 0usize;
        let mut planner = RetainedChunkPlanner::new(scene, self.retained_chunk_candidates.len());
        for candidate in &self.retained_chunk_candidates {
            if candidate.scene_range.end <= qualified_until {
                continue;
            }
            let proof = Self::shared_quad_proof(candidate, scene, previous, &mut planner)
                .or_else(|| planner.prove(candidate.scene_range.clone()));
            match proof {
                Some(RetainedChunkProof::Quad(proof)) => {
                    qualified_until = candidate.scene_range.end;
                    quads.push(PreparedRetainedQuadChunk {
                    id: candidate.id,
                    quad_range: proof.range,
                    paint_range: candidate.scene_range.clone(),
                    replayed: candidate.replayed,
                    is_solid: proof.is_solid,
                    });
                }
                Some(RetainedChunkProof::MonochromeSprite(proof)) => {
                    qualified_until = candidate.scene_range.end;
                    sprites.push(PreparedRetainedMonochromeSpriteChunk {
                        id: candidate.id,
                        sprite_range: proof.range,
                        replayed: candidate.replayed,
                        texture_id: proof.texture_id,
                        sampling: proof.sampling,
                    });
                }
                None => {}
            }
        }

        quads.sort_unstable_by(|left, right| {
            left.quad_range
                .start
                .cmp(&right.quad_range.start)
                .then_with(|| right.quad_range.end.cmp(&left.quad_range.end))
                .then_with(|| left.id.depth.cmp(&right.id.depth))
        });
        let mut retained_end = 0usize;
        quads.retain(|chunk| {
            if chunk.quad_range.start < retained_end {
                return false;
            }
            retained_end = chunk.quad_range.end;
            true
        });

        sprites.sort_unstable_by(|left, right| {
            left.sprite_range
                .start
                .cmp(&right.sprite_range.start)
                .then_with(|| right.sprite_range.end.cmp(&left.sprite_range.end))
                .then_with(|| left.id.depth.cmp(&right.id.depth))
        });
        retained_end = 0;
        sprites.retain(|chunk| {
            if chunk.sprite_range.start < retained_end {
                return false;
            }
            retained_end = chunk.sprite_range.end;
            true
        });
        let table: &mut PreparedChunks = self.prepared_mut();
        table.all_replayed = quads.iter().all(|chunk| chunk.replayed)
            && sprites.iter().all(|chunk| chunk.replayed);
        table.quads = quads;
        table.sprites = sprites;
        table.replay_quad_indices.clear();
        table.replay_quad_indices.extend(0..table.quads.len());
        table.replay_quad_indices.sort_unstable_by_key(|index| table.quads[*index].paint_range.start);
    }
}

/// A retained candidate is reusable only when its inclusive draw-order
/// interval contains exactly its own primitive count. This rejects external
/// operations, nested captures and duplicate DrawOrder values.

#[cfg(test)]
mod retained_order_coverage_tests {
    use crate::DrawOrder;
    use crate::paint::retention::retained_order_range_is_exclusive;

    #[test]
    fn sorted_coverage_matches_full_outside_operation_scan_with_duplicate_orders() {
        for count in 1..=32 {
            // Duplicates and non-monotonic operation order mirror nested
            // captures and layers whose painter order is not insertion order.
            let original: Vec<DrawOrder> = (0..count)
                .map(|index| ((index * 11 + 3) % 17) as DrawOrder)
                .collect();
            let mut sorted = original.clone();
            sorted.sort_unstable();
            for start in 0..original.len() {
                for end in start + 1..=original.len() {
                    let candidate = &original[start..end];
                    let first = *candidate.iter().min().unwrap();
                    let last = *candidate.iter().max().unwrap();
                    let old = original.iter().enumerate().all(|(index, &order)| {
                        (start..end).contains(&index) || order < first || order > last
                    });
                    let new =
                        retained_order_range_is_exclusive(&sorted, first, last, candidate.len());
                    assert_eq!(new, old, "count={count} candidate={start}..{end}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bounds, Quad, ScaledPixels};
    #[test]
    fn unchanged_paint_reuses_prepared_chunks_only_for_identical_candidates() {
        let bounds = Bounds::new(
            crate::point(ScaledPixels(0.), ScaledPixels(0.)),
            crate::size(ScaledPixels(32.), ScaledPixels(32.)),
        );
        let mut previous = crate::window::retained::PaintFixture::default();
        for _ in 0..32 {
            previous.insert_primitive(Quad {
                bounds, content_mask: crate::ContentMask::new(bounds), ..Quad::default()
            });
        }
        previous.record(GlobalElementId::from_path(&["same".into()]), 1, 0..32);
        previous.finish();
        assert_eq!(previous.chunks().len(), 1);

        let mut current = crate::window::retained::PaintFixture::default();
        current.replay(0..32, &previous);
        assert!(current.paint.finish_against(&previous.paint));
        assert!(current.retention.reuse_prepared_chunks(&current.paint.scene, &previous.retention));
        let prepared = current.retention.prepared.as_deref().unwrap();
        assert_eq!(prepared.quads[0].id, previous.chunks()[0].id);
        assert!(prepared.quads[0].replayed);
        assert_eq!(prepared.replay_quad_indices, previous.retention.prepared.as_deref().unwrap().replay_quad_indices);
        current.retention.finish_against(&mut current.paint.scene, &previous.paint.scene, &previous.retention, true);
        assert_eq!(current.chunks().len(), 1);

        let mut next = crate::window::retained::PaintFixture::default();
        next.replay(0..32, &current);
        assert!(next.paint.finish_against(&current.paint));
        next.retention.finish_against(&mut next.paint.scene, &current.paint.scene, &current.retention, true);
        assert!(Arc::ptr_eq(next.retention.prepared.as_ref().unwrap(), current.retention.prepared.as_ref().unwrap()));
        assert!(!previous.chunks()[0].replayed);
        next.retention.clear();
        assert_eq!(current.chunks().len(), 1);

        // An identical Scene with a different owner or candidate split must
        // still go through normal chunk qualification.
        current.record(GlobalElementId::from_path(&["different".into()]), 1, 0..32);
        assert!(!current.retention.reuse_prepared_chunks(&current.paint.scene, &previous.retention));
        current.retention.finish_against(&mut current.paint.scene, &previous.paint.scene, &previous.retention, true);
        assert_eq!(current.chunks().len(), 1);
    }

    #[test]
    fn shared_quad_qualification_rechecks_destination_order_coverage() {
        let bounds = Bounds::new(
            crate::point(ScaledPixels(0.), ScaledPixels(0.)),
            crate::size(ScaledPixels(32.), ScaledPixels(32.)),
        );
        let quad = Quad { bounds, content_mask: crate::ContentMask::new(bounds), ..Quad::default() };
        let mut previous = crate::window::retained::PaintFixture::default();
        for _ in 0..32 { previous.insert_primitive(quad); }
        previous.record(GlobalElementId::from_path(&["stable".into()]), 1, 0..32);
        previous.insert_primitive(quad);
        previous.finish();
        assert_eq!(previous.chunks().len(), 1);

        let mut current = crate::window::retained::PaintFixture::default();
        current.replay(0..32, &previous);
        current.insert_primitive(Quad { background: crate::Hsla::white().into(), ..quad });
        assert!(!current.paint.finish_against(&previous.paint));
        let mut planner = RetainedChunkPlanner::new(&current.paint.scene, 1);
        let candidate = &current.retention.retained_chunk_candidates[0];
        let proof = RetainedPaint::shared_quad_proof(candidate, &current.paint.scene,
            Some((&previous.paint.scene, &previous.retention)), &mut planner);
        assert!(matches!(proof, Some(RetainedChunkProof::Quad(_))));
        current.retention.finish_against(&mut current.paint.scene, &previous.paint.scene, &previous.retention, false);
        assert_eq!(current.chunks()[0].id, previous.chunks()[0].id);

        // A nonoverlapping new Quad receives order 1, which falls inside
        // the old chunk's order interval: importing its old proof is unsafe.
        let mut conflicting = crate::window::retained::PaintFixture::default();
        conflicting.replay(0..32, &previous);
        let other = Bounds::new(
            crate::point(ScaledPixels(100.), ScaledPixels(100.)),
            crate::size(ScaledPixels(32.), ScaledPixels(32.)),
        );
        conflicting.insert_primitive(Quad { bounds: other, content_mask: crate::ContentMask::new(other), ..quad });
        assert!(!conflicting.paint.finish_against(&previous.paint));
        let mut planner = RetainedChunkPlanner::new(&conflicting.paint.scene, 1);
        assert!(RetainedPaint::shared_quad_proof(&conflicting.retention.retained_chunk_candidates[0],
            &conflicting.paint.scene, Some((&previous.paint.scene, &previous.retention)), &mut planner).is_none());
        conflicting.retention.finish_against(&mut conflicting.paint.scene, &previous.paint.scene, &previous.retention, false);
        assert!(conflicting.chunks().is_empty());
    }

    #[test]
    fn retained_fragment_replay_copies_only_contained_sorted_chunks() {
        let bounds = Bounds::new(
            crate::point(ScaledPixels(0.0), ScaledPixels(0.0)),
            crate::size(ScaledPixels(32.0), ScaledPixels(32.0)),
        );
        let mut previous = crate::window::retained::PaintFixture::default();
        let mut ranges = Vec::new();
        // Post-order parent ownership overlaps all child ranges; the parent
        // candidate must not migrate when only one half is replayed.
        for index in 0_u32..16 {
            let start = previous.len();
            for _ in 0..32 {
                previous.insert_primitive(Quad {
                    bounds,
                    content_mask: crate::ContentMask::new(bounds),
                    ..Quad::default()
                });
            }
            let end = previous.len();
            previous.record(
                GlobalElementId::from_path(&[("child", index).into()]),
                7,
                start..end,
            );
            ranges.push(start..end);
        }
        previous.record(
            GlobalElementId::from_path(&["parent".into()]),
            7,
            0..previous.len(),
        );
        previous.finish();
        assert!(
            previous
                .retention
                .retained_chunk_candidates
                .windows(2)
                .all(|pair| pair[0].scene_range.start <= pair[1].scene_range.start)
        );
        assert_eq!(previous.retention.retained_chunk_candidates.len(), 17);

        let split = ranges[7].end;
        let mut replayed = crate::window::retained::PaintFixture::default();
        replayed.replay(0..split, &previous);
        replayed.replay(split..previous.len(), &previous);
        assert_eq!(replayed.len(), previous.len());
        // The outer candidate crosses both replay calls, so only the sixteen
        // complete child chunks are replayed. No second ID index is stored.
        assert_eq!(replayed.retention.retained_chunk_candidates.len(), 16);
        assert!(
            replayed
                .retention
                .retained_chunk_candidates
                .iter()
                .all(|chunk| chunk.replayed)
        );
        replayed.finish();
        assert_eq!(replayed.chunks().len(), 16);
        assert!(replayed.chunks().iter().all(|chunk| chunk.replayed));

        // A late metadata change makes the source mutable again; the
        // binary-search proof cannot be used until finish() is called.
        previous.record(
            GlobalElementId::from_path(&["late-candidate".into()]),
            7,
            ranges[0].clone(),
        );
        assert!(!previous.retention.finished);

        // The unfinalized source path remains safe even before sorting.
        let mut unfinished = crate::window::retained::PaintFixture::default();
        let child = ranges[0].clone();
        for _ in 0..32 {
            unfinished.insert_primitive(Quad {
                bounds,
                content_mask: crate::ContentMask::new(bounds),
                ..Quad::default()
            });
        }
        unfinished.record(
            GlobalElementId::from_path(&["unfinished".into()]),
            1,
            child.clone(),
        );
        let mut replayed_unfinished = crate::window::retained::PaintFixture::default();
        replayed_unfinished.replay(child, &unfinished);
        assert_eq!(
            replayed_unfinished
                .retention
                .retained_chunk_candidates
                .len(),
            1
        );
    }
}
