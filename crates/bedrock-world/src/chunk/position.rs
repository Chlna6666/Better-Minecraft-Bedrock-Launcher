//! Minecraft Bedrock world, chunk and block coordinates.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Bedrock dimension identifier.
pub enum Dimension {
    /// The overworld dimension, encoded as `0`.
    Overworld,
    /// The Nether dimension, encoded as `1`.
    Nether,
    /// The End dimension, encoded as `2`.
    End,
    /// A dimension id not recognized by this crate.
    Unknown(i32),
}

impl Dimension {
    #[must_use]
    /// Returns the numeric Bedrock dimension id.
    pub const fn id(self) -> i32 {
        match self {
            Self::Overworld => 0,
            Self::Nether => 1,
            Self::End => 2,
            Self::Unknown(value) => value,
        }
    }

    #[must_use]
    /// Decodes a numeric Bedrock dimension id.
    pub const fn from_id(id: i32) -> Self {
        match id {
            0 => Self::Overworld,
            1 => Self::Nether,
            2 => Self::End,
            value => Self::Unknown(value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Vertical build-height generation used for chunk bounds.
pub enum ChunkVersion {
    /// Pre-Caves-and-Cliffs vertical range.
    Old,
    /// Modern extended vertical range.
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
/// Absolute block position within a world.
pub struct BlockPos {
    /// Absolute X block coordinate.
    pub x: i32,
    /// Absolute Y block coordinate.
    pub y: i32,
    /// Absolute Z block coordinate.
    pub z: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
/// Chunk position and dimension.
pub struct ChunkPos {
    /// Chunk X coordinate.
    pub x: i32,
    /// Chunk Z coordinate.
    pub z: i32,
    /// Dimension containing this chunk.
    pub dimension: Dimension,
}

impl ChunkPos {
    #[must_use]
    /// Returns the inclusive block Y range for this chunk and version.
    pub const fn y_range(self, version: ChunkVersion) -> (i32, i32) {
        match self.dimension {
            Dimension::Nether => (0, 127),
            Dimension::End => (0, 255),
            Dimension::Overworld => match version {
                ChunkVersion::Old => (0, 255),
                ChunkVersion::New => (-64, 319),
            },
            Dimension::Unknown(_) => (0, -1),
        }
    }

    #[must_use]
    /// Returns the inclusive subchunk Y-index range for this chunk and version.
    pub const fn subchunk_index_range(self, version: ChunkVersion) -> (i8, i8) {
        match self.dimension {
            Dimension::Nether => (0, 7),
            Dimension::End => (0, 15),
            Dimension::Overworld => match version {
                ChunkVersion::Old => (0, 15),
                ChunkVersion::New => (-4, 19),
            },
            Dimension::Unknown(_) => (0, -1),
        }
    }

    #[must_use]
    /// Returns the minimum block position covered by this chunk.
    pub const fn min_block_pos(self, version: ChunkVersion) -> BlockPos {
        let (min_y, _) = self.y_range(version);
        BlockPos {
            x: self.x * 16,
            y: min_y,
            z: self.z * 16,
        }
    }

    #[must_use]
    /// Returns the maximum block position covered by this chunk.
    pub const fn max_block_pos(self, version: ChunkVersion) -> BlockPos {
        let (_, max_y) = self.y_range(version);
        BlockPos {
            x: self.x * 16 + 15,
            y: max_y,
            z: self.z * 16 + 15,
        }
    }
}

impl BlockPos {
    #[must_use]
    /// Converts this block position to a chunk position in the given dimension.
    pub const fn to_chunk_pos(self, dimension: Dimension) -> ChunkPos {
        let x = if self.x < 0 { self.x - 15 } else { self.x } / 16;
        let z = if self.z < 0 { self.z - 15 } else { self.z } / 16;
        ChunkPos { x, z, dimension }
    }

    #[must_use]
    /// Returns local chunk X/Z offsets and the absolute Y coordinate.
    pub const fn in_chunk_offset(self) -> (u8, i32, u8) {
        let mut x = self.x % 16;
        let mut z = self.z % 16;
        if x < 0 {
            x += 16;
        }
        if z < 0 {
            z += 16;
        }
        (x as u8, self.y, z as u8)
    }
}
