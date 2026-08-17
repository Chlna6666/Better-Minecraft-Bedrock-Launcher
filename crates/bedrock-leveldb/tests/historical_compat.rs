use bedrock_leveldb::{Db, OpenOptions, ReadOptions, VisitorControl};
use std::path::{Path, PathBuf};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn open_fixture_if_present(name: &str) -> Option<Db> {
    let path = fixture_root().join(name);
    if !path.join("CURRENT").is_file() {
        eprintln!("skipping local Mojang LevelDB fixture {name}: {} is absent", path.display());
        return None;
    }
    Some(
        Db::open(
            &path,
            OpenOptions {
                read_only: true,
                create_if_missing: false,
                error_if_exists: false,
                ..OpenOptions::default()
            },
        )
        .unwrap_or_else(|error| panic!("open historical fixture {name}: {error}")),
    )
}

fn assert_fixture_scans(name: &str) {
    let Some(db) = open_fixture_if_present(name) else {
        return;
    };
    let mut visited = 0usize;
    db.for_each_entry(ReadOptions::default(), |_key, _value| {
        visited = visited.saturating_add(1);
        Ok(VisitorControl::Continue)
    })
    .unwrap_or_else(|error| panic!("scan historical fixture {name}: {error}"));
    assert!(visited != 0, "historical fixture {name} unexpectedly contains no visible records");
}

#[test]
fn opens_uncompressed_mojang_tables() {
    assert_fixture_scans("native-none");
}

#[test]
fn opens_snappy_mojang_tables() {
    assert_fixture_scans("native-snappy");
}

#[test]
fn opens_zlib_mojang_tables() {
    assert_fixture_scans("native-zlib");
}

#[test]
fn opens_bedrock_raw_deflate_tables() {
    assert_fixture_scans("native-bedrock-raw-deflate");
}

#[test]
fn replays_historical_wal_without_mutating_fixture() {
    assert_fixture_scans("legacy-wal-replay");
}

#[test]
fn opens_multi_table_historical_database() {
    assert_fixture_scans("multi-table-compaction");
}
