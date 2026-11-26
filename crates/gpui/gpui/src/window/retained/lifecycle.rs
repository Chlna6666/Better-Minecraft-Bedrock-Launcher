use super::RetainedPaint;
use std::sync::Arc;

impl RetainedPaint {
    /// Resets unique Window scratch metadata after presentation has released its snapshot.
    pub(crate) fn clear(&mut self) {
        self.retained_chunk_candidates.clear();
        if let Some(table) = self.prepared.as_mut().and_then(Arc::get_mut) {
            table.quads.clear();
            table.sprites.clear();
            table.replay_quad_indices.clear();
            table.all_replayed = false;
        } else {
            self.prepared = None;
        }
        self.finished = false;
        self.revision = 0;
    }

    pub(crate) fn retained_capacity(&self) -> usize {
        self.retained_chunk_candidates.capacity()
            + self.prepared.as_deref().map_or(0, |table| {
                table.quads.capacity() + table.sprites.capacity() + table.replay_quad_indices.capacity()
            })
    }

    /// Adapts empty scratch allocations to the current frame without touching shared snapshots.
    pub(crate) fn trim_for_reuse(&mut self, current: &Self) {
        let candidate_floor = current.retained_chunk_candidates.len().max(24);
        if self.retained_chunk_candidates.capacity() > candidate_floor.saturating_mul(2) {
            self.retained_chunk_candidates.shrink_to(candidate_floor);
        }
        let Some(table) = self.prepared.as_mut().and_then(Arc::get_mut) else { return; };
        let prepared_floor = current.prepared.as_deref().map_or(0, |table| table.quads.len()).max(24);
        let sprite_floor = current.prepared.as_deref().map_or(0, |table| table.sprites.len()).max(24);
        if table.quads.capacity() > prepared_floor.saturating_mul(2) {
            table.quads.shrink_to(prepared_floor);
        }
        if table.sprites.capacity() > sprite_floor.saturating_mul(2) {
            table.sprites.shrink_to(sprite_floor);
        }
        if table.replay_quad_indices.capacity() > prepared_floor.saturating_mul(2) {
            table.replay_quad_indices.shrink_to(prepared_floor);
        }
    }
}
