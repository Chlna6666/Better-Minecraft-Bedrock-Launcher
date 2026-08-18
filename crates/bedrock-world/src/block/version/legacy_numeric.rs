//! Authoritative legacy numeric block ID/metadata mapping.
//!
//! BedrockBlockUpgradeSchema's `id_meta_to_nbt/*.bin` maps historical string block ids and metadata
//! values to versioned BlockState NBT. `block_legacy_id_map.json` supplies the corresponding numeric
//! ids. This block-domain parser has no chunk/storage dependency; chunk codecs adapt it through their
//! own resolver trait.

use crate::block::BlockState;
use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, parse_root_nbt_with_consumed};
use std::collections::{BTreeMap, BTreeSet};

/// Parsed legacy numeric block table.
#[derive(Debug, Clone)]
pub struct LegacyNumericBlockStateTable {
    dense_slots: Box<[u16; 4096]>,
    dense_states: Vec<BlockState>,
    extended: BTreeMap<(u32, u32), BlockState>,
    source_versions: BTreeSet<i32>,
    mapped_entries: usize,
}

/// Diagnostics produced while loading an authoritative numeric table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LegacyNumericBlockStateTableStats {
    /// Number of `(numeric id, metadata)` mappings retained.
    pub mapped_entries: usize,
    /// Mappings that fit the classic 8-bit id/four-bit metadata terrain path.
    pub dense_entries: usize,
    /// Mappings retained outside the classic dense range.
    pub extended_entries: usize,
    /// Number of distinct BlockState storage versions present in the source table.
    pub source_versions: usize,
}

impl LegacyNumericBlockStateTable {
    /// Parses an `id_meta_to_nbt` table together with a string-id -> numeric-id JSON map.
    pub fn parse(table: &[u8], legacy_id_map_json: &str) -> Result<Self> {
        let legacy_ids: BTreeMap<String, i64> =
            serde_json::from_str(legacy_id_map_json).map_err(|error| {
                validation(format!("invalid legacy block id map JSON: {error}"))
            })?;
        let mut offset = 0usize;
        let block_count = usize::try_from(read_var_u32(table, &mut offset)?)
            .map_err(|_| validation("legacy block table count overflowed usize"))?;

        let mut dense_slots = Box::new([u16::MAX; 4096]);
        let mut dense_states = Vec::<BlockState>::new();
        let mut extended = BTreeMap::<(u32, u32), BlockState>::new();
        let mut source_versions = BTreeSet::new();
        let mut mapped_entries = 0usize;

        for _ in 0..block_count {
            let name_len = usize::try_from(read_var_u32(table, &mut offset)?)
                .map_err(|_| validation("legacy block identifier length overflowed usize"))?;
            let name = std::str::from_utf8(take(table, &mut offset, name_len)?).map_err(|error| {
                validation(format!("legacy block table contains invalid UTF-8 identifier: {error}"))
            })?;
            let numeric_id = legacy_ids
                .get(name)
                .copied()
                .and_then(|value| u32::try_from(value).ok());

            let meta_count = usize::try_from(read_var_u32(table, &mut offset)?)
                .map_err(|_| validation("legacy block metadata count overflowed usize"))?;
            for _ in 0..meta_count {
                let metadata = read_var_u32(table, &mut offset)?;
                let remaining = table
                    .get(offset..)
                    .ok_or_else(|| validation("legacy block table NBT offset exceeds input"))?;
                let (root, consumed) = parse_root_nbt_with_consumed(remaining)?;
                if consumed == 0 {
                    return Err(validation("legacy block table NBT parser did not advance"));
                }
                offset = offset
                    .checked_add(consumed)
                    .ok_or_else(|| validation("legacy block table NBT offset overflowed usize"))?;
                let state = block_state_from_nbt(root)?;
                source_versions.insert(state.version.ok_or_else(|| {
                    validation(format!(
                        "legacy numeric mapping {name}:{metadata} has no BlockState version"
                    ))
                })?);

                let Some(numeric_id) = numeric_id else {
                    continue;
                };
                mapped_entries = mapped_entries.saturating_add(1);

                if numeric_id <= u32::from(u8::MAX) && metadata < 16 {
                    let slot = usize::try_from(numeric_id)
                        .ok()
                        .and_then(|id| id.checked_mul(16))
                        .and_then(|base| usize::try_from(metadata).ok().and_then(|meta| base.checked_add(meta)))
                        .ok_or_else(|| validation("legacy numeric dense slot overflowed usize"))?;
                    let existing = dense_slots[slot];
                    if existing == u16::MAX {
                        let index = u16::try_from(dense_states.len())
                            .map_err(|_| validation("legacy numeric dense table exceeds u16"))?;
                        dense_states.push(state);
                        dense_slots[slot] = index;
                    } else if dense_states[usize::from(existing)] != state {
                        return Err(validation(format!(
                            "legacy numeric table defines conflicting states for {numeric_id}:{metadata}"
                        )));
                    }
                } else {
                    match extended.entry((numeric_id, metadata)) {
                        std::collections::btree_map::Entry::Vacant(entry) => {
                            entry.insert(state);
                        }
                        std::collections::btree_map::Entry::Occupied(entry) => {
                            if entry.get() != &state {
                                return Err(validation(format!(
                                    "legacy numeric table defines conflicting extended states for {numeric_id}:{metadata}"
                                )));
                            }
                        }
                    }
                }
            }
        }

        if offset != table.len() {
            return Err(validation(format!(
                "legacy numeric block table has {} trailing bytes",
                table.len().saturating_sub(offset)
            )));
        }

        Ok(Self {
            dense_slots,
            dense_states,
            extended,
            source_versions,
            mapped_entries,
        })
    }

