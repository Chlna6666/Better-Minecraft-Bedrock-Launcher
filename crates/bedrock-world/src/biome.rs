//! Bedrock biome and height-map records across historical and current chunk generations.

mod legacy;
/// Explicit migration between historical and modern biome storage.
pub mod migration;

pub use crate::chunk::legacy::LegacyBiomeSample;
pub use crate::parsed::{Biome2d, Biome3d, HeightMap2d, ParsedBiomeData, ParsedBiomeStorage};
pub use legacy::Biome2dLegacy;
pub use migration::promote_data2d_to_data3d;
