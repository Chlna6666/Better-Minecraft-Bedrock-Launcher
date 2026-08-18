//! Compatibility and integrity auditing for Minecraft Bedrock worlds.

pub mod compatibility;
pub mod compatibility_scan;
pub mod integrity;

pub use compatibility::{
    ActorStorage, ChunkCapabilities, CompatibilityLevel, WorldCapabilities,
};
pub use compatibility_scan::{
    ChunkCompatibilitySummary, WorldCompatibilityReport, scan_world_compatibility_blocking,
};
pub use integrity::{
    WorldIntegrityIssue, WorldIntegrityIssueKind, WorldIntegrityOptions, WorldIntegrityReport,
    WorldIntegritySeverity, WorldIntegrityStatus, audit_world_integrity_blocking,
};
