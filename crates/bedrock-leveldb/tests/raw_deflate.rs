#![cfg(feature = "zlib")]

use bedrock_leveldb::{CompressionPolicy, Db, Options, WriteOptions};
use bytes::Bytes;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_db_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "bedrock-leveldb-raw-deflate-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ))
}

#[test]
fn default_raw_deflate_native_table_reopens() {
    let path = temp_db_path();
    let options = Options::default();
    assert_eq!(options.compression_policy, CompressionPolicy::RawDeflate);

    let db = Db::open(&path, options).expect("open writable db");
    let value = Bytes::from(vec![0x5a; 64 * 1024]);
    db.put(
        Bytes::from_static(b"bedrock:raw-deflate"),
        value.clone(),
        WriteOptions { sync: true },
    )
    .expect("write value");
    db.flush_memtable().expect("flush native table");
    drop(db);

    let reopened = Db::open(
        &path,
        Options {
            read_only: true,
            create_if_missing: false,
            ..Options::default()
        },
    )
    .expect("reopen raw-deflate db");
    assert_eq!(
        reopened.get(b"bedrock:raw-deflate").expect("read value"),
        Some(value)
    );
    drop(reopened);

    std::fs::remove_dir_all(path).expect("cleanup");
}
