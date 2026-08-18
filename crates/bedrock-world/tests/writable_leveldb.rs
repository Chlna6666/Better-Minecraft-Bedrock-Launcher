#![cfg(feature = "bedrock-leveldb")]

use bedrock_world::{BedrockLevelDbStorage, WorldStorage};
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_database_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "bedrock-world-writable-leveldb-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after the Unix epoch")
            .as_nanos()
    ))
}

#[test]
fn writable_world_keeps_large_edit_in_wal_without_full_database_table() {
    let database_path = temporary_database_path();
    std::fs::create_dir_all(&database_path).expect("create temporary database directory");
    drop(
        bedrock_leveldb::Db::open(&database_path, bedrock_leveldb::OpenOptions::default())
            .expect("initialize LevelDB"),
    );

    let edit = vec![0x5a; 5 * 1024 * 1024];
    let storage = BedrockLevelDbStorage::open(&database_path).expect("open writable storage");
    storage
        .put(b"large-map-edit", &edit)
        .expect("write large map edit");
    drop(storage);

    let table_count = std::fs::read_dir(&database_path)
        .expect("read database directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "ldb")
        })
        .count();
    assert_eq!(
        table_count, 0,
        "ordinary saves must not flush the whole world"
    );

    let reopened =
        BedrockLevelDbStorage::open_read_only(&database_path).expect("reopen read-only storage");
    assert_eq!(
        reopened
            .get(b"large-map-edit")
            .expect("read saved map edit")
            .as_deref(),
        Some(edit.as_slice())
    );
    drop(reopened);
    std::fs::remove_dir_all(database_path).expect("remove temporary database directory");
}
