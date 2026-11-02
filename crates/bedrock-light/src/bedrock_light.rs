//! CPU voxel lighting independent of map tiles, world storage and GPU presentation.
mod light;
pub use light::{LightCell, LightError, LightVolume};
