//! Whole-world integrity auditing for Bedrock world storage.
//!
//! The auditor is intentionally read-only. It validates storage relationships and parseability
//! without rewriting unknown records, allowing map editors and servers to decide whether a problem
//! is repairable, requires a schema upgrade, or should block writes entirely.

use crate::chunk::{
    BedrockDbKey, ChunkRecordTag, LegacyTerrain, SubChunkDecodeMode, SubChunkFormat,
    parse_subchunk_with_mode,
};
use crate::error::Result;
use crate::level_dat::read_level_dat_document;
use crate::nbt::{NbtTag, parse_consecutive_root_nbt, parse_root_nbt};
use crate::parsed::parse_actor_digest_ids;
use crate::storage::{StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::world::{BedrockWorld, WorldStorageHandle};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Overall integrity classification for a world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldIntegrityStatus {
    /// No known integrity problems were detected.
    Healthy,
    /// Metadata or index relationships can be repaired without semantic block-state migration.
    Repairable,
    /// Historical/unsupported block-state or subchunk data requires an explicit upgrade path.
    LegacyNeedsUpgrade,
    /// The world contains malformed or contradictory records that should block normal writes.
    Corrupt,
}

/// Severity assigned to one audit issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorldIntegritySeverity {
    /// Informational diagnostic.
    Info,
    /// Recoverable or compatibility concern.
    Warning,
    /// Structural corruption or malformed data.
    Error,
}

/// Stable issue category emitted by the integrity auditor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldIntegrityIssueKind {
    /// `level.dat` could not be parsed.
    LevelDatUnreadable,
    /// `level.dat` does not contain a usable `RandomSeed`.
    MissingRandomSeed,
    /// A tolerated `level.dat` header warning was observed.
    LevelDatWarning,
    /// A storage key could not be classified as a supported Bedrock record.
    UnknownStorageKey,
    /// A record that must carry a payload is empty.
    EmptyRecord,
    /// A modern or legacy subchunk payload could not be decoded.
    UnsupportedSubChunk,
    /// A legacy terrain payload has an invalid byte length.
    MalformedLegacyTerrain,
    /// A player/actor/block-entity NBT payload is malformed.
    MalformedNbt,
    /// A `digp` digest references a missing `actorprefix` record.
    DanglingActorDigest,
    /// An `actorprefix` record is not referenced by any scanned digest.
    OrphanActorRecord,
    /// One actor is referenced by more than one chunk digest.
    ActorReferencedByMultipleChunks,
    /// A decoded palette contains an older block-state storage version.
    LegacyBlockState,
    /// A decoded palette block state has no storage version.
    UnknownBlockStateVersion,
}

/// One integrity finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldIntegrityIssue {
    /// Finding severity.
    pub severity: WorldIntegritySeverity,
    /// Stable issue category.
    pub kind: WorldIntegrityIssueKind,
    /// Storage key/category or metadata location associated with the issue.
    pub location: String,
    /// Human-readable diagnostic detail.
    pub detail: String,
}

/// Options controlling a world integrity scan.
#[derive(Debug, Clone)]
pub struct WorldIntegrityOptions {
    /// Raw storage scan settings.
    pub storage: StorageReadOptions,
    /// Validate modern/legacy subchunk payloads and palette state metadata.
    pub validate_subchunks: bool,
    /// Validate NBT payloads for players, actors, and block entities.
    pub validate_nbt: bool,
    /// Validate `digp` ↔ `actorprefix` ownership relationships.
    pub validate_actor_links: bool,
    /// Optional target block-state storage version used to classify historical palettes.
    pub target_block_state_version: Option<i32>,
    /// Maximum number of per-record issues retained in the report.
    pub max_issues: usize,
}

impl Default for WorldIntegrityOptions {
    fn default() -> Self {
        Self {
            storage: StorageReadOptions::default(),
            validate_subchunks: true,
            validate_nbt: true,
            validate_actor_links: true,
            target_block_state_version: None,
            max_issues: 1024,
        }
    }
}

