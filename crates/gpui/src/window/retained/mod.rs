//! Window-owned Paint retention metadata derived from the authoritative display list.
mod replay;
pub(crate) use replay::{ReplayStructureIndex, retain_revision};

mod chunks;
pub(crate) use chunks::PreparedRetainedQuadChunk;
use chunks::RetainedChunkCandidate;
mod identity;
pub(crate) use identity::RetainedChunkId;
mod fragments;
mod lifecycle;

/// Window-owned retention metadata for one immutable Paint generation.
/// GPU consumers borrow prepared ranges without owning Paint payloads or UI Fragment state.
#[derive(Default)]
pub(crate) struct RetainedPaint {
    retained_chunk_candidates: Vec<RetainedChunkCandidate>,
    prepared_retained_quad_chunks: Vec<PreparedRetainedQuadChunk>,
    finished: bool,
    revision: u64,
    sources: collections::FxHashMap<u64, std::sync::Arc<Vec<PreparedRetainedQuadChunk>>>,
    replay_structure: std::sync::OnceLock<ReplayStructureIndex>,
}

#[cfg(test)]
mod fixture;
#[cfg(test)]
pub(crate) use fixture::PaintFixture;
