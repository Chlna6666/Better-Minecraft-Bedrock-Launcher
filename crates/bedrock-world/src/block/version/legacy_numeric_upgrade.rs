//! Forward-verified reverse lookup from modern BlockStates to historical numeric Bedrock blocks.
//!
//! Every representable historical `(u8 id, u8 metadata)` candidate is upgraded once through the
//! authoritative BlockState rules. The resulting modern semantic states are indexed for exact reverse
//! lookup, so destructive writers never invert rename/property rules heuristically.

use super::{
    AuthoritativeBlockStateCatalog, BlockStateStorageVersion, LegacyNumericBlock,
    LegacyNumericBlockMatch, LegacyNumericBlockStateTable,
};
use crate::block::BlockState;
use crate::error::Result;
use crate::nbt::NbtTag;
use std::collections::BTreeMap;

/// Diagnostics for a forward-verified historical numeric reverse table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LegacyNumericBlockUpgradeTableStats {
    /// Historical u8 ID/u8 metadata entries that existed and were upgraded.
    pub source_mappings: usize,
    /// Distinct modern semantic BlockStates produced by those mappings.
    pub target_states: usize,
    /// Modern states reached by more than one historical numeric representation.
    pub ambiguous_target_states: usize,
}

/// Reverse table built by running historical numeric BlockStates through authoritative upgrades.
#[derive(Debug, Clone)]
pub struct LegacyNumericBlockUpgradeTable {
    output_version: BlockStateStorageVersion,
    by_name: BTreeMap<String, Vec<(BTreeMap<String, NbtTag>, LegacyNumericBlockMatch)>>,
    stats: LegacyNumericBlockUpgradeTableStats,
}

impl LegacyNumericBlockUpgradeTable {
    /// Builds a reverse table for historical numeric representations that classic Bedrock formats can
    /// actually store: an 8-bit block ID and up to an 8-bit metadata value.
    ///
    /// Each existing candidate is upgraded exactly once. Multiple historical aliases that converge to
    /// one modern permutation become [`LegacyNumericBlockMatch::Ambiguous`] instead of selecting one.
    pub fn build(
        numeric: &LegacyNumericBlockStateTable,
        catalog: &AuthoritativeBlockStateCatalog,
    ) -> Result<Self> {
        let mut by_name =
            BTreeMap::<String, Vec<(BTreeMap<String, NbtTag>, LegacyNumericBlockMatch)>>::new();
        let mut stats = LegacyNumericBlockUpgradeTableStats::default();

        for numeric_id in 0_u32..=u32::from(u8::MAX) {
            for metadata in 0_u32..=u32::from(u8::MAX) {
                let Some(source) = numeric.get(numeric_id, metadata) else {
                    continue;
                };
                stats.source_mappings = stats.source_mappings.saturating_add(1);
                let upgraded = catalog.upgrade(source)?;
                let candidate = LegacyNumericBlock {
                    numeric_id,
                    metadata,
                };
                insert_candidate(&mut by_name, upgraded, candidate, &mut stats);
            }
        }

        stats.target_states = by_name.values().map(Vec::len).sum();
        Ok(Self {
            output_version: catalog.output_version(),
            by_name,
            stats,
        })
    }

    /// Returns the modern BlockState storage-version endpoint used to build this table.
    #[must_use]
    pub const fn output_version(&self) -> BlockStateStorageVersion {
        self.output_version
    }

    /// Returns build diagnostics.
    #[must_use]
    pub const fn stats(&self) -> LegacyNumericBlockUpgradeTableStats {
        self.stats
    }

    /// Looks up one modern semantic BlockState without allocating.
    ///
    /// Persisted BlockState `version` is intentionally ignored; the table endpoint is exposed
    /// separately through [`Self::output_version`].
    #[must_use]
    pub fn match_numeric(&self, state: &BlockState) -> LegacyNumericBlockMatch {
        let Some(permutations) = self.by_name.get(state.name.as_str()) else {
            return LegacyNumericBlockMatch::Missing;
        };
        permutations
            .iter()
            .find_map(|(states, result)| (states == &state.states).then_some(*result))
            .unwrap_or(LegacyNumericBlockMatch::Missing)
    }
}

