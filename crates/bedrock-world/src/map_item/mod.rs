//! Bedrock `map_<id>` saved data.

use crate::nbt::NbtTag;
use bytes::Bytes;

pub use crate::chunk::key::MapItemId;

#[derive(Debug, Clone, PartialEq)]
/// Map item saved data with decoded NBT roots and an optional color buffer.
pub struct SavedData {
    /// Validated storage id without the `map_` prefix.
    pub id: MapItemId,
    /// Consecutive NBT roots stored in the map value.
    pub roots: Vec<NbtTag>,
    /// Common map fields extracted from NBT when present.
    pub known_fields: KnownFields,
    /// Decoded map color buffer when width, height, and color bytes are present.
    pub pixels: Option<Pixels>,
    /// Raw value bytes for lossless preservation.
    pub raw: Bytes,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Common map NBT fields recognized by this crate.
pub struct KnownFields {
    /// Dimension id containing the map center.
    pub dimension: Option<i32>,
    /// World X coordinate of the map center.
    pub center_x: Option<i32>,
    /// World Z coordinate of the map center.
    pub center_z: Option<i32>,
    /// Bedrock map scale.
    pub scale: Option<i32>,
    /// Pixel width recorded in NBT.
    pub width: Option<i32>,
    /// Pixel height recorded in NBT.
    pub height: Option<i32>,
    /// Lock state when recorded by the map NBT.
    pub locked: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Raw Bedrock map color buffer.
pub struct Pixels {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Bedrock map color indices in row-major order.
    pub colors: Vec<u8>,
}
