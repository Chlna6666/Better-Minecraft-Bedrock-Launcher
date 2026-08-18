//! Minecraft Bedrock biome and height-map records across historical and current chunk generations.

mod legacy;
/// Explicit conversion between persisted biome storage generations.
pub mod conversion;

pub use crate::chunk::legacy::LegacyBiomeSample;
pub use crate::parsed::{Biome2d, Biome3d, HeightMap2d, ParsedBiomeData, ParsedBiomeStorage};
pub use conversion::promote_data2d_to_data3d;
pub use legacy::Biome2dLegacy;
