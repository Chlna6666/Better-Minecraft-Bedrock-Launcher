//! Data-driven Bedrock block-state upgrade pipeline.
//!
//! Historical Bedrock worlds may persist block identifiers, state names, state values, and
//! storage-version metadata that no longer match a newer authoritative block palette. This module
//! deliberately separates *recognising* an old state from *rewriting* it: an older state without a
//! matching rule remains unresolved instead of being silently stamped with the target version.

use crate::chunk::BlockState;
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use std::collections::{BTreeMap, BTreeSet};

/// One exact state-value rewrite performed by a [`BlockStateUpgradeRule`].
#[derive(Debug, Clone, PartialEq)]
pub struct BlockStateValueRewrite {
    /// State property to inspect.
    pub state: String,
    /// Value that must be present before the rewrite applies.
    pub from: NbtTag,
    /// Replacement value.
    pub to: NbtTag,
}

/// Declarative rewrite rule for one historical block-state family.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockStateUpgradeRule {
    /// Source block identifier matched by this rule.
    pub source_identifier: String,
    /// Optional minimum source storage version, inclusive.
    pub min_source_version: Option<i32>,
    /// Optional maximum source storage version, inclusive.
    pub max_source_version: Option<i32>,
    /// Optional replacement block identifier.
    pub target_identifier: Option<String>,
    /// State-property renames applied before value rewrites.
    pub rename_states: BTreeMap<String, String>,
    /// State properties removed by the newer schema.
    pub remove_states: BTreeSet<String>,
    /// State properties inserted or overwritten by the newer schema.
    pub set_states: BTreeMap<String, NbtTag>,
    /// Exact state-value rewrites.
    pub value_rewrites: Vec<BlockStateValueRewrite>,
}

impl BlockStateUpgradeRule {
    /// Creates a rule matching the supplied source identifier.
    #[must_use]
    pub fn new(source_identifier: impl Into<String>) -> Self {
        Self {
            source_identifier: source_identifier.into(),
            min_source_version: None,
            max_source_version: None,
            target_identifier: None,
            rename_states: BTreeMap::new(),
            remove_states: BTreeSet::new(),
            set_states: BTreeMap::new(),
            value_rewrites: Vec::new(),
        }
    }

    #[must_use]
    fn matches(&self, state: &BlockState) -> bool {
        if state.name != self.source_identifier {
            return false;
        }
        let Some(version) = state.version else {
            return self.min_source_version.is_none() && self.max_source_version.is_none();
        };
        self.min_source_version.is_none_or(|minimum| version >= minimum)
            && self.max_source_version.is_none_or(|maximum| version <= maximum)
    }

    fn apply(&self, state: &mut BlockState) -> Result<bool> {
        if !self.matches(state) {
            return Ok(false);
        }

        if let Some(target_identifier) = &self.target_identifier {
            state.name.clone_from(target_identifier);
        }

        for (old, new) in &self.rename_states {
            if old == new {
                continue;
            }
            if let Some(value) = state.states.remove(old) {
                if state.states.contains_key(new) {
                    return Err(BedrockWorldError::Validation(format!(
                        "block-state upgrade would overwrite existing state {new} on {}",
                        state.name
                    )));
                }
                state.states.insert(new.clone(), value);
            }
        }
        for state_name in &self.remove_states {
            state.states.remove(state_name);
        }
        for rewrite in &self.value_rewrites {
            if state
                .states
                .get(&rewrite.state)
                .is_some_and(|value| value == &rewrite.from)
            {
                state.states.insert(rewrite.state.clone(), rewrite.to.clone());
            }
        }
        for (name, value) in &self.set_states {
            state.states.insert(name.clone(), value.clone());
        }
        Ok(true)
    }
}

/// Outcome of upgrading one block state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockStateUpgradeStatus {
    /// The state exactly targets the requested storage version.
    Current,
    /// One or more explicit rewrite rules upgraded the state.
    Upgraded {
        /// Number of rewrite rules that matched and were applied.
        rules_applied: usize,
    },
    /// The state is older than the target but no rewrite rule is known.
    UnresolvedLegacy,
    /// The state is newer than the target schema and must not be rewritten by an older library.
    FutureVersion {
        /// Storage version carried by the world state.
        version: i32,
    },
    /// The state carries no storage version and no explicit version-agnostic rule matched it.
    UnknownVersion,
}