/// Aggregate result of a whole-world integrity audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldIntegrityReport {
    /// Overall world classification.
    pub status: WorldIntegrityStatus,
    /// Raw records visited.
    pub records_scanned: usize,
    /// Chunk-scoped records visited.
    pub chunk_records: usize,
    /// Subchunk records decoded or inspected.
    pub subchunks: usize,
    /// Palette block states inspected.
    pub block_states: usize,
    /// Palette states older than the requested target version.
    pub legacy_block_states: usize,
    /// Palette states without storage-version metadata.
    pub unknown_version_block_states: usize,
    /// Modern actor payloads encountered.
    pub actor_records: usize,
    /// Actor digest records encountered.
    pub actor_digests: usize,
    /// Actor references that point at no actor payload.
    pub dangling_actor_references: usize,
    /// Actor payloads not referenced by any digest.
    pub orphan_actor_records: usize,
    /// Unknown/unclassified storage records encountered.
    pub unknown_records: usize,
    /// Retained findings, bounded by [`WorldIntegrityOptions::max_issues`].
    pub issues: Vec<WorldIntegrityIssue>,
    /// Number of additional findings omitted after hitting the issue bound.
    pub omitted_issues: usize,
}

impl Default for WorldIntegrityReport {
    fn default() -> Self {
        Self {
            status: WorldIntegrityStatus::Healthy,
            records_scanned: 0,
            chunk_records: 0,
            subchunks: 0,
            block_states: 0,
            legacy_block_states: 0,
            unknown_version_block_states: 0,
            actor_records: 0,
            actor_digests: 0,
            dangling_actor_references: 0,
            orphan_actor_records: 0,
            unknown_records: 0,
            issues: Vec::new(),
            omitted_issues: 0,
        }
    }
}

impl WorldIntegrityReport {
    fn push_issue(&mut self, options: &WorldIntegrityOptions, issue: WorldIntegrityIssue) {
        if self.issues.len() < options.max_issues {
            self.issues.push(issue);
        } else {
            self.omitted_issues = self.omitted_issues.saturating_add(1);
        }
    }

    fn finish_status(&mut self) {
        if self
            .issues
            .iter()
            .any(|issue| issue.severity == WorldIntegritySeverity::Error)
        {
            self.status = WorldIntegrityStatus::Corrupt;
            return;
        }
        if self.legacy_block_states != 0
            || self
                .issues
                .iter()
                .any(|issue| issue.kind == WorldIntegrityIssueKind::UnsupportedSubChunk)
        {
            self.status = WorldIntegrityStatus::LegacyNeedsUpgrade;
            return;
        }
        if self
            .issues
            .iter()
            .any(|issue| issue.severity == WorldIntegritySeverity::Warning)
        {
            self.status = WorldIntegrityStatus::Repairable;
        }
    }
}

