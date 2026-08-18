//! Persisted Minecraft Bedrock saved-item format generations.

/// Actual saved-item representation generations used by Minecraft Bedrock world data.
///
/// These names follow the BedrockItemUpgradeSchema data model. The enum describes the persisted item
/// representation itself; it is not a synthetic player or world schema version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SavedItemFormat {
    /// MCPE <= 1.5: numeric TAG_Short `id` plus TAG_Short `Damage` metadata.
    Classic,
    /// MCPE 1.6-1.8: string `Name` plus TAG_Short `Damage`; blockitems are reconstructed from ID+meta.
    Medieval,
    /// MCPE 1.9+: string `Name` plus metadata and optional persisted `Block` BlockState NBT.
    Modern,
}