/// Upgraded state together with the decision made by the upgrader.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockStateUpgradeResult {
    /// Resulting semantic state. Unresolved states are returned unchanged.
    pub state: BlockState,
    /// Upgrade classification.
    pub status: BlockStateUpgradeStatus,
}

/// Ordered, data-driven block-state upgrader.
#[derive(Debug, Clone)]
pub struct BlockStateUpgrader {
    target_version: i32,
    rules: Vec<BlockStateUpgradeRule>,
}

impl BlockStateUpgrader {
    /// Creates an upgrader targeting one authoritative Bedrock block-state version.
    #[must_use]
    pub const fn new(target_version: i32) -> Self {
        Self {
            target_version,
            rules: Vec::new(),
        }
    }

    /// Returns the target block-state version.
    #[must_use]
    pub const fn target_version(&self) -> i32 {
        self.target_version
    }

    /// Appends one rewrite rule. Rules are evaluated in insertion order.
    pub fn push_rule(&mut self, rule: BlockStateUpgradeRule) {
        self.rules.push(rule);
    }

    /// Returns the configured rewrite rules.
    #[must_use]
    pub fn rules(&self) -> &[BlockStateUpgradeRule] {
        &self.rules
    }

    /// Attempts to upgrade one state without guessing missing migrations.
    pub fn upgrade(&self, state: &BlockState) -> Result<BlockStateUpgradeResult> {
        if let Some(version) = state.version {
            if version == self.target_version {
                return Ok(BlockStateUpgradeResult {
                    state: state.clone(),
                    status: BlockStateUpgradeStatus::Current,
                });
            }
            if version > self.target_version {
                return Ok(BlockStateUpgradeResult {
                    state: state.clone(),
                    status: BlockStateUpgradeStatus::FutureVersion { version },
                });
            }
        }

        let mut upgraded = state.clone();
        let mut rules_applied = 0usize;
        for rule in &self.rules {
            if rule.apply(&mut upgraded)? {
                rules_applied = rules_applied.saturating_add(1);
            }
        }
        if rules_applied != 0 {
            upgraded.version = Some(self.target_version);
            return Ok(BlockStateUpgradeResult {
                state: upgraded,
                status: BlockStateUpgradeStatus::Upgraded { rules_applied },
            });
        }

        Ok(BlockStateUpgradeResult {
            state: state.clone(),
            status: if state.version.is_some() {
                BlockStateUpgradeStatus::UnresolvedLegacy
            } else {
                BlockStateUpgradeStatus::UnknownVersion
            },
        })
    }

    /// Upgrades one state and rejects unresolved, unknown, or future-version data.
    pub fn upgrade_strict(&self, state: &BlockState) -> Result<BlockState> {
        self.upgrade_strict_with_validator(state, |_| true)
    }

