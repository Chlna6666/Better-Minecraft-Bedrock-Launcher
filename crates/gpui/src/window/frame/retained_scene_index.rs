//! Entity lookup into the authoritative frame fragment log.
//! Links live in the fragments, so each owner needs only three integers;
//! replay never copies a per-owner heap allocation.

use super::*;

pub(in crate::window) struct Entry {
    first: usize,
    last: usize,
    len: usize,
}

struct Segments<'a> {
    fragments: &'a [RetainedSceneSegment],
    next: Option<usize>,
    remaining: usize,
}

impl<'a> Iterator for Segments<'a> {
    type Item = &'a RetainedSceneSegment;

    fn next(&mut self) -> Option<Self::Item> {
        let index = self.next?;
        let segment = &self.fragments[index];
        self.next = segment.next_for_entity.map(|index| index.get() - 1);
        self.remaining -= 1;
        Some(segment)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for Segments<'_> {}

impl Entry {
    pub(in crate::window) fn len(&self) -> usize {
        self.len
    }

    pub(in crate::window) fn segments<'a>(
        &self,
        segments: &'a [RetainedSceneSegment],
    ) -> impl ExactSizeIterator<Item = &'a RetainedSceneSegment> {
        Segments {
            fragments: segments,
            next: Some(self.first),
            remaining: self.len,
        }
    }

    // Only animation invalidation and failed fragment proofs need the union.
    // Do not duplicate it or calculate it on every fragment insertion.
    pub(in crate::window) fn bounds(
        &self,
        segments: &[RetainedSceneSegment],
    ) -> Bounds<ScaledPixels> {
        self.segments(segments)
            .map(|segment| segment.bounds)
            .reduce(|bounds, next| bounds.union(&next))
            .expect("a retained entity has at least one fragment")
    }
}

impl Frame {
    pub(in crate::window) fn push_retained_scene_segment(&mut self, mut segment: RetainedSceneSegment) {
        let index = self.retained_scene_segments.len();
        // A replayed fragment may have an old frame-local link. Only the
        // destination frame is allowed to link it to another fragment.
        segment.next_for_entity = None;
        match self.retained_scene_index.entry(segment.entity_id) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let entry = entry.get_mut();
                self.retained_scene_segments[entry.last].next_for_entity =
                    std::num::NonZeroUsize::new(index + 1);
                entry.last = index;
                entry.len += 1;
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Entry { first: index, last: index, len: 1 });
            }
        }
        self.retained_scene_segments.push(segment);
    }
}

#[cfg(test)]
mod tests;