fn insert_candidate(
    by_name: &mut BTreeMap<String, Vec<(BTreeMap<String, NbtTag>, LegacyNumericBlockMatch)>>,
    upgraded: BlockState,
    candidate: LegacyNumericBlock,
    stats: &mut LegacyNumericBlockUpgradeTableStats,
) {
    let permutations = by_name.entry(upgraded.name).or_default();
    if let Some((_, result)) = permutations
        .iter_mut()
        .find(|(states, _)| states == &upgraded.states)
    {
        match result {
            LegacyNumericBlockMatch::Missing => {
                *result = LegacyNumericBlockMatch::Unique(candidate);
            }
            LegacyNumericBlockMatch::Unique(first) if *first != candidate => {
                *result = LegacyNumericBlockMatch::Ambiguous {
                    first: *first,
                    second: candidate,
                    matches: 2,
                };
                stats.ambiguous_target_states = stats.ambiguous_target_states.saturating_add(1);
            }
            LegacyNumericBlockMatch::Unique(_) => {}
            LegacyNumericBlockMatch::Ambiguous { matches, .. } => {
                *matches = matches.saturating_add(1);
            }
        }
        return;
    }
    permutations.push((upgraded.states, LegacyNumericBlockMatch::Unique(candidate)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockStateSchemaSource, LegacyNumericBlockStateTable};
    use crate::nbt::{NbtTag, serialize_root_nbt};
    use indexmap::IndexMap;

    fn put_var_u32(mut value: u32, output: &mut Vec<u8>) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            output.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn numeric_table(entries: &[(u32, u32, &str, i32)]) -> LegacyNumericBlockStateTable {
        let mut grouped = BTreeMap::<&str, Vec<(u32, i32)>>::new();
        for (_, meta, name, version) in entries {
            grouped.entry(name).or_default().push((*meta, *version));
        }
        let mut bytes = Vec::new();
        put_var_u32(grouped.len() as u32, &mut bytes);
        for (name, metas) in grouped {
            put_var_u32(name.len() as u32, &mut bytes);
            bytes.extend_from_slice(name.as_bytes());
            put_var_u32(metas.len() as u32, &mut bytes);
            for (meta, version) in metas {
                put_var_u32(meta, &mut bytes);
                let nbt = NbtTag::Compound(IndexMap::from([
                    ("name".to_string(), NbtTag::String(name.to_string())),
                    ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                    ("version".to_string(), NbtTag::Int(version)),
                ]));
                bytes.extend_from_slice(&serialize_root_nbt(&nbt).unwrap());
            }
        }
        let ids = entries
            .iter()
            .map(|(id, _, name, _)| format!(r#""{name}":{id}"#))
            .collect::<Vec<_>>()
            .join(",");
        LegacyNumericBlockStateTable::parse(&bytes, &format!("{{{ids}}}")).unwrap()
    }

    #[test]
    fn reverse_table_matches_after_authoritative_rename() {
        let source_version = 0x0100_0000;
        let table = numeric_table(&[(1, 0, "minecraft:old", source_version)]);
        let catalog = AuthoritativeBlockStateCatalog::from_sources(&[BlockStateSchemaSource {
            name: "0001_test.json",
            json: r#"{"maxVersionMajor":1,"maxVersionMinor":1,"maxVersionPatch":0,"maxVersionRevision":0,"renamedIds":{"minecraft:old":"minecraft:new"}}"#,
        }])
        .unwrap();
        let reverse = LegacyNumericBlockUpgradeTable::build(&table, &catalog).unwrap();
        let target = BlockState {
            name: "minecraft:new".to_string(),
            states: BTreeMap::new(),
            version: Some(catalog.output_version().raw()),
        };
        assert_eq!(
            reverse.match_numeric(&target),
            LegacyNumericBlockMatch::Unique(LegacyNumericBlock {
                numeric_id: 1,
                metadata: 0,
            })
        );
    }

    #[test]
    fn reverse_table_reports_aliases_after_upgrade() {
        let source_version = 0x0100_0000;
        let table = numeric_table(&[
            (1, 0, "minecraft:first", source_version),
            (2, 0, "minecraft:second", source_version),
        ]);
        let catalog = AuthoritativeBlockStateCatalog::from_sources(&[BlockStateSchemaSource {
            name: "0001_test.json",
            json: r#"{"maxVersionMajor":1,"maxVersionMinor":1,"maxVersionPatch":0,"maxVersionRevision":0,"renamedIds":{"minecraft:first":"minecraft:new","minecraft:second":"minecraft:new"}}"#,
        }])
        .unwrap();
        let reverse = LegacyNumericBlockUpgradeTable::build(&table, &catalog).unwrap();
        let target = BlockState {
            name: "minecraft:new".to_string(),
            states: BTreeMap::new(),
            version: None,
        };
        assert!(matches!(
            reverse.match_numeric(&target),
            LegacyNumericBlockMatch::Ambiguous { matches: 2, .. }
        ));
        assert_eq!(reverse.stats().ambiguous_target_states, 1);
    }
}
