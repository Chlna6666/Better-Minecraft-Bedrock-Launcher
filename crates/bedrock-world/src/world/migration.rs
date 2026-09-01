//! Minecraft Bedrock whole-world upgrade and downgrade entry points.
//!
//! This module is intentionally a thin orchestration layer. Concrete format transformations remain in
//! their domain modules (`chunk`, `biome`, `entity`, `item`, `level.dat`) so old-version support can be
//! completed without creating another compatibility namespace.

use crate::chunk::SubChunkUpgradeReport;
use crate::block::{BlockUpgradeData, VanillaBlockStatePalette};
use crate::error::{BedrockWorldError, Result};
use crate::version::GameVersion;
use crate::world::{World, StorageBackend};

/// One Minecraft Bedrock migration phase that is known but not yet wired into the whole-world entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationGap {
    /// Historical `LegacyTerrain` split/combine is not yet part of the whole-world entry.
    LegacyTerrain,
    /// `Data2D`, `Data2DLegacy` and `Data3D` biome conversion is not yet part of the whole-world entry.
    BiomeRecords,
    /// Inline Entity to `digp`/`actorprefix` actor-storage conversion is not yet part of the entry.
    ActorStorage,
    /// Player inventory item-stack upgrade/downgrade is not yet part of the entry.
    PlayerSavedItems,
    /// `level.dat` game version metadata rewrite is not yet part of the entry.
    LevelDatVersion,
}

/// SubChunk-specific material required by the whole-world Bedrock upgrade entry.
#[derive(Clone, Copy)]
pub struct SubChunkUpgradeOptions<'a> {
    /// Authoritative block-state upgrade data for the requested target game version.
    pub block_upgrade_data: &'a BlockUpgradeData,
    /// Vanilla block-state palette for the requested target game version.
    pub target_palette: &'a VanillaBlockStatePalette,
}

/// Options for upgrading a Minecraft Bedrock world toward a newer game version.
#[derive(Clone)]
pub struct UpgradeOptions<'a> {
    /// Target Minecraft Bedrock game version.
    pub target: GameVersion,
    /// Optional SubChunk upgrade material. When omitted, SubChunk records are not rewritten.
    pub subchunks: Option<SubChunkUpgradeOptions<'a>>,
}

/// Report returned by [`World::upgrade_bedrock_world`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeReport {
    /// Target Minecraft Bedrock game version requested by the caller.
    pub target: GameVersion,
    /// SubChunk upgrade report when the caller supplied SubChunk migration material.
    pub subchunks: Option<SubChunkUpgradeReport>,
    /// Known whole-world migration phases still not connected to this entry point.
    pub incomplete_phases: Vec<MigrationGap>,
}

impl UpgradeReport {
    /// Returns whether this report rewrote any SubChunk records.
    #[must_use]
    pub fn rewrote_subchunks(&self) -> bool {
        self.subchunks
            .as_ref()
            .is_some_and(|report| report.rewritten_records() != 0)
    }
}

/// Options for downgrading a Minecraft Bedrock world toward an older game version.
#[derive(Clone)]
pub struct DowngradeOptions {
    /// Target Minecraft Bedrock game version.
    pub target: GameVersion,
}

/// Report shape reserved for future downgrade implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DowngradeReport {
    /// Target Minecraft Bedrock game version requested by the caller.
    pub target: GameVersion,
    /// Known whole-world migration phases still not connected to this entry point.
    pub incomplete_phases: Vec<MigrationGap>,
}

impl<S> World<S>
where
    S: StorageBackend,
{
    /// Upgrades supported Minecraft Bedrock persisted records toward a newer game version.
    ///
    /// This is the server-facing orchestration entry. It currently wires the implemented SubChunk
    /// upgrade path and returns explicit gaps for the remaining historical record families instead of
    /// silently pretending the whole world has been fully upgraded.
    pub fn upgrade_bedrock_world(
        &self,
        options: UpgradeOptions<'_>,
    ) -> Result<UpgradeReport> {
        let subchunks = options
            .subchunks
            .map(|subchunks| {
                self.upgrade_bedrock_subchunks(
                    options.target.clone(),
                    subchunks.block_upgrade_data,
                    subchunks.target_palette,
                )
            })
            .transpose()?;

        Ok(UpgradeReport {
            target: options.target,
            subchunks,
            incomplete_phases: vec![
                MigrationGap::LegacyTerrain,
                MigrationGap::BiomeRecords,
                MigrationGap::ActorStorage,
                MigrationGap::PlayerSavedItems,
                MigrationGap::LevelDatVersion,
            ],
        })
    }

    /// Downgrades supported Minecraft Bedrock persisted records toward an older game version.
    ///
    /// The downgrade entry exists so server code can depend on a stable API shape, but the concrete
    /// reverse transformations are intentionally not marked as completed yet. Callers receive a typed
    /// unsupported error instead of a partial destructive rewrite.
    pub fn downgrade_bedrock_world(
        &self,
        options: DowngradeOptions,
    ) -> Result<DowngradeReport> {
        let _ = self;
        Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "Minecraft Bedrock world downgrade to {} is not complete; missing phases: {:?}",
            options.target,
            [
                MigrationGap::LegacyTerrain,
                MigrationGap::BiomeRecords,
                MigrationGap::ActorStorage,
                MigrationGap::PlayerSavedItems,
                MigrationGap::LevelDatVersion,
            ]
        )))
    }
}
