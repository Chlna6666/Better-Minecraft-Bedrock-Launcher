//! Version-graph migration for historical Bedrock block states.
//!
//! A block-state storage version is not a semantic block identity by itself. Bedrock has evolved
//! state names, identifiers and value domains across releases, so robust conversion is represented as
//! explicit directed migration edges rather than stamping an old state with the newest version.

use super::block_state_upgrade::{
    BlockStateUpgradeRule, BlockStateUpgradeStatus, BlockStateUpgrader,
};
use crate::block::BlockState;
use crate::error::{BedrockWorldError, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// One directed block-state schema migration step.
#[derive(Debug, Clone)]
pub struct BlockStateMigrationStep {
    /// Exact source storage version accepted by this edge.
    pub from_version: i32,
    /// Target storage version produced by this edge.
    pub to_version: i32,
    /// Declarative semantic rewrite rules evaluated during this edge.
    pub rules: Vec<BlockStateUpgradeRule>,
    /// Whether states not matched by any rule are known to be schema-compatible across this edge.
    ///
    /// This must only be enabled from authoritative version knowledge. It is not a general fallback.
    pub allow_identity: bool,
}

impl BlockStateMigrationStep {
    /// Creates a migration edge that requires an explicit matching rewrite for every state.
    #[must_use]
    pub const fn new(from_version: i32, to_version: i32) -> Self {
        Self {
            from_version,
            to_version,
            rules: Vec::new(),
            allow_identity: false,
        }
    }

    /// Creates an explicitly-authorised identity-compatible schema edge.
    #[must_use]
    pub const fn identity(from_version: i32, to_version: i32) -> Self {
        Self {
            from_version,
            to_version,
            rules: Vec::new(),
            allow_identity: true,
        }
    }

    /// Appends one rewrite rule to this migration step.
    pub fn push_rule(&mut self, rule: BlockStateUpgradeRule) {
        self.rules.push(rule);
    }

    fn apply(&self, state: &BlockState) -> Result<BlockState> {
        if state.version != Some(self.from_version) {
            return Err(BedrockWorldError::Validation(format!(
                "block-state migration step expects version {}, got {:?}",
                self.from_version, state.version
            )));
        }

        let mut upgrader = BlockStateUpgrader::new(self.to_version);
        for mut rule in self.rules.clone() {
            if rule.min_source_version.is_none() {
                rule.min_source_version = Some(self.from_version);
            }
            if rule.max_source_version.is_none() {
                rule.max_source_version = Some(self.from_version);
            }
            upgrader.push_rule(rule);
        }
        let result = upgrader.upgrade(state)?;
        match result.status {
            BlockStateUpgradeStatus::Upgraded { .. } => Ok(result.state),
            BlockStateUpgradeStatus::UnresolvedLegacy if self.allow_identity => {
                let mut identity = state.clone();
                identity.version = Some(self.to_version);
                Ok(identity)
            }
            BlockStateUpgradeStatus::UnknownVersion if self.allow_identity => {
                Err(BedrockWorldError::Validation(
                    "identity migration requires an explicit source block-state version".to_string(),
                ))
            }
            BlockStateUpgradeStatus::Current => Ok(result.state),
            BlockStateUpgradeStatus::FutureVersion { version } => {
                Err(BedrockWorldError::Validation(format!(
                    "migration edge {} -> {} received future version {version}",
                    self.from_version, self.to_version
                )))
            }
            BlockStateUpgradeStatus::UnresolvedLegacy | BlockStateUpgradeStatus::UnknownVersion => {
                Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                    "no block-state migration rule matched {} on schema edge {} -> {}",
                    state.name, self.from_version, self.to_version
                )))
            }
        }
    }
}

/// Directed graph of known block-state schema migrations.
#[derive(Debug, Clone, Default)]
pub struct BlockStateMigrationGraph {
    edges: BTreeMap<i32, Vec<BlockStateMigrationStep>>,
}