/// Audits one world folder and raw storage backend without modifying either.
pub fn audit_world_integrity_blocking(
    world_path: &Path,
    storage: &dyn WorldStorage,
    options: WorldIntegrityOptions,
) -> Result<WorldIntegrityReport> {
    let mut report = WorldIntegrityReport::default();
    audit_level_dat(world_path, &options, &mut report);

    let mut actor_records = BTreeSet::<i64>::new();
    let mut actor_references = BTreeMap::<i64, usize>::new();
    let mut actor_digest_locations = BTreeMap::<i64, String>::new();

    storage.for_each_entry(options.storage.clone(), &mut |key, value| {
        report.records_scanned = report.records_scanned.saturating_add(1);
        let decoded = BedrockDbKey::decode(key);
        let location = decoded.summary_kind();

        if value.is_empty() {
            report.push_issue(
                &options,
                WorldIntegrityIssue {
                    severity: WorldIntegritySeverity::Error,
                    kind: WorldIntegrityIssueKind::EmptyRecord,
                    location: location.clone(),
                    detail: "storage value is empty".to_string(),
                },
            );
        }

        match decoded {
            BedrockDbKey::Chunk(chunk_key) => {
                report.chunk_records = report.chunk_records.saturating_add(1);
                match chunk_key.tag {
                    ChunkRecordTag::SubChunkPrefix if options.validate_subchunks => {
                        report.subchunks = report.subchunks.saturating_add(1);
                        let y = chunk_key.subchunk_y.unwrap_or(0);
                        match parse_subchunk_with_mode(y, value.clone(), SubChunkDecodeMode::CountsOnly)
                        {
                            Ok(subchunk) => match subchunk.format {
                                SubChunkFormat::Raw { version, .. } => report.push_issue(
                                    &options,
                                    WorldIntegrityIssue {
                                        severity: WorldIntegritySeverity::Warning,
                                        kind: WorldIntegrityIssueKind::UnsupportedSubChunk,
                                        location: format!("{location}@y={y}"),
                                        detail: format!(
                                            "subchunk payload version {version:?} is preserved raw"
                                        ),
                                    },
                                ),
                                SubChunkFormat::Paletted { storages, .. } => {
                                    for palette in storages {
                                        for state in palette.states {
                                            report.block_states =
                                                report.block_states.saturating_add(1);
                                            match (options.target_block_state_version, state.version) {
                                                (Some(target), Some(version)) if version < target => {
                                                    report.legacy_block_states = report
                                                        .legacy_block_states
                                                        .saturating_add(1);
                                                }
                                                (Some(_), None) => {
                                                    report.unknown_version_block_states = report
                                                        .unknown_version_block_states
                                                        .saturating_add(1);
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            },
                            Err(error) => report.push_issue(
                                &options,
                                WorldIntegrityIssue {
                                    severity: WorldIntegritySeverity::Error,
                                    kind: WorldIntegrityIssueKind::UnsupportedSubChunk,
                                    location: format!("{location}@y={y}"),
                                    detail: error.to_string(),
                                },
                            ),
                        }
                    }
                    ChunkRecordTag::LegacyTerrain if options.validate_subchunks => {
                        if let Err(error) = LegacyTerrain::parse(value.clone()) {
                            report.push_issue(
                                &options,
                                WorldIntegrityIssue {
                                    severity: WorldIntegritySeverity::Error,
                                    kind: WorldIntegrityIssueKind::MalformedLegacyTerrain,
                                    location,
                                    detail: error.to_string(),
                                },
                            );
                        }
                    }
                    ChunkRecordTag::BlockEntity | ChunkRecordTag::Entity
                        if options.validate_nbt =>
                    {
                        if let Err(error) = parse_consecutive_root_nbt(value) {
                            report.push_issue(
                                &options,
                                WorldIntegrityIssue {
                                    severity: WorldIntegritySeverity::Error,
                                    kind: WorldIntegrityIssueKind::MalformedNbt,
                                    location,
                                    detail: error.to_string(),
                                },
                            );
                        }
                    }
                    _ => {}
                }
            }
            BedrockDbKey::ActorPrefix { actor_id } => {
                report.actor_records = report.actor_records.saturating_add(1);
                actor_records.insert(actor_id);
                if options.validate_nbt {
                    if let Err(error) = parse_consecutive_root_nbt(value) {
                        report.push_issue(
                            &options,
                            WorldIntegrityIssue {
                                severity: WorldIntegritySeverity::Error,
                                kind: WorldIntegrityIssueKind::MalformedNbt,
                                location,
                                detail: error.to_string(),
                            },
                        );
                    }
                }
            }
            BedrockDbKey::ActorDigest { pos } => {
                report.actor_digests = report.actor_digests.saturating_add(1);
                if options.validate_actor_links {
                    match parse_actor_digest_ids(value) {
                        Ok(ids) => {
                            for uid in ids {
                                *actor_references.entry(uid.0).or_insert(0) += 1;
                                actor_digest_locations
                                    .entry(uid.0)
                                    .or_insert_with(|| format!("digp:{pos:?}"));
                            }
                        }
                        Err(error) => report.push_issue(
                            &options,
                            WorldIntegrityIssue {
                                severity: WorldIntegritySeverity::Error,
                                kind: WorldIntegrityIssueKind::DanglingActorDigest,
                                location,
                                detail: error.to_string(),
                            },
                        ),
                    }
                }
            }
            BedrockDbKey::LocalPlayer | BedrockDbKey::RemotePlayer(_) if options.validate_nbt => {
                if let Err(error) = parse_root_nbt(value) {
                    report.push_issue(
                        &options,
                        WorldIntegrityIssue {
                            severity: WorldIntegritySeverity::Error,
                            kind: WorldIntegrityIssueKind::MalformedNbt,
                            location,
                            detail: error.to_string(),
                        },
                    );
                }
            }
            BedrockDbKey::Unknown(_) => {
                report.unknown_records = report.unknown_records.saturating_add(1);
            }
            _ => {}
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    if options.validate_actor_links {
        for (actor_id, references) in &actor_references {
            if !actor_records.contains(actor_id) {
                report.dangling_actor_references =
                    report.dangling_actor_references.saturating_add(*references);
                report.push_issue(
                    &options,
                    WorldIntegrityIssue {
                        severity: WorldIntegritySeverity::Error,
                        kind: WorldIntegrityIssueKind::DanglingActorDigest,
                        location: actor_digest_locations
                            .get(actor_id)
                            .cloned()
                            .unwrap_or_else(|| "digp".to_string()),
                        detail: format!("actor {actor_id} is referenced but actorprefix is missing"),
                    },
                );
            }
            if *references > 1 {
                report.push_issue(
                    &options,
                    WorldIntegrityIssue {
                        severity: WorldIntegritySeverity::Warning,
                        kind: WorldIntegrityIssueKind::ActorReferencedByMultipleChunks,
                        location: format!("actorprefix:{actor_id}"),
                        detail: format!("actor is referenced by {references} chunk digests"),
                    },
                );
            }
        }
        for actor_id in actor_records {
            if !actor_references.contains_key(&actor_id) {
                report.orphan_actor_records = report.orphan_actor_records.saturating_add(1);
                report.push_issue(
                    &options,
                    WorldIntegrityIssue {
                        severity: WorldIntegritySeverity::Warning,
                        kind: WorldIntegrityIssueKind::OrphanActorRecord,
                        location: format!("actorprefix:{actor_id}"),
                        detail: "actor payload is not referenced by any digp record".to_string(),
                    },
                );
            }
        }
    }

    if report.legacy_block_states != 0 {
        report.push_issue(
            &options,
            WorldIntegrityIssue {
                severity: WorldIntegritySeverity::Warning,
                kind: WorldIntegrityIssueKind::LegacyBlockState,
                location: "SubChunk palettes".to_string(),
                detail: format!(
                    "{} palette states are older than target version {:?}",
                    report.legacy_block_states, options.target_block_state_version
                ),
            },
        );
    }
    if report.unknown_version_block_states != 0 {
        report.push_issue(
            &options,
            WorldIntegrityIssue {
                severity: WorldIntegritySeverity::Warning,
                kind: WorldIntegrityIssueKind::UnknownBlockStateVersion,
                location: "SubChunk palettes".to_string(),
                detail: format!(
                    "{} palette states have no storage-version metadata",
                    report.unknown_version_block_states
                ),
            },
        );
    }
    report.finish_status();
    Ok(report)
}

fn audit_level_dat(
    world_path: &Path,
    options: &WorldIntegrityOptions,
    report: &mut WorldIntegrityReport,
) {
    let level_dat = world_path.join("level.dat");
    match read_level_dat_document(&level_dat) {
        Ok(document) => {
            for warning in &document.warnings {
                report.push_issue(
                    options,
                    WorldIntegrityIssue {
                        severity: WorldIntegritySeverity::Warning,
                        kind: WorldIntegrityIssueKind::LevelDatWarning,
                        location: "level.dat".to_string(),
                        detail: format!("{warning:?}"),
                    },
                );
            }
            match document.random_seed() {
                Ok(Some(_)) => {}
                Ok(None) => report.push_issue(
                    options,
                    WorldIntegrityIssue {
                        severity: WorldIntegritySeverity::Error,
                        kind: WorldIntegrityIssueKind::MissingRandomSeed,
                        location: "level.dat.RandomSeed".to_string(),
                        detail: "existing world has no authoritative RandomSeed".to_string(),
                    },
                ),
                Err(error) => report.push_issue(
                    options,
                    WorldIntegrityIssue {
                        severity: WorldIntegritySeverity::Error,
                        kind: WorldIntegrityIssueKind::LevelDatUnreadable,
                        location: "level.dat.RandomSeed".to_string(),
                        detail: error.to_string(),
                    },
                ),
            }
            if let NbtTag::Compound(root) = &document.root {
                for name in ["SpawnX", "SpawnY", "SpawnZ"] {
                    if !matches!(root.get(name), Some(NbtTag::Int(_) | NbtTag::Short(_))) {
                        report.push_issue(
                            options,
                            WorldIntegrityIssue {
                                severity: WorldIntegritySeverity::Warning,
                                kind: WorldIntegrityIssueKind::LevelDatWarning,
                                location: format!("level.dat.{name}"),
                                detail: "spawn coordinate is missing or uses an unsupported type"
                                    .to_string(),
                            },
                        );
                    }
                }
            }
        }
        Err(error) => report.push_issue(
            options,
            WorldIntegrityIssue {
                severity: WorldIntegritySeverity::Error,
                kind: WorldIntegrityIssueKind::LevelDatUnreadable,
                location: "level.dat".to_string(),
                detail: error.to_string(),
            },
        ),
    }
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Runs a read-only integrity audit against this opened world.
    pub fn audit_integrity_blocking(
        &self,
        options: WorldIntegrityOptions,
    ) -> Result<WorldIntegrityReport> {
        audit_world_integrity_blocking(self.path(), self.storage(), options)
    }

    /// Async wrapper for [`Self::audit_integrity_blocking`].
    #[cfg(feature = "async")]
    pub async fn audit_integrity(
        &self,
        options: WorldIntegrityOptions,
    ) -> Result<WorldIntegrityReport> {
        let path = self.path().to_path_buf();
        let storage = self.storage_backend().clone();
        tokio::task::spawn_blocking(move || {
            audit_world_integrity_blocking(&path, storage.storage(), options)
        })
        .await
        .map_err(|error| crate::BedrockWorldError::Join(error.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{ActorDigestKey, ActorUid};
    use crate::level_dat::{LevelDatDocument, write_level_dat_document};
    use crate::storage::MemoryStorage;
    use bytes::Bytes;
    use indexmap::IndexMap;
    use std::fs;

    #[test]
    fn audit_detects_dangling_and_orphan_actors() {
        let storage = MemoryStorage::default();
        let digest = ActorDigestKey::new(crate::ChunkPos {
            x: 0,
            z: 0,
            dimension: crate::Dimension::Overworld,
        });
        storage
            .put(&digest.storage_key(), &42_i64.to_le_bytes())
            .expect("digest");
        storage
            .put(&ActorUid(7).storage_key(), &[10, 0, 0, 0])
            .expect("actor");

        let temp = std::env::temp_dir().join(format!(
            "bedrock-world-integrity-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).expect("dir");
        let document = LevelDatDocument::new(
            10,
            NbtTag::Compound(IndexMap::from([
                ("RandomSeed".to_string(), NbtTag::Long(1)),
                ("SpawnX".to_string(), NbtTag::Int(0)),
                ("SpawnY".to_string(), NbtTag::Int(64)),
                ("SpawnZ".to_string(), NbtTag::Int(0)),
            ])),
        );
        write_level_dat_document(&temp.join("level.dat"), &document).expect("level.dat");

        let report = audit_world_integrity_blocking(
            &temp,
            &storage,
            WorldIntegrityOptions {
                validate_nbt: false,
                ..WorldIntegrityOptions::default()
            },
        )
        .expect("audit");
        assert_eq!(report.dangling_actor_references, 1);
        assert_eq!(report.orphan_actor_records, 1);
        assert_eq!(report.status, WorldIntegrityStatus::Corrupt);
        let _ = fs::remove_dir_all(temp);
    }
}
