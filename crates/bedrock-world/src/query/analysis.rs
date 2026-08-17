//! Selection and slime-chunk analysis.

pub use super::implementation::{
    SelectionStats, SlimeChunkBounds, SlimeChunkWindow, SlimeWindowSize,
    is_bedrock_slime_chunk, is_slime_chunk, query_selection_stats_blocking,
    query_slime_chunk_windows,
};
