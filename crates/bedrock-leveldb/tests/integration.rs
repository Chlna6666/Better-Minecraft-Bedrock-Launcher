use bedrock_leveldb::{
    ChecksumMode, ChunkCoordinates, ChunkKey, ChunkRecordTag, CompressionPolicy, Db, Dimension,
    ErrorKind, LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN, LEGACY_TERRAIN_VALUE_LEN, LevelDbError,
    OpenOptions, ReadOptions, ScanCancelFlag, ScanMode, SubChunkIndex, VisitorControl,
    WriteOptions,
};
use bytes::Bytes;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
struct LogEvent {
    level: log::Level,
    message: String,
}

struct TestLogger {
    events: Mutex<Vec<LogEvent>>,
}

static TEST_LOGGER: TestLogger = TestLogger {
    events: Mutex::new(Vec::new()),
};
static LOGGER_INIT: OnceLock<()> = OnceLock::new();

impl log::Log for TestLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Trace
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.events.lock().expect("logger mutex").push(LogEvent {
                level: record.level(),
                message: record.args().to_string(),
            });
        }
    }

    fn flush(&self) {}
}

fn install_test_logger() -> bool {
    let mut installed = false;
    LOGGER_INIT.get_or_init(|| {
        log::set_logger(&TEST_LOGGER).expect("test logger is installed once");
        log::set_max_level(log::LevelFilter::Trace);
        installed = true;
    });
    installed
}

fn clear_logs() {
    TEST_LOGGER.events.lock().expect("logger mutex").clear();
}

fn captured_logs() -> Vec<LogEvent> {
    TEST_LOGGER.events.lock().expect("logger mutex").clone()
}

fn expect_error<T>(result: bedrock_leveldb::Result<T>) -> LevelDbError {
    match result {
        Ok(_) => panic!("expected error"),
        Err(error) => error,
    }
}

#[test]
fn read_only_missing_database_does_not_create_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing = temp.path().join("missing");

    let result = Db::open(
        &missing,
        OpenOptions {
            read_only: true,
            create_if_missing: true,
            ..OpenOptions::default()
        },
    );

    let error = expect_error(result);
    assert_eq!(error.kind(), ErrorKind::NotFound);
    assert_eq!(error.path(), Some(missing.as_path()));
    assert!(!missing.exists());
}

#[test]
fn read_only_handle_rejects_mutating_operations() {
    let temp = tempfile::tempdir().expect("tempdir");
    {
        let db = Db::open(
            temp.path(),
            OpenOptions {
                compression_policy: CompressionPolicy::None,
                ..OpenOptions::default()
            },
        )
        .expect("open writable");
        db.put(b"k".as_slice(), b"v".as_slice(), WriteOptions::default())
            .expect("put");
        db.flush().expect("flush");
    }

    let db = Db::open(
        temp.path(),
        OpenOptions {
            read_only: true,
            create_if_missing: false,
            compression_policy: CompressionPolicy::None,
            ..OpenOptions::default()
        },
    )
    .expect("open read-only");

    assert_eq!(expect_error(db.flush()).kind(), ErrorKind::ReadOnly);
    assert_eq!(
        expect_error(db.put(b"k2".as_slice(), b"v2".as_slice(), WriteOptions::default())).kind(),
        ErrorKind::ReadOnly
    );
}

#[test]
fn get_many_owned_reads_render_record_keys_in_order() {
    let temp = tempfile::tempdir().expect("tempdir");
    let legacy_key = Bytes::from(
        ChunkKey::new(
            ChunkCoordinates::new(0, 0),
            Dimension::Overworld,
            ChunkRecordTag::LegacyTerrain,
        )
        .encode(),
    );
    let legacy_subchunk_key = Bytes::from(
        ChunkKey::new_subchunk(
            ChunkCoordinates::new(0, 0),
            Dimension::Overworld,
            SubChunkIndex::from_raw(0),
        )
        .encode(),
    );
    let modern_subchunk_key = Bytes::from(
        ChunkKey::new_subchunk(
            ChunkCoordinates::new(1, 0),
            Dimension::Overworld,
            SubChunkIndex::from_raw(0),
        )
        .encode(),
    );
    let other_key = Bytes::from_static(b"~local_player");
    let terrain = Bytes::from(vec![7_u8; LEGACY_TERRAIN_VALUE_LEN]);
    let mut legacy_subchunk_bytes = vec![0_u8; LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN];
    legacy_subchunk_bytes[0] = 2;
    legacy_subchunk_bytes[1 + 1_125] = 99;
    let legacy_subchunk = Bytes::from(legacy_subchunk_bytes);
    let modern_subchunk = Bytes::from_static(b"\x08\x00modern-paletted-fixture");
    {
        let db = Db::open(
            temp.path(),
            OpenOptions {
                compression_policy: CompressionPolicy::None,
                ..OpenOptions::default()
            },
        )
        .expect("open writable");
        db.put(legacy_key.clone(), terrain.clone(), WriteOptions::default())
            .expect("put legacy terrain");
        db.put(
            legacy_subchunk_key.clone(),
            legacy_subchunk.clone(),
            WriteOptions::default(),
        )
        .expect("put legacy subchunk");
        db.put(
            modern_subchunk_key.clone(),
            modern_subchunk.clone(),
            WriteOptions::default(),
        )
        .expect("put modern subchunk");
        db.put(
            other_key.clone(),
            b"player".as_slice(),
            WriteOptions::default(),
        )
        .expect("put other");
        db.flush().expect("flush");
    }

    let db = Db::open(
        temp.path(),
        OpenOptions {
            read_only: true,
            create_if_missing: false,
            compression_policy: CompressionPolicy::None,
            ..OpenOptions::default()
        },
    )
    .expect("open read-only");
    let values = db
        .get_many_owned(
            vec![
                Bytes::from_static(b"missing"),
                legacy_key.clone(),
                legacy_subchunk_key,
                modern_subchunk_key,
                other_key.clone(),
                legacy_key,
            ],
            ReadOptions::default(),
        )
        .expect("get many");

    assert!(values[0].is_none());
    assert_eq!(values[1], Some(terrain.clone()));
    assert_eq!(values[2], Some(legacy_subchunk));
    assert_eq!(values[3], Some(modern_subchunk));
    assert_eq!(values[4], Some(Bytes::from_static(b"player")));
    assert_eq!(values[5], Some(terrain));
}

