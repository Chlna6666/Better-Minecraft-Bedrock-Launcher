//! Window-owned Paint retention metadata derived from the authoritative display list.
mod chunks;
pub(crate) use chunks::PreparedRetainedMonochromeSpriteChunk;
pub(crate) use chunks::PreparedRetainedQuadChunk;
use chunks::RetainedChunkCandidate;
use std::sync::Arc;
mod identity;
pub(crate) use identity::RetainedChunkId;
mod lifecycle;

/// Window-owned retention metadata for one immutable Paint generation.
/// GPU consumers borrow prepared ranges without owning Paint payloads or UI Fragment state.
#[derive(Default)]
pub(crate) struct RetainedPaint {
    retained_chunk_candidates: Vec<RetainedChunkCandidate>,
    prepared: Option<Arc<PreparedChunks>>,
    finished: bool,
    revision: u64,
}

/// Immutable qualification results and their operation-order index have a
/// single owner. Consecutive replay-only publications can share the table.
#[derive(Default)]
struct PreparedChunks {
    quads: Vec<PreparedRetainedQuadChunk>,
    sprites: Vec<PreparedRetainedMonochromeSpriteChunk>,
    replay_quad_indices: Vec<usize>,
    all_replayed: bool,
}

impl RetainedPaint {
    fn prepared_mut(&mut self) -> &mut PreparedChunks {
        if self.prepared.as_mut().and_then(Arc::get_mut).is_none() {
            self.prepared = Some(Arc::default());
        }
        Arc::get_mut(self.prepared.as_mut().expect("prepared table")).expect("unique table")
    }
}

#[cfg(test)]
mod fixture;
#[cfg(test)]
pub(crate) use fixture::PaintFixture;
