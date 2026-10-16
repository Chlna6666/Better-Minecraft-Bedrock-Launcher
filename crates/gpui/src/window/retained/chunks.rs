use super::{RetainedChunkId, RetainedPaint};
use crate::paint::retention::QuadChunkPlanner;
use crate::{GlobalElementId, Scene};
use std::ops::Range;

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

impl RetainedPaint {
    pub(crate) fn finish(&mut self, scene: &mut Scene) {
        // Preserve one candidate authority and sort it in place. Recording
        // remains cheap append-only during Paint; finished retention permits
        // O(log chunks + locally overlapping chunks) replay lookups.
        self.retained_chunk_candidates
            .sort_unstable_by(|left, right| {
                left.scene_range
                    .start
                    .cmp(&right.scene_range.start)
                    .then_with(|| right.scene_range.end.cmp(&left.scene_range.end))
            });
        // Validate the once-per-finish ordering contract here, not on every
        // CachedView replay. A debug-build full candidate scan per fragment
        // defeats the binary-search fast path with many retained siblings.
        debug_assert!(
            self.retained_chunk_candidates
                .windows(2)
                .all(|pair| pair[0].scene_range.start <= pair[1].scene_range.start)
        );
        self.prepare_retained_quad_chunks(scene);
        self.replay_quad_indices.clear();
        self.replay_quad_indices
            .extend(0..self.prepared_retained_quad_chunks.len());
        self.replay_quad_indices.sort_unstable_by_key(|index| {
            self.prepared_retained_quad_chunks[*index].paint_range.start
        });
        scene.quads.compact();

        self.finished = true;
        self.revision = scene.revision;
    }

    /// Transfers prepared metadata for an independently painted Filter source.
    /// Only Paint revisions and Chunk ranges are retained; the source Scene remains the sole payload owner.
    pub(crate) fn include(&mut self, mut source: Self) {
        assert!(source.finished, "source Paint retention must be finalized");
        self.sources.extend(source.sources.drain());
        if !source.prepared_retained_quad_chunks.is_empty() {
            self.sources.insert(
                source.revision,
                std::sync::Arc::new(source.prepared_retained_quad_chunks),
            );
        }
    }

    /// Borrows ranges for this exact Paint generation, including independently captured sources.
    /// An unpaired generation is never allowed to consume stale prepared indices.
    pub(crate) fn chunks_for(&self, scene: &Scene) -> &[PreparedRetainedQuadChunk] {
        if self.finished && self.revision == scene.revision {
            &self.prepared_retained_quad_chunks
        } else {
            self.sources
                .get(&scene.revision)
                .map_or(&[], |chunks| chunks.as_slice())
        }
    }

    /// Select only closed, qualified static spans belonging to the paired source generation.
    pub(crate) fn replay_quads(
        &self,
        scene: &Scene,
        range: Range<usize>,
    ) -> Vec<crate::scene::QuadReplay> {
        if !self.finished || self.revision != scene.revision {
            return Vec::new();
        }
        let chunks = &self.prepared_retained_quad_chunks;
        let first = self
            .replay_quad_indices
            .partition_point(|index| chunks[*index].paint_range.start < range.start);
        let end = self
            .replay_quad_indices
            .partition_point(|index| chunks[*index].paint_range.start < range.end);
        self.replay_quad_indices[first..end]
            .iter()
            .map(|index| &chunks[*index])
            .filter(|chunk| chunk.paint_range.end <= range.end)
            .map(|chunk| crate::scene::QuadReplay {
                paint_range: chunk.paint_range.clone(),
                quad_range: chunk.quad_range.clone(),
            })
            .collect()
    }

    /// Carries only candidates fully contained in a draw-order-preserving Paint replay.
    pub(crate) fn replay(&mut self, range: Range<usize>, previous: &Self, replay_start: usize) {
        self.sources.extend(
            previous
                .sources
                .iter()
                .map(|(revision, chunks)| (*revision, std::sync::Arc::clone(chunks))),
        );
        let candidates = &previous.retained_chunk_candidates;
        // Completed retention sorts its existing candidate storage by
        // paint-operation start. CachedView fragment replays only inspect
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
                id: chunk.id.clone(),
                scene_range: start..end,
                replayed: true,
            });
        }

        self.finished = false;
    }

    pub(crate) fn record(
        &mut self,
        identity: GlobalElementId,
        generation: u64,
        scene_range: Range<usize>,
    ) {
        if scene_range.is_empty() {
            return;
        }
        self.finished = false;
        self.retained_chunk_candidates.push(RetainedChunkCandidate {
            id: RetainedChunkId::new(identity, generation),
            scene_range,
            replayed: false,
        });
    }

    fn prepare_retained_quad_chunks(&mut self, scene: &Scene) {
        let mut prepared = std::mem::take(&mut self.prepared_retained_quad_chunks);
        prepared.clear();

        let mut planner = QuadChunkPlanner::new(scene, self.retained_chunk_candidates.len());
        for candidate in &self.retained_chunk_candidates {
            let Some(proof) = planner.prove(candidate.scene_range.clone()) else {
                continue;
            };
            prepared.push(PreparedRetainedQuadChunk {
                id: candidate.id.clone(),
                quad_range: proof.range,
                paint_range: candidate.scene_range.clone(),
                replayed: candidate.replayed,
                is_solid: proof.is_solid,
            });
        }

        prepared.sort_unstable_by(|left, right| {
            left.quad_range
                .start
                .cmp(&right.quad_range.start)
                .then_with(|| right.quad_range.end.cmp(&left.quad_range.end))
                .then_with(|| left.id.identity.0.len().cmp(&right.id.identity.0.len()))
        });
        let mut retained_end = 0usize;
        prepared.retain(|chunk| {
            if chunk.quad_range.start < retained_end {
                return false;
            }
            retained_end = chunk.quad_range.end;
            true
        });
        self.prepared_retained_quad_chunks = prepared;
    }
}

/// Every admitted candidate operation is a Quad within [first, last].
/// Thus the total number of primitive operations in that inclusive order
/// interval must exactly equal the candidate's Quad count. This detects
/// external operations (including nested blur and duplicate DrawOrder) without
/// losing the original painter-order barrier proof.

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
    fn cached_view_fragment_replay_copies_only_contained_sorted_chunks() {
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