    /// Upgrades one state and requires the resulting semantic permutation to be accepted by a
    /// caller-supplied authoritative palette validator.
    ///
    /// This closes an important safety gap in data-driven migrations: matching a rewrite rule proves
    /// only that a transformation was requested, not that the resulting identifier/state assignment
    /// actually exists in the target Bedrock palette. Servers and editors should use this method when
    /// an authoritative version palette is available and only persist the returned state on success.
    pub fn upgrade_strict_with_validator<F>(
        &self,
        state: &BlockState,
        validator: F,
    ) -> Result<BlockState>
    where
        F: FnOnce(&BlockState) -> bool,
    {
        let result = self.upgrade(state)?;
        let resolved = match result.status {
            BlockStateUpgradeStatus::Current | BlockStateUpgradeStatus::Upgraded { .. } => {
                result.state
            }
            BlockStateUpgradeStatus::UnresolvedLegacy => {
                return Err(BedrockWorldError::Validation(format!(
                    "no block-state upgrade rule for {} version {:?} -> {}",
                    state.name, state.version, self.target_version
                )));
            }
            BlockStateUpgradeStatus::FutureVersion { version } => {
                return Err(BedrockWorldError::Validation(format!(
                    "block state {} uses future storage version {version}; target version {} must not rewrite it",
                    state.name, self.target_version
                )));
            }
            BlockStateUpgradeStatus::UnknownVersion => {
                return Err(BedrockWorldError::Validation(format!(
                    "block state {} has no storage version and no version-agnostic upgrade rule",
                    state.name
                )));
            }
        };
        if !validator(&resolved) {
            return Err(BedrockWorldError::Validation(format!(
                "upgraded block state {} is not present in the authoritative target palette version {}",
                resolved.name, self.target_version
            )));
        }
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old_state() -> BlockState {
        BlockState {
            name: "minecraft:old_test".to_string(),
            states: BTreeMap::from([
                ("old_direction".to_string(), NbtTag::Int(2)),
                ("obsolete".to_string(), NbtTag::Byte(1)),
            ]),
            version: Some(10),
        }
    }

    fn test_upgrader() -> BlockStateUpgrader {
        let mut rule = BlockStateUpgradeRule::new("minecraft:old_test");
        rule.max_source_version = Some(10);
        rule.target_identifier = Some("minecraft:test".to_string());
        rule.rename_states
            .insert("old_direction".to_string(), "direction".to_string());
        rule.remove_states.insert("obsolete".to_string());
        rule.set_states
            .insert("waterlogged".to_string(), NbtTag::Byte(0));
        let mut upgrader = BlockStateUpgrader::new(20);
        upgrader.push_rule(rule);
        upgrader
    }

    #[test]
    fn explicit_rule_upgrades_identifier_schema_and_version() {
        let upgrader = test_upgrader();
        let result = upgrader.upgrade(&old_state()).expect("upgrade");
        assert_eq!(
            result.status,
            BlockStateUpgradeStatus::Upgraded { rules_applied: 1 }
        );
        assert_eq!(result.state.name, "minecraft:test");
        assert_eq!(result.state.version, Some(20));
        assert_eq!(result.state.states.get("direction"), Some(&NbtTag::Int(2)));
        assert!(!result.state.states.contains_key("obsolete"));
    }

    #[test]
    fn unknown_legacy_state_is_never_silently_reversioned() {
        let upgrader = BlockStateUpgrader::new(20);
        let result = upgrader.upgrade(&old_state()).expect("upgrade");
        assert_eq!(result.status, BlockStateUpgradeStatus::UnresolvedLegacy);
        assert_eq!(result.state.version, Some(10));
        assert!(upgrader.upgrade_strict(&old_state()).is_err());
    }

    #[test]
    fn future_version_is_preserved_and_rejected_by_strict_upgrade() {
        let state = BlockState {
            name: "minecraft:future_test".to_string(),
            states: BTreeMap::new(),
            version: Some(21),
        };
        let upgrader = BlockStateUpgrader::new(20);
        let result = upgrader.upgrade(&state).expect("classify future state");
        assert_eq!(
            result.status,
            BlockStateUpgradeStatus::FutureVersion { version: 21 }
        );
        assert_eq!(result.state, state);
        assert!(upgrader.upgrade_strict(&state).is_err());
    }

    #[test]
    fn strict_validator_rejects_rule_output_missing_from_target_palette() {
        let upgrader = test_upgrader();
        let error = upgrader
            .upgrade_strict_with_validator(&old_state(), |_| false)
            .expect_err("validator must reject unknown target permutation");
        assert!(error.to_string().contains("authoritative target palette"));
    }

    #[test]
    fn strict_validator_accepts_known_target_permutation() {
        let upgrader = test_upgrader();
        let upgraded = upgrader
            .upgrade_strict_with_validator(&old_state(), |state| {
                state.name == "minecraft:test"
                    && state.states.get("direction") == Some(&NbtTag::Int(2))
                    && state.states.get("waterlogged") == Some(&NbtTag::Byte(0))
            })
            .expect("known permutation");
        assert_eq!(upgraded.version, Some(20));
    }
}
