//! Minecraft Bedrock `Data2D`, `Data2DLegacy` and `Data3D` biome records.

mod data2d;
mod downgrade;
mod legacy;
mod quart;
mod upgrade;
mod world;

pub use crate::chunk::legacy::LegacyBiomeSample;
pub use crate::scan::{Biome2d, Biome3d, HeightMap2d, BiomeData, BiomeStorage};
pub use data2d::{data2d_to_data3d, data3d_to_data2d};
pub use downgrade::BiomeData2dDowngradeReport;
pub use legacy::Biome2dLegacy;
pub use quart::encode_data3d_quart;
pub use upgrade::BiomeData3dUpgradeReport;
