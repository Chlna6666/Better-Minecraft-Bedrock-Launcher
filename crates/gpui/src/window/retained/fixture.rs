use super::RetainedPaint;
use crate::{GlobalElementId, Scene};
use std::ops::{Deref, DerefMut, Range};

#[derive(Default)]
pub(crate) struct PaintFixture {
    pub(crate) paint: Scene,
    pub(crate) retention: RetainedPaint,
}

impl Deref for PaintFixture {
    type Target = Scene;
    fn deref(&self) -> &Scene {
        &self.paint
    }
}
impl DerefMut for PaintFixture {
    fn deref_mut(&mut self) -> &mut Scene {
        &mut self.paint
    }
}
impl PaintFixture {
    pub(crate) fn record(&mut self, id: GlobalElementId, generation: u64, range: Range<usize>) {
        self.retention.record(id, generation, range);
    }
    pub(crate) fn finish(&mut self) {
        self.paint.finish();
        self.retention.finish(&mut self.paint);
    }
    pub(crate) fn replay(&mut self, range: Range<usize>, previous: &Self) {
        let start = self.len();
        let spans = previous
            .retention
            .replay_quads(&previous.paint, range.clone());
        if self.paint.replay(range.clone(), &previous.paint, &spans) {
            self.retention.replay(range, &previous.retention, start);
        }
    }
    pub(crate) fn chunks(&self) -> &[super::PreparedRetainedQuadChunk] {
        self.retention.chunks_for(&self.paint)
    }
}
