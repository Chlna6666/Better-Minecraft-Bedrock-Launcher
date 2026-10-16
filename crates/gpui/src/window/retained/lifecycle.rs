use super::RetainedPaint;

impl RetainedPaint {
    /// Resets unique Window scratch metadata after presentation has released its snapshot.
    pub(crate) fn clear(&mut self) {
        self.retained_chunk_candidates.clear();
        self.prepared_retained_quad_chunks.clear();
        self.replay_quad_indices.clear();
        self.finished = false;
        self.revision = 0;
        self.sources.clear();
        self.replay_structure.take();
    }

    pub(crate) fn retained_capacity(&self) -> usize {
        self.retained_chunk_candidates.capacity()
            + self.prepared_retained_quad_chunks.capacity()
            + self.replay_quad_indices.capacity()
            + self.sources.capacity()
            + self
                .replay_structure
                .get()
                .map_or(0, |index| index.retained_capacity())
    }

    /// Adapts empty scratch allocations to the current frame without touching shared snapshots.
    pub(crate) fn trim_for_reuse_against(&mut self, current: &Self) {
        let candidate_floor = current.retained_chunk_candidates.len().max(24);
        let prepared_floor = current.prepared_retained_quad_chunks.len().max(24);
        if self.retained_chunk_candidates.capacity() > candidate_floor.saturating_mul(2) {
            self.retained_chunk_candidates.shrink_to(candidate_floor);
        }
        if self.prepared_retained_quad_chunks.capacity() > prepared_floor.saturating_mul(2) {
            self.prepared_retained_quad_chunks.shrink_to(prepared_floor);
        }
        if self.replay_quad_indices.capacity() > prepared_floor.saturating_mul(2) {
            self.replay_quad_indices.shrink_to(prepared_floor);
        }
        let source_floor = current.sources.len().max(24);
        if self.sources.capacity() > source_floor.saturating_mul(2) {
            self.sources.shrink_to(source_floor);
        }
    }
}
