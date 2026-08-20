use bedrock_leveldb::{
    CompressionPolicy, Db, LevelDbOpenOptions, ReadOptions, ReadStrategy, ScanMode,
    VisitorControl, WriteOptions,
};
use std::fs;

fn table_sizes(directory: &std::path::Path) -> Vec<u64> {
    let mut sizes = fs::read_dir(directory)
        .expect("read database directory")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("ldb"))
        .map(|entry| entry.metadata().expect("read table metadata").len())
        .collect::<Vec<_>>();
    sizes.sort_unstable();
    sizes
}

#[test]
fn every_scan_api_reconciles_newer_values_and_tombstones() {
    let directory = tempfile::tempdir().expect("create temporary database directory");
    let options = LevelDbOpenOptions {
        write_buffer_size: 0,
        ..LevelDbOpenOptions::default()
    };
    let db = Db::open(directory.path(), options).expect("open database");
    db.put("key", "old", WriteOptions::default()).expect("put old");
    db.flush().expect("flush old");
    db.put("key", "new", WriteOptions::default()).expect("put new");
    db.flush().expect("flush new");

    for scan_mode in [ScanMode::Sequential, ScanMode::ParallelTables] {
        let read_options = ReadOptions {
            scan_mode,
            read_strategy: ReadStrategy::Borrowed,
            ..ReadOptions::default()
        };
        let mut values = Vec::new();
        db.for_each_entry_ref(read_options.clone(), |entry| {
            if entry.key.as_bytes() == b"key" {
                values.push(entry.value.as_bytes().to_vec());
            }
            Ok(VisitorControl::Continue)
        })
        .expect("scan entry refs");
        assert_eq!(values, [b"new".to_vec()]);

        let mut keys = Vec::new();
        db.for_each_key(read_options.clone(), |key| {
            keys.push(key.to_vec());
            Ok(VisitorControl::Continue)
        })
        .expect("scan keys");
        assert_eq!(keys.iter().filter(|key| key.as_slice() == b"key").count(), 1);

        let (_, partitions) = db
            .scan_entries_partitioned(read_options, Vec::new, |entries, key, value| {
                if key == b"key" {
                    entries.push(value.to_vec());
                }
                Ok(VisitorControl::Continue)
            })
            .expect("scan partitioned entries");
        assert_eq!(partitions.into_iter().flatten().collect::<Vec<_>>(), [b"new".to_vec()]);
    }

    db.delete("key", WriteOptions::default()).expect("delete key");
    db.flush().expect("flush tombstone");
    let mut keys = Vec::new();
    db.for_each_key(ReadOptions::default(), |key| {
        keys.push(key.to_vec());
        Ok(VisitorControl::Continue)
    })
    .expect("scan keys after delete");
    assert!(!keys.iter().any(|key| key.as_slice() == b"key"));
}

#[test]
fn flush_writes_only_memtable_and_preserves_tombstones_across_reopen() {
    let directory = tempfile::tempdir().expect("create temporary database directory");
    let options = LevelDbOpenOptions {
        compression_policy: CompressionPolicy::None,
        write_buffer_size: 0,
        ..LevelDbOpenOptions::default()
    };

    {
        let db = Db::open(directory.path(), options.clone()).expect("open database");
        db.put("large", vec![7_u8; 256 * 1024], WriteOptions::default())
            .expect("write large value");
        db.flush().expect("flush first memtable");
        let first_sizes = table_sizes(directory.path());
        assert_eq!(first_sizes.len(), 1);

        db.put("small", "value", WriteOptions::default())
            .expect("write small value");
        db.flush().expect("flush second memtable");
        let second_sizes = table_sizes(directory.path());
        assert_eq!(second_sizes.len(), 2);
        assert_eq!(second_sizes[1], first_sizes[0]);
        assert!(second_sizes[0] < first_sizes[0] / 8);

        db.delete("large", WriteOptions::default())
            .expect("delete large value");
        db.flush().expect("flush tombstone");
        assert_eq!(db.stats_fast().expect("read stats").tables, 3);
        assert_eq!(db.get(b"large").expect("read deleted key"), None);
    }

    let db = Db::open(directory.path(), options).expect("reopen database");
    assert_eq!(db.get(b"large").expect("read tombstoned key"), None);
    assert_eq!(db.get(b"small").expect("read small key").as_deref(), Some(b"value".as_slice()));
}
