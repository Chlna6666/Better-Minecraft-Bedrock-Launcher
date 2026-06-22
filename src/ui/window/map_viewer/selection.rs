use bedrock_render::{ChunkPos, Dimension};
use bedrock_world::SlimeChunkBounds;
use gpui::{Pixels, Point, px};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct ChunkSelection {
    pub(super) start: ChunkPos,
    pub(super) end: ChunkPos,
}

impl ChunkSelection {
    pub(super) fn bounds(self) -> SlimeChunkBounds {
        SlimeChunkBounds {
            dimension: self.start.dimension,
            min_chunk_x: self.start.x.min(self.end.x),
            max_chunk_x: self.start.x.max(self.end.x),
            min_chunk_z: self.start.z.min(self.end.z),
            max_chunk_z: self.start.z.max(self.end.z),
        }
    }

    pub(super) fn chunk_count(self) -> usize {
        let bounds = self.bounds();
        let width = i64::from(bounds.max_chunk_x) - i64::from(bounds.min_chunk_x) + 1;
        let depth = i64::from(bounds.max_chunk_z) - i64::from(bounds.min_chunk_z) + 1;
        usize::try_from(width.saturating_mul(depth)).unwrap_or(usize::MAX)
    }

    pub(super) fn chunks(self) -> Vec<ChunkPos> {
        let bounds = self.bounds();
        let capacity = self.chunk_count();
        let mut chunks = Vec::with_capacity(capacity.min(4096));
        for z in bounds.min_chunk_z..=bounds.max_chunk_z {
            for x in bounds.min_chunk_x..=bounds.max_chunk_x {
                chunks.push(ChunkPos {
                    x,
                    z,
                    dimension: bounds.dimension,
                });
            }
        }
        chunks
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RightSelectionDrag {
    pub(super) start_position: Point<Pixels>,
    pub(super) start_chunk: ChunkPos,
    pub(super) current_chunk: ChunkPos,
    pub(super) moved: bool,
}

impl RightSelectionDrag {
    pub(super) fn new(start_position: Point<Pixels>, start_chunk: ChunkPos) -> Self {
        Self {
            start_position,
            start_chunk,
            current_chunk: start_chunk,
            moved: false,
        }
    }

    pub(super) fn selection(self) -> ChunkSelection {
        ChunkSelection {
            start: self.start_chunk,
            end: self.current_chunk,
        }
    }
}

pub(super) fn chunk_from_block(block_x: i32, block_z: i32, dimension: Dimension) -> ChunkPos {
    ChunkPos {
        x: block_x.div_euclid(16),
        z: block_z.div_euclid(16),
        dimension,
    }
}

pub(super) fn right_selection_moved(
    start: Point<Pixels>,
    current: Point<Pixels>,
    threshold: f32,
) -> bool {
    let delta_x = (current.x - start.x) / px(1.0);
    let delta_y = (current.y - start.y) / px(1.0);
    delta_x.hypot(delta_y) > threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn chunk_selection_bounds_normalize_negative_coordinates() {
        let selection = ChunkSelection {
            start: ChunkPos {
                x: 7,
                z: -12,
                dimension: Dimension::Overworld,
            },
            end: ChunkPos {
                x: -4,
                z: 3,
                dimension: Dimension::Overworld,
            },
        };
        let bounds = selection.bounds();

        assert_eq!(bounds.min_chunk_x, -4);
        assert_eq!(bounds.max_chunk_x, 7);
        assert_eq!(bounds.min_chunk_z, -12);
        assert_eq!(bounds.max_chunk_z, 3);
        assert_eq!(bounds.dimension, Dimension::Overworld);
    }

    #[::core::prelude::v1::test]
    fn right_selection_drag_builds_normalized_selection() {
        let mut drag = RightSelectionDrag::new(
            gpui::point(px(10.0), px(10.0)),
            ChunkPos {
                x: 4,
                z: 8,
                dimension: Dimension::Overworld,
            },
        );
        drag.current_chunk = ChunkPos {
            x: 1,
            z: 2,
            dimension: Dimension::Overworld,
        };
        let bounds = drag.selection().bounds();

        assert_eq!(bounds.min_chunk_x, 1);
        assert_eq!(bounds.max_chunk_x, 4);
        assert_eq!(bounds.min_chunk_z, 2);
        assert_eq!(bounds.max_chunk_z, 8);
    }
}
