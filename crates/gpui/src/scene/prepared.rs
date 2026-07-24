use std::ops::Range;

use crate::AtlasTextureId;

use super::MonochromeSpriteSampling;

#[derive(Default, Clone, Debug)]
pub(crate) struct PreparedSceneBatches {
    pub(super) batches: Vec<PreparedSceneBatch>,
    pub batch_count: usize,
    pub primitive_count: usize,
    pub retained_capacity: usize,
}

impl PreparedSceneBatches {
    pub fn as_slice(&self) -> &[PreparedSceneBatch] {
        &self.batches
    }

    pub(super) fn clear(&mut self) {
        self.batches.clear();
        self.batch_count = 0;
        self.primitive_count = 0;
        self.retained_capacity = self.batches.capacity();
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
    GpuMeshes3d(PreparedGpuMesh3dPass),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedGpuMesh3dPass {
    pub range: Range<usize>,
}
