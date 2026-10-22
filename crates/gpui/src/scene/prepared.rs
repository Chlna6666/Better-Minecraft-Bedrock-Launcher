use std::{ops::Range, sync::Arc};

use crate::AtlasTextureId;

use super::MonochromeSpriteSampling;

#[derive(Default, Clone, Debug)]
pub(crate) struct PreparedSceneBatches {
    pub(super) batches: Arc<Vec<PreparedSceneBatch>>,
    scratch: Vec<PreparedSceneBatch>,
    pub batch_count: usize,
    pub primitive_count: usize,
    pub retained_capacity: usize,
}

impl PreparedSceneBatches {
    pub fn as_slice(&self) -> &[PreparedSceneBatch] {
        self.batches.as_slice()
    }

    pub(super) fn clear(&mut self) {
        let previous = std::mem::replace(&mut self.batches, Arc::default());
        if let Ok(mut batches) = Arc::try_unwrap(previous) {
            batches.clear();
            if batches.capacity() > self.scratch.capacity() {
                self.scratch = batches;
            }
        }
        self.scratch.clear();
        self.batch_count = 0;
        self.primitive_count = 0;
        self.retained_capacity = self.capacity();
    }

    pub(super) fn take_scratch(&mut self) -> Vec<PreparedSceneBatch> {
        std::mem::take(&mut self.scratch)
    }

    pub(super) fn finish(&mut self, batches: Vec<PreparedSceneBatch>, primitive_count: usize) {
        self.batch_count = batches.len();
        self.primitive_count = primitive_count;
        self.retained_capacity = batches.capacity() + self.scratch.capacity();
        self.batches = Arc::new(batches);
    }

    pub(super) fn share_from(&mut self, previous: &Self) {
        self.batches = Arc::clone(&previous.batches);
        self.batch_count = previous.batch_count;
        self.primitive_count = previous.primitive_count;
        self.retained_capacity = self.capacity();
    }

    pub(super) fn capacity(&self) -> usize {
        self.batches.capacity() + self.scratch.capacity()
    }

    pub(super) fn trim_scratch(&mut self, target: usize) {
        if self.scratch.capacity() > target.saturating_mul(2) {
            self.scratch.shrink_to(target);
        }
        self.retained_capacity = self.capacity();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PreparedSceneBatch {
    Shadows(Range<usize>),
    Quads(PreparedQuadRun),
    Paths(Range<usize>),
    Underlines(Range<usize>),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        sampling: MonochromeSpriteSampling,
        range: Range<usize>,
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    Surfaces(Range<usize>),
    BackdropBlurs(PreparedBackdropBlurGroup),
    Blurs(Range<usize>),
    RendererExtensions(Range<usize>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedQuadRun {
    pub range: Range<usize>,
    pub is_solid: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedBackdropBlurGroup {
    pub range: Range<usize>,
}
