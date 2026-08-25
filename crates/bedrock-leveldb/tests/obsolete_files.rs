use bedrock_leveldb::{Db, LevelDbOpenOptions, WriteOptions};
use std::fs;

#[test]
fn writable_open_reclaims_unreferenced_tables_and_logs_but_read_only_open_does_not() {
    let directory = tempfile::tempdir().expect("create temporary database directory");
    {
        let db = Db::open(directory.path(), LevelDbOpenOptions::default()).expect("open database");
        db.put("live", "value", WriteOptions::default())
            .expect("put live value");
        db.flush().expect("flush live value");
    }
    let obsolete_table = directory.path().join("999998.ldb");
    let obsolete_log = directory.path().join("999999.log");
    fs::write(&obsolete_table, b"obsolete table").expect("seed obsolete table");
    fs::write(&obsolete_log, b"obsolete log").expect("seed obsolete log");

    {
        let options = LevelDbOpenOptions {
            read_only: true,
            create_if_missing: false,
            ..LevelDbOpenOptions::default()
        };
        let _reader = Db::open(directory.path(), options).expect("open read-only database");
        assert!(obsolete_table.exists());
        assert!(obsolete_log.exists());
    }

    let db = Db::open(directory.path(), LevelDbOpenOptions::default()).expect("open writer");
    assert!(!obsolete_table.exists());
    assert!(!obsolete_log.exists());
    assert_eq!(
        db.get(b"live").expect("read live value").as_deref(),
        Some(b"value".as_slice())
    );
}
