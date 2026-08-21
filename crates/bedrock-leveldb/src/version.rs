use crate::manifest::{Manifest, TableFileMeta};
use crate::obsolete;
use bytes::Bytes;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub(crate) type MemTableEntries = BTreeMap<Vec<u8>, Option<Bytes>>;

/// Immutable memtable detached from the foreground writer.
///
/// The entry tree is reference counted so point reads and the background flush
/// worker can share the same key/value storage without copying the memtable.
#[derive(Debug)]
pub(crate) struct ImmutableMemTable {
    entries: Arc<MemTableEntries>,
    last_sequence: u64,
    log_number: u64,
    approximate_bytes: usize,
}

impl ImmutableMemTable {
    pub(crate) fn new(
        entries: MemTableEntries,
        last_sequence: u64,
        log_number: u64,
        approximate_bytes: usize,
    ) -> Self {
        Self {
            entries: Arc::new(entries),
            last_sequence,
            log_number,
            approximate_bytes,
        }
    }

    #[must_use]
    pub(crate) fn entries(&self) -> &MemTableEntries {
        &self.entries
    }

    #[must_use]
    pub(crate) fn entries_arc(&self) -> Arc<MemTableEntries> {
        Arc::clone(&self.entries)
    }

    #[must_use]
    pub(crate) const fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    #[must_use]
    pub(crate) const fn log_number(&self) -> u64 {
        self.log_number
    }

    #[must_use]
    pub(crate) const fn approximate_bytes(&self) -> usize {
        self.approximate_bytes
    }

    #[must_use]
    pub(crate) fn get(&self, key: &[u8]) -> Option<Option<Bytes>> {
        self.entries.get(key).cloned()
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Immutable set of SSTables visible to a reader.
///
/// Compaction attaches removed SST paths to the old version before publishing
/// a replacement. Physical deletion is delayed until the last reader drops the
/// old version, preventing reads from racing `remove_file` after releasing the
/// database metadata lock.
#[derive(Debug)]
pub(crate) struct ReadVersion {
    tables: Arc<[TableFileMeta]>,
    retired_paths: Mutex<Vec<PathBuf>>,
}

impl ReadVersion {
    #[must_use]
    pub(crate) fn from_manifest(manifest: &Manifest) -> Self {
        let tables = if manifest.table_files.is_empty() {
            manifest
                .table_numbers
                .iter()
                .copied()
                .map(TableFileMeta::without_range)
                .collect::<Vec<_>>()
        } else {
            manifest.table_files.clone()
        };
        Self {
            tables: Arc::from(tables),
            retired_paths: Mutex::new(Vec::new()),
        }
    }

    #[must_use]
    pub(crate) fn tables(&self) -> &[TableFileMeta] {
        &self.tables
    }

    pub(crate) fn retire_paths(&self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        match self.retired_paths.lock() {
            Ok(mut retired) => retired.extend(paths.iter().cloned()),
            Err(poisoned) => poisoned.into_inner().extend(paths.iter().cloned()),
        }
    }
}

impl Drop for ReadVersion {
    fn drop(&mut self) {
        let paths = match self.retired_paths.get_mut() {
            Ok(paths) => std::mem::take(paths),
            Err(poisoned) => std::mem::take(poisoned.into_inner()),
        };
        obsolete::remove_with_retry(&paths);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_memtable_shares_entries() {
        let mut entries = BTreeMap::new();
        entries.insert(b"a".to_vec(), Some(Bytes::from_static(b"one")));
        let table = ImmutableMemTable::new(entries, 7, 3, 4);
        let first = table.entries_arc();
        let second = table.entries_arc();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(table.get(b"a"), Some(Some(Bytes::from_static(b"one"))));
    }

    #[test]
    fn read_version_uses_shared_boundary_metadata() {
        let mut manifest = Manifest::default();
        let mut smallest = b"a".to_vec();
        smallest.extend_from_slice(&1_u64.to_le_bytes());
        let mut largest = b"z".to_vec();
        largest.extend_from_slice(&1_u64.to_le_bytes());
        manifest.table_files.push(TableFileMeta::native(
            2,
            1,
            100,
            smallest,
            largest,
        ));
        manifest.table_numbers.push(2);

        let version = ReadVersion::from_manifest(&manifest);
        assert_eq!(version.tables().len(), 1);
        assert_eq!(version.tables()[0].smallest_user_key(), Some(b"a".as_slice()));
    }
}
