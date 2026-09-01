//! Compatibility and integrity auditing for Minecraft Bedrock worlds.

pub mod audit;
pub mod compatibility;
pub mod scan;

pub use audit::{Issue, IssueKind, Options, Report, Severity, Status, audit};
pub use compatibility::{ActorStorage, ChunkCapabilities, CompatibilityLevel, WorldCapabilities};
pub use scan::{ChunkSummary, CompatibilityReport, scan_compatibility};
