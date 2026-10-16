use crate::GlobalElementId;

/// Exact structural identity and semantic generation of a reusable Paint Chunk.
/// Full element paths prevent anonymous or repeated paths from aliasing in the GPU upload cache.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RetainedChunkId {
    pub(super) identity: GlobalElementId,
    generation: u64,
}

impl RetainedChunkId {
    pub(crate) fn new(identity: GlobalElementId, generation: u64) -> Self {
        Self {
            identity,
            generation,
        }
    }
}
