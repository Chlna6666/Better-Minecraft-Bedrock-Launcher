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

/// Append one logical fragment position and return its previous tail.
fn append(
    index: &mut FxHashMap<EntityId, Entry>,
    entity: EntityId,
    position: usize,
) -> Option<usize> {
    let entry = index.entry(entity).or_insert(Entry {
        first: position,
        last: position,
        len: 0,
    });
    let previous = (entry.len != 0).then_some(entry.last);
    entry.last = position;
    entry.len += 1;
    previous
}

impl Frame {
    pub(in crate::window) fn push_retained_scene_segment(&mut self, mut segment: RetainedSceneSegment) {
        let index = self.retained_scene_segments.len();
        // A replayed fragment may have an old frame-local link. Only the
        // destination frame is allowed to link it to another fragment.
        segment.next_for_entity = None;
        if let Some(previous) = append(&mut self.retained_scene_index, segment.entity_id, index) {
            self.retained_scene_segments[previous].next_for_entity =
                std::num::NonZeroUsize::new(index + 1);
        }
        self.retained_scene_segments.push(segment);
    }
}

/// Prepared structural fragment adoption. Scene operation coordinates are
/// certified separately from fragment table positions. Enclosing records
/// crossing an edited operation range still need an explicit owner proof.
#[allow(dead_code)]
pub(in crate::window) struct SegmentSplice<'a> {
    owners: RecordOwnerSplice<'a, RetainedSceneSegment>,
    coordinates: Vec<(usize, Range<usize>)>,
    links: Vec<(usize, Option<std::num::NonZeroUsize>)>,
    affected: FxHashSet<EntityId>,
    index: FxHashMap<EntityId, Entry>,
}

#[allow(dead_code)]
impl<'a> SegmentSplice<'a> {
    pub(in crate::window) fn prepare(
        old: &mut Frame,
        replacement: &Frame,
        fragments: &'a RecordSplicePlan,
        scene: &RecordSplicePlan,
    ) -> Option<Self> {
        let owners = fragments.prepare_owners(
            &old.retained_scene_segments,
            &replacement.retained_scene_segments,
        )?;
        let mut coordinates = Vec::new();
        let mut affected = FxHashSet::default();
        for range in fragments.edited_ranges(false) {
            affected.extend(old.retained_scene_segments[range].iter().map(|segment| segment.entity_id));
        }
        let mut position = 0;
        let mut valid = true;
        fragments.visit_ranges(|source, range| {
            if !valid {
                return;
            }
            let (segments, retained) = match source {
                SpliceSource::Retained => (&old.retained_scene_segments, true),
                SpliceSource::Replacement => (&replacement.retained_scene_segments, false),
            };
            for source_index in range {
                let segment = &segments[source_index];
                let mapped = if retained {
                    scene.retained_span(segment.scene_range.clone())
                } else {
                    scene.replacement_span(segment.scene_range.clone())
                };
                let Some(mapped) = mapped else {
                    valid = false;
                    return;
                };
                if mapped != segment.scene_range {
                    coordinates.push((position, mapped));
                }
                // A coordinate-only scene change does not affect the
                // fragment table's entity links. Membership or slot
                // relocation does, including predecessor links.
                if !retained || position != source_index {
                    affected.insert(segment.entity_id);
                }
                position += 1;
            }
        });
        if !valid {
            return None;
        }

        // Only affected entity chains are rebuilt. Unaffected keys and
        // their embedded links stay resident in the committed index.
        let mut index = FxHashMap::default();
        index.try_reserve(affected.len()).ok()?;
        let mut links: Vec<(usize, Option<std::num::NonZeroUsize>)> = Vec::new();
        let mut tails: FxHashMap<EntityId, usize> = FxHashMap::default();
        tails.try_reserve(affected.len()).ok()?;
        let mut position = 0;
        fragments.visit_ranges(|source, range| {
            let segments = match source {
                SpliceSource::Retained => &old.retained_scene_segments,
                SpliceSource::Replacement => &replacement.retained_scene_segments,
            };
            for segment in &segments[range] {
                if affected.contains(&segment.entity_id) {
                    if let Some(previous) = tails.insert(segment.entity_id, links.len()) {
                        links[previous].1 = std::num::NonZeroUsize::new(position + 1);
                    }
                    append(&mut index, segment.entity_id, position);
                    links.push((position, None));
                }
                position += 1;
            }
        });
        let added = index.keys().filter(|entity| !old.retained_scene_index.contains_key(*entity)).count();
        old.retained_scene_index.try_reserve(added).ok()?;
        Some(Self { owners, coordinates, links, affected, index })
    }

    /// Publish records and destination links together. Retired fragments
    /// are returned to the outer transaction instead of dropped here.
    pub(in crate::window) fn adopt(self, old: &mut Frame, replacement: &mut Frame) -> Vec<RetainedSceneSegment> {
        let retired = self.owners.adopt(
            &mut old.retained_scene_segments,
            &mut replacement.retained_scene_segments,
        );
        for (position, range) in self.coordinates {
            old.retained_scene_segments[position].scene_range = range;
        }
        for (position, next) in self.links {
            old.retained_scene_segments[position].next_for_entity = next;
        }
        for entity in self.affected {
            old.retained_scene_index.remove(&entity);
        }
        old.retained_scene_index.extend(self.index);
        replacement.retained_scene_index.clear();
        retired
    }
}

#[cfg(test)]
mod tests;
