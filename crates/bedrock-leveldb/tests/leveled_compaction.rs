use bedrock_leveldb::{CompressionPolicy, Db, LevelDbOpenOptions, WriteOptions};

#[test]
fn level_zero_trigger_compacts_incremental_tables_without_losing_values() {
    let directory = tempfile::tempdir().expect("create temporary database directory");
    let options = LevelDbOpenOptions {
        compression_policy: CompressionPolicy::None,
        write_buffer_size: 0,
        ..LevelDbOpenOptions::default()
    };
    {
        let db = Db::open(directory.path(), options.clone()).expect("open database");
        for index in 0..4 {
            db.put(
                format!("chunk:{index}"),
                format!("value:{index}"),
                WriteOptions::default(),
            )
            .expect("write incremental value");
            db.flush().expect("flush level-zero table");
        }
        assert_eq!(db.stats_fast().expect("read stats").tables, 1);
    }

    let db = Db::open(directory.path(), options).expect("reopen compacted database");
    for index in 0..4 {
        assert_eq!(
            db.get(format!("chunk:{index}").as_bytes())
                .expect("read compacted value")
                .as_deref(),
            Some(format!("value:{index}").as_bytes())
        );
    }
}

#[test]
fn explicit_compaction_keeps_newest_value_and_reclaims_bottom_level_tombstone() {
    let directory = tempfile::tempdir().expect("create temporary database directory");
    let options = LevelDbOpenOptions {
        write_buffer_size: 0,
        ..LevelDbOpenOptions::default()
    };
    let db = Db::open(directory.path(), options).expect("open database");
    db.put("actor", "old", WriteOptions::default()).expect("put old");
    db.flush().expect("flush old");
    db.put("actor", "new", WriteOptions::default()).expect("put new");
    db.flush().expect("flush new");
    db.delete("actor", WriteOptions::default()).expect("delete actor");
    db.flush().expect("flush tombstone");

    db.compact().expect("compact all levels");
    assert_eq!(db.get(b"actor").expect("read actor"), None);
    assert_eq!(db.stats_fast().expect("read stats").tables, 0);
}