    /// Returns one mapping without allocating.
    #[must_use]
    pub fn get(&self, numeric_id: u32, metadata: u32) -> Option<&BlockState> {
        if numeric_id <= u32::from(u8::MAX) && metadata < 16 {
            let slot = usize::try_from(numeric_id).ok()?.checked_mul(16)?
                + usize::try_from(metadata).ok()?;
            let index = *self.dense_slots.get(slot)?;
            if index != u16::MAX {
                return self.dense_states.get(usize::from(index));
            }
        }
        self.extended.get(&(numeric_id, metadata))
    }

    /// Returns compact loading diagnostics.
    #[must_use]
    pub fn stats(&self) -> LegacyNumericBlockStateTableStats {
        LegacyNumericBlockStateTableStats {
            mapped_entries: self.mapped_entries,
            dense_entries: self.dense_states.len(),
            extended_entries: self.extended.len(),
            source_versions: self.source_versions.len(),
        }
    }

    /// Returns the unique source BlockState version when every mapping uses the same version.
    #[must_use]
    pub fn uniform_source_version(&self) -> Option<i32> {
        if self.source_versions.len() == 1 {
            self.source_versions.iter().next().copied()
        } else {
            None
        }
    }
}

fn block_state_from_nbt(root: NbtTag) -> Result<BlockState> {
    let NbtTag::Compound(mut root) = root else {
        return Err(validation("legacy numeric block table root is not a compound"));
    };
    let name = match root.shift_remove("name") {
        Some(NbtTag::String(name)) if !name.is_empty() => name,
        Some(other) => {
            return Err(validation(format!(
                "legacy numeric block name has invalid NBT type: {other:?}"
            )));
        }
        None => return Err(validation("legacy numeric block state has no name")),
    };
    let states = match root.shift_remove("states") {
        Some(NbtTag::Compound(states)) => states.into_iter().collect(),
        Some(other) => {
            return Err(validation(format!(
                "legacy numeric block states has invalid NBT type: {other:?}"
            )));
        }
        None => BTreeMap::new(),
    };
    let version = match root.shift_remove("version") {
        Some(NbtTag::Int(version)) => Some(version),
        Some(other) => {
            return Err(validation(format!(
                "legacy numeric block version has invalid NBT type: {other:?}"
            )));
        }
        None => None,
    };
    Ok(BlockState {
        name,
        states,
        version,
    })
}

fn read_var_u32(data: &[u8], offset: &mut usize) -> Result<u32> {
    let mut value = 0_u32;
    for shift in (0_u32..35).step_by(7) {
        let byte = *data
            .get(*offset)
            .ok_or_else(|| validation("legacy numeric block table ended inside varint"))?;
        *offset = offset
            .checked_add(1)
            .ok_or_else(|| validation("legacy numeric block table offset overflowed usize"))?;
        if shift == 28 && byte > 0x0f {
            return Err(validation(
                "legacy numeric block table contains overflowing u32 varint",
            ));
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(validation(
        "legacy numeric block table contains overlong u32 varint",
    ))
}

fn take<'a>(data: &'a [u8], offset: &mut usize, len: usize) -> Result<&'a [u8]> {
    let end = offset
        .checked_add(len)
        .ok_or_else(|| validation("legacy numeric block table slice overflowed usize"))?;
    let value = data
        .get(*offset..end)
        .ok_or_else(|| validation("legacy numeric block table ended inside a length-delimited field"))?;
    *offset = end;
    Ok(value)
}

fn validation(message: impl Into<String>) -> BedrockWorldError {
    BedrockWorldError::Validation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockStateStorageVersion;
    use crate::nbt::serialize_root_nbt;
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

    #[test]
    fn parses_dense_id_meta_mapping() {
        let version = BlockStateStorageVersion::from_components(1, 12, 0, 1).raw();
        let root = NbtTag::Compound(IndexMap::from([
            ("name".to_string(), NbtTag::String("minecraft:test".to_string())),
            (
                "states".to_string(),
                NbtTag::Compound(IndexMap::from([("kind".to_string(), NbtTag::Int(3))])),
            ),
            ("version".to_string(), NbtTag::Int(version)),
        ]));
        let nbt = serialize_root_nbt(&root).unwrap();
        let mut table = Vec::new();
        put_var_u32(1, &mut table);
        put_var_u32("minecraft:test".len() as u32, &mut table);
        table.extend_from_slice(b"minecraft:test");
        put_var_u32(1, &mut table);
        put_var_u32(2, &mut table);
        table.extend_from_slice(&nbt);

        let parsed =
            LegacyNumericBlockStateTable::parse(&table, r#"{"minecraft:test":1}"#).unwrap();
        let state = parsed.get(1, 2).unwrap();
        assert_eq!(state.name, "minecraft:test");
        assert_eq!(state.states.get("kind"), Some(&NbtTag::Int(3)));
        assert_eq!(parsed.uniform_source_version(), Some(version));
        assert_eq!(parsed.stats().dense_entries, 1);
    }
}