#[test]
fn repair_rejects_read_only_options() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing = temp.path().join("repair-target");

    let result = Db::repair(
        &missing,
        OpenOptions {
            read_only: true,
            create_if_missing: true,
            ..OpenOptions::default()
        },
    );

    assert_eq!(expect_error(result).kind(), ErrorKind::ReadOnly);
    assert!(!missing.exists());
}

#[test]
fn delete_tombstone_survives_reopen() {
    let temp = tempfile::tempdir().expect("tempdir");
    let options = OpenOptions {
        compression_policy: CompressionPolicy::None,
        ..OpenOptions::default()
    };

    {
        let db = Db::open(temp.path(), options.clone()).expect("open");
        db.put(b"k".as_slice(), b"old".as_slice(), WriteOptions::default())
            .expect("put");
        db.delete(b"k".as_slice(), WriteOptions::default())
            .expect("delete");
    }

    let db = Db::open(temp.path(), options).expect("reopen");
    assert_eq!(db.get(b"k").expect("get"), None);
}

#[test]
fn flushed_native_table_reopens_and_scans() {
    let temp = tempfile::tempdir().expect("tempdir");
    let options = OpenOptions {
        compression_policy: CompressionPolicy::None,
        write_buffer_size: 1,
        ..OpenOptions::default()
    };

    {
        let db = Db::open(temp.path(), options.clone()).expect("open");
        db.put(
            b"chunk:1".as_slice(),
            b"one".as_slice(),
            WriteOptions::default(),
        )
        .expect("put one");
        db.put(
            b"chunk:2".as_slice(),
            b"two".as_slice(),
            WriteOptions::default(),
        )
        .expect("put two");
    }

    let db = Db::open(temp.path(), options).expect("reopen");
    assert_eq!(
        db.get(b"chunk:1").expect("get"),
        Some(Bytes::from_static(b"one"))
    );

    let mut values = Vec::new();
    db.for_each_prefix(b"chunk:", ReadOptions::default(), |key, value| {
        values.push((Bytes::copy_from_slice(key), value.clone()));
        Ok(VisitorControl::Continue)
    })
    .expect("scan");
    values.sort();
    assert_eq!(values.len(), 2);

    let table_path = std::fs::read_dir(temp.path())
        .expect("read dir")
        .map(|entry| entry.expect("entry").path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("ldb"))
        .expect("native table file");
    let table_bytes = std::fs::read(&table_path).expect("read native table");
    assert_ne!(&table_bytes[..9], b"BWLDBTBL1");

    let manifest_name = std::fs::read_to_string(temp.path().join("CURRENT")).expect("CURRENT");
    let manifest_bytes =
        std::fs::read(temp.path().join(manifest_name.trim())).expect("read manifest");
    assert_ne!(&manifest_bytes[..9], b"BWLDBMAN1");
}

#[test]
fn checksum_verification_detects_native_table_corruption() {
    let temp = tempfile::tempdir().expect("tempdir");
    let options = OpenOptions {
        compression_policy: CompressionPolicy::None,
        write_buffer_size: 1,
        ..OpenOptions::default()
    };

    {
        let db = Db::open(temp.path(), options.clone()).expect("open");
        db.put(
            b"k".as_slice(),
            b"value".as_slice(),
            WriteOptions::default(),
        )
        .expect("put");
    }

    let table_path = std::fs::read_dir(temp.path())
        .expect("read dir")
        .map(|entry| entry.expect("entry").path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("ldb"))
        .expect("table file");
    let mut bytes = std::fs::read(&table_path).expect("read table");
    let last = bytes.last_mut().expect("non-empty table");
    *last ^= 0xff;
    std::fs::write(&table_path, bytes).expect("write corrupt table");

    let db = Db::open(temp.path(), options).expect("reopen");
    let result = db.get_with(
        b"k",
        ReadOptions {
            checksum: ChecksumMode::Verify,
            ..ReadOptions::default()
        },
    );
    let error = expect_error(result);
    assert_eq!(error.kind(), ErrorKind::Corruption);
    assert_eq!(error.path(), Some(table_path.as_path()));
}

