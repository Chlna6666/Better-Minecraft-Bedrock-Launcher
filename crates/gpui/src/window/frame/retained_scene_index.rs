//! Frame-local indices follow segment insertion, replay, swap and scratch clearing together.

use super::*;

pub(in crate::window) struct Entry {
    pub(in crate::window) bounds: Bounds<ScaledPixels>,
    indices: SmallVec<[usize; 1]>,
}

impl Entry {
    pub(in crate::window) fn len(&self) -> usize {
        self.indices.len()
    }

    pub(in crate::window) fn first<'a>(
        &self,
        segments: &'a [RetainedSceneSegment],
    ) -> &'a RetainedSceneSegment {
        &segments[self.indices[0]]
    }

    pub(in crate::window) fn segments<'a>(
        &'a self,
        segments: &'a [RetainedSceneSegment],
    ) -> impl ExactSizeIterator<Item = &'a RetainedSceneSegment> {
        self.indices.iter().map(|index| &segments[*index])
    }

    pub(super) fn spill_capacity(&self) -> usize {
        if self.indices.spilled() {
            self.indices.capacity()
        } else {
            0
        }
    }
}

impl Frame {
    pub(super) fn trim_retained_scene_index(&mut self, floor: usize) {
        self.retained_scene_index
            .shrink_to(floor.max(self.retained_scene_index.len()));
        self.retained_scene_index_spill_capacity = self
            .retained_scene_index
            .values_mut()
            .map(|entry| {
                entry.indices.shrink_to_fit();
                entry.spill_capacity()
            })
            .sum();
    }

    pub(in crate::window) fn push_retained_scene_segment(&mut self, segment: RetainedSceneSegment) {
        let index = self.retained_scene_segments.len();
        let entry = self
            .retained_scene_index
            .entry(segment.entity_id)
            .or_insert_with(|| Entry {
                bounds: segment.bounds,
                indices: SmallVec::new(),
            });
        let previous_spill = entry.spill_capacity();
        entry.bounds = entry.bounds.union(&segment.bounds);
        entry.indices.push(index);
        self.retained_scene_index_spill_capacity += entry.spill_capacity() - previous_spill;
        self.retained_scene_segments.push(segment);
    }
}

#[cfg(test)]
mod tests;
