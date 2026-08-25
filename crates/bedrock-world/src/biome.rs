//! Minecraft Bedrock `Data2D`, `Data2DLegacy` and `Data3D` biome records.

mod data2d;
mod legacy;
mod quart;

pub use crate::chunk::legacy::LegacyBiomeSample;
pub use crate::parsed::{Biome2d, Biome3d, HeightMap2d, ParsedBiomeData, ParsedBiomeStorage};
pub use data2d::{data2d_to_data3d, data3d_to_data2d};
pub use legacy::Biome2dLegacy;
pub use quart::encode_data3d_quart;