#[test]
fn scan_cancellation_returns_cancelled_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = Db::open(temp.path(), OpenOptions::default()).expect("open");
    db.put(b"k".as_slice(), b"v".as_slice(), WriteOptions::default())
        .expect("put");

    let cancel = ScanCancelFlag::new();
    cancel.cancel();
    let result = db.for_each_key(
        ReadOptions {
            cancel: Some(cancel),
            ..ReadOptions::default()
        },
        |_key| Ok(VisitorControl::Continue),
    );

    let error = expect_error(result);
    assert_eq!(error.kind(), ErrorKind::Cancelled);
}

#[test]
fn missing_manifest_error_carries_path_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest_path = temp.path().join("MANIFEST-000001");
    std::fs::write(temp.path().join("CURRENT"), "MANIFEST-000001\n").expect("write CURRENT");

    let result = Db::open(
        temp.path(),
        OpenOptions {
            read_only: true,
            create_if_missing: false,
            ..OpenOptions::default()
        },
    );

    let error = expect_error(result);
    assert_eq!(error.kind(), ErrorKind::Io);
    assert_eq!(error.path(), Some(manifest_path.as_path()));
}

#[test]
fn repair_warns_when_discarding_unreadable_files_and_library_does_not_init_logger() {
    let temp = tempfile::tempdir().expect("tempdir");
    let pre_logger_path = temp.path().join("pre_logger_open");
    let _db = Db::open(&pre_logger_path, OpenOptions::default()).expect("open before logger");
    assert!(install_test_logger());
    clear_logs();

    let corrupt_table = temp.path().join("000123.ldb");
    std::fs::write(&corrupt_table, b"not a leveldb table").expect("write corrupt table");
    let report = Db::repair(
        temp.path(),
        OpenOptions {
            compression_policy: CompressionPolicy::None,
            ..OpenOptions::default()
        },
    )
    .expect("repair");

    assert_eq!(report.dropped_files, 1);
    assert!(captured_logs().iter().any(|event| {
        event.level == log::Level::Warn
            && event
                .message
                .contains("dropping unreadable table during repair")
            && event.message.contains("000123.ldb")
    }));
}

#[test]
fn parallel_scan_matches_sequential_scan_in_integration_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let options = OpenOptions {
        compression_policy: CompressionPolicy::None,
        write_buffer_size: 1,
        ..OpenOptions::default()
    };
    let db = Db::open(temp.path(), options).expect("open");
    for index in 0..64 {
        db.put(
            Bytes::from(format!("key:{index:03}")),
            Bytes::from(format!("value:{index:03}")),
            WriteOptions::default(),
        )
        .expect("put");
    }

    let mut sequential = Vec::new();
    db.for_each_key(ReadOptions::default(), |key| {
        sequential.push(Bytes::copy_from_slice(key));
        Ok(VisitorControl::Continue)
    })
    .expect("sequential");

    let mut parallel = Vec::new();
    db.for_each_key(
        ReadOptions {
            scan_mode: ScanMode::ParallelTables,
            ..ReadOptions::default()
        },
        |key| {
            parallel.push(Bytes::copy_from_slice(key));
            Ok(VisitorControl::Continue)
        },
    )
    .expect("parallel");

    sequential.sort();
    parallel.sort();
    assert_eq!(parallel, sequential);
}

#[cfg(not(feature = "zlib"))]
#[test]
fn zlib_native_writes_require_zlib_feature() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = Db::open(
        temp.path(),
        OpenOptions {
            compression_policy: CompressionPolicy::Zlib,
            write_buffer_size: 1,
            ..OpenOptions::default()
        },
    )
    .expect("open");

    let result = db.put(b"k".as_slice(), b"v".as_slice(), WriteOptions::default());
    assert_eq!(expect_error(result).kind(), ErrorKind::Unsupported);
}

#[cfg(not(feature = "snappy"))]
#[test]
fn snappy_native_writes_require_snappy_feature() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = Db::open(
        temp.path(),
        OpenOptions {
            compression_policy: CompressionPolicy::Snappy,
            write_buffer_size: 1,
            ..OpenOptions::default()
        },
    )
    .expect("open");

    let result = db.put(b"k".as_slice(), b"v".as_slice(), WriteOptions::default());
    assert_eq!(expect_error(result).kind(), ErrorKind::Unsupported);
}
