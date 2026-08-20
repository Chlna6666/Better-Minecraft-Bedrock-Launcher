use bedrock_leveldb::{Db, LevelDbOpenOptions, WriteOptions};
use std::fs;
use std::path::PathBuf;

fn only_table(directory: &std::path::Path) -> PathBuf {
    fs::read_dir(directory)
        .expect("read database directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| path.extension().and_then(|extension| extension.to_str()) == Some("ldb"))
        .expect("database table")
}

#[test]
fn repair_ignores_valid_unreferenced_table_files() {
    let target = tempfile::tempdir().expect("create target database directory");
    {
        let db = Db::open(target.path(), LevelDbOpenOptions::default()).expect("open target");
        db.put("live", "current", WriteOptions::default()).expect("put live");
        db.flush().expect("flush live");
    }

    let stale = tempfile::tempdir().expect("create stale database directory");
    {
        let db = Db::open(stale.path(), LevelDbOpenOptions::default()).expect("open stale source");
        db.put("obsolete", "must-not-return", WriteOptions::default())
            .expect("put obsolete value");
        db.flush().expect("flush obsolete value");
    }
    fs::copy(only_table(stale.path()), target.path().join("900000.ldb"))
        .expect("copy unreferenced valid table");

    let report = Db::repair(target.path(), LevelDbOpenOptions::default()).expect("repair target");
    assert_eq!(report.recovered_tables, 1);

    let db = Db::open(target.path(), LevelDbOpenOptions::default()).expect("open repaired target");
    assert_eq!(db.get(b"live").expect("read live").as_deref(), Some(b"current".as_slice()));
    assert_eq!(db.get(b"obsolete").expect("read obsolete"), None);
}
