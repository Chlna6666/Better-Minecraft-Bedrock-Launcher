//! Window-owned Paint retention metadata derived from the authoritative display list.
mod chunks;
pub(crate) use chunks::PreparedRetainedMonochromeSpriteChunk;
pub(crate) use chunks::PreparedRetainedQuadChunk;
use chunks::RetainedChunkCandidate;
mod identity;
pub(crate) use identity::RetainedChunkId;
mod lifecycle;

/// Window-owned retention metadata for one immutable Paint generation.
/// GPU consumers borrow prepared ranges without owning Paint payloads or UI Fragment state.
#[derive(Default)]
pub(crate) struct RetainedPaint {
    retained_chunk_candidates: Vec<RetainedChunkCandidate>,
    prepared_retained_quad_chunks: Vec<PreparedRetainedQuadChunk>,
    prepared_retained_monochrome_sprite_chunks: Vec<PreparedRetainedMonochromeSpriteChunk>,
    /// Sparse operation-order index into the sole prepared Chunk table.
    replay_quad_indices: Vec<usize>,
    finished: bool,
    revision: u64,
}

#[cfg(test)]
mod fixture;
#[cfg(test)]
pub(crate) use fixture::PaintFixture;
