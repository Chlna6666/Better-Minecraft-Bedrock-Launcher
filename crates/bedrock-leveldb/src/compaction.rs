use crate::error::Result;
use crate::manifest::{Manifest, TableFileMeta};
use crate::table::{self, TableLookup};
use bytes::Bytes;
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

pub(crate) const MAX_LEVEL: u32 = 6;
const LEVEL_ZERO_FILE_TRIGGER: usize = 4;
const TARGET_OUTPUT_FILE_BYTES: usize = 2 * 1024 * 1024;

pub(crate) struct CompactionPlan {
    pub(crate) inputs: Vec<TableFileMeta>,
    pub(crate) output_level: u32,
}

impl CompactionPlan {
    pub(crate) fn input_numbers(&self) -> HashSet<u64> {
        self.inputs.iter().map(|table| table.number).collect()
    }
}

pub(crate) fn plan(manifest: &Manifest, force: bool) -> Option<CompactionPlan> {
    let input_level = choose_input_level(manifest, force)?;
    let mut inputs = if input_level == 0 || force {
        manifest
            .table_files
            .iter()
            .filter(|table| table.level == input_level)
            .cloned()
            .collect::<Vec<_>>()
    } else {
        manifest
            .table_files
            .iter()
            .filter(|table| table.level == input_level)
            .min_by_key(|table| table.number)
            .cloned()
            .into_iter()
            .collect()
    };
    let output_level = input_level.saturating_add(1).min(MAX_LEVEL);
    let range = table_range(&inputs);
    inputs.extend(
        manifest
            .table_files
            .iter()
            .filter(|table| table.level == output_level && overlaps(table, range.as_ref()))
            .cloned(),
    );
    Some(CompactionPlan {
        inputs,
        output_level,
    })
}

pub(crate) fn merge(
    root: &Path,
    plan: &CompactionPlan,
    paranoid_checks: bool,
) -> Result<Vec<BTreeMap<Vec<u8>, Option<Bytes>>>> {
    let mut inputs = plan.inputs.clone();
    inputs.sort_by_key(|table| (Reverse(table.level), table.number));
    let mut merged = BTreeMap::<Vec<u8>, Option<Bytes>>::new();
    for table_meta in inputs {
        let path = root.join(Manifest::table_name(table_meta.number));
        for (key, lookup) in table::read_table_lookups(&path, paranoid_checks)? {
            let value = match lookup {
                TableLookup::Value(value) => Some(value),
                TableLookup::Deleted => None,
                TableLookup::Missing => continue,
            };
            merged.insert(key, value);
        }
    }
    if plan.output_level == MAX_LEVEL {
        merged.retain(|_, value| value.is_some());
    }
    Ok(partition(merged))
}

fn choose_input_level(manifest: &Manifest, force: bool) -> Option<u32> {
    let level_zero_count = manifest
        .table_files
        .iter()
        .filter(|table| table.level == 0)
        .count();
    if level_zero_count >= LEVEL_ZERO_FILE_TRIGGER || (force && level_zero_count != 0) {
        return Some(0);
    }
    for level in 1..MAX_LEVEL {
        let tables = manifest
            .table_files
            .iter()
            .filter(|table| table.level == level)
            .collect::<Vec<_>>();
        let bytes = tables
            .iter()
            .fold(0_u64, |total, table| total.saturating_add(table.file_size));
        if bytes > level_size_limit(level) || (force && !tables.is_empty()) {
            return Some(level);
        }
    }
    None
}

fn level_size_limit(level: u32) -> u64 {
    let exponent = level.saturating_sub(1).min(5);
    10_u64
        .saturating_pow(exponent)
        .saturating_mul(10 * 1024 * 1024)
}

fn table_range(tables: &[TableFileMeta]) -> Option<(Vec<u8>, Vec<u8>)> {
    if tables
        .iter()
        .any(|table| table.smallest_key.is_none() || table.largest_key.is_none())
    {
        return None;
    }
    let smallest = tables
        .iter()
        .filter_map(|table| table.smallest_key.as_ref())
        .min()?
        .clone();
    let largest = tables
        .iter()
        .filter_map(|table| table.largest_key.as_ref())
        .max()?
        .clone();
    Some((smallest, largest))
}

fn overlaps(table: &TableFileMeta, range: Option<&(Vec<u8>, Vec<u8>)>) -> bool {
    let Some((smallest, largest)) = range else {
        return true;
    };
    table.largest_key.as_ref().is_none_or(|key| key >= smallest)
        && table.smallest_key.as_ref().is_none_or(|key| key <= largest)
}

fn partition(entries: BTreeMap<Vec<u8>, Option<Bytes>>) -> Vec<BTreeMap<Vec<u8>, Option<Bytes>>> {
    let mut outputs = Vec::new();
    let mut current = BTreeMap::new();
    let mut current_bytes = 0_usize;
    for (key, value) in entries {
        let entry_bytes = key
            .len()
            .saturating_add(value.as_ref().map_or(0, Bytes::len))
            .saturating_add(24);
        if !current.is_empty()
            && current_bytes.saturating_add(entry_bytes) > TARGET_OUTPUT_FILE_BYTES
        {
            outputs.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(entry_bytes);
        current.insert(key, value);
    }
    if !current.is_empty() {
        outputs.push(current);
    }
    outputs
}