impl BlockStateMigrationGraph {
    /// Creates an empty migration graph.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            edges: BTreeMap::new(),
        }
    }

    /// Adds one directed migration edge.
    pub fn add_step(&mut self, step: BlockStateMigrationStep) -> Result<()> {
        if step.from_version == step.to_version {
            return Err(BedrockWorldError::Validation(
                "block-state migration edge must change the schema version".to_string(),
            ));
        }
        let siblings = self.edges.entry(step.from_version).or_default();
        if siblings
            .iter()
            .any(|existing| existing.to_version == step.to_version)
        {
            return Err(BedrockWorldError::Validation(format!(
                "duplicate block-state migration edge {} -> {}",
                step.from_version, step.to_version
            )));
        }
        siblings.push(step);
        siblings.sort_by_key(|edge| edge.to_version);
        Ok(())
    }

    /// Returns one shortest known version path, including source and target versions.
    #[must_use]
    pub fn version_path(&self, from_version: i32, to_version: i32) -> Option<Vec<i32>> {
        if from_version == to_version {
            return Some(vec![from_version]);
        }
        let mut queue = VecDeque::from([from_version]);
        let mut visited = BTreeSet::from([from_version]);
        let mut parent = BTreeMap::<i32, i32>::new();

        while let Some(current) = queue.pop_front() {
            for edge in self.edges.get(&current).into_iter().flatten() {
                if !visited.insert(edge.to_version) {
                    continue;
                }
                parent.insert(edge.to_version, current);
                if edge.to_version == to_version {
                    let mut path = vec![to_version];
                    let mut cursor = to_version;
                    while cursor != from_version {
                        cursor = *parent.get(&cursor)?;
                        path.push(cursor);
                    }
                    path.reverse();
                    return Some(path);
                }
                queue.push_back(edge.to_version);
            }
        }
        None
    }

    /// Migrates one block state along a known version path.
    pub fn migrate_to(&self, state: &BlockState, target_version: i32) -> Result<BlockState> {
        let source_version = state.version.ok_or_else(|| {
            BedrockWorldError::Validation(format!(
                "block state {} has no source storage version",
                state.name
            ))
        })?;
        if source_version == target_version {
            return Ok(state.clone());
        }
        let path = self.version_path(source_version, target_version).ok_or_else(|| {
            BedrockWorldError::UnsupportedChunkFormat(format!(
                "no block-state migration path from version {source_version} to {target_version}"
            ))
        })?;

        let mut current = state.clone();
        for versions in path.windows(2) {
            let from = versions[0];
            let to = versions[1];
            let step = self
                .edges
                .get(&from)
                .and_then(|edges| edges.iter().find(|edge| edge.to_version == to))
                .ok_or_else(|| {
                    BedrockWorldError::CorruptWorld(format!(
                        "block-state migration graph lost edge {from} -> {to}"
                    ))
                })?;
            current = step.apply(&current)?;
        }
        Ok(current)
    }

    /// Migrates and validates the final semantic state against an authoritative target palette.
    pub fn migrate_to_strict_with_validator<F>(
        &self,
        state: &BlockState,
        target_version: i32,
        validator: F,
    ) -> Result<BlockState>
    where
        F: Fn(&BlockState) -> bool,
    {
        let migrated = self.migrate_to(state, target_version)?;
        if !validator(&migrated) {
            return Err(BedrockWorldError::Validation(format!(
                "migrated block state {} is not registered in target palette version {target_version}",
                migrated.name
            )));
        }
        Ok(migrated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::NbtTag;

    fn state(version: i32) -> BlockState {
        BlockState {
            name: "minecraft:test".to_string(),
            states: BTreeMap::from([("old".to_string(), NbtTag::Int(1))]),
            version: Some(version),
        }
    }

    #[test]
    fn graph_uses_explicit_multi_step_path() {
        let mut first = BlockStateMigrationStep::new(10, 20);
        let mut rename = BlockStateUpgradeRule::new("minecraft:test");
        rename
            .rename_states
            .insert("old".to_string(), "middle".to_string());
        first.push_rule(rename);

        let mut second = BlockStateMigrationStep::new(20, 30);
        let mut rename = BlockStateUpgradeRule::new("minecraft:test");
        rename
            .rename_states
            .insert("middle".to_string(), "new".to_string());
        second.push_rule(rename);

        let mut graph = BlockStateMigrationGraph::new();
        graph.add_step(first).expect("first edge");
        graph.add_step(second).expect("second edge");
        assert_eq!(graph.version_path(10, 30), Some(vec![10, 20, 30]));
        let migrated = graph.migrate_to(&state(10), 30).expect("migrate");
        assert_eq!(migrated.version, Some(30));
        assert_eq!(migrated.states.get("new"), Some(&NbtTag::Int(1)));
    }

    #[test]
    fn graph_never_assumes_identity_without_explicit_edge_permission() {
        let mut graph = BlockStateMigrationGraph::new();
        graph
            .add_step(BlockStateMigrationStep::new(10, 20))
            .expect("edge");
        assert!(graph.migrate_to(&state(10), 20).is_err());

        let mut graph = BlockStateMigrationGraph::new();
        graph
            .add_step(BlockStateMigrationStep::identity(10, 20))
            .expect("identity edge");
        assert_eq!(graph.migrate_to(&state(10), 20).unwrap().version, Some(20));
    }
}
