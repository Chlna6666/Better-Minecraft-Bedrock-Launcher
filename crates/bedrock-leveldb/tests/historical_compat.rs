use bedrock_leveldb::{Db, OpenOptions, ReadOptions, VisitorControl};
use std::env;
use std::path::{Path, PathBuf};

const HISTORICAL_FIXTURES: &[&str] = &[
    "native-none",
    "native-snappy",
    "native-zlib",
    "native-bedrock-raw-deflate",
    "legacy-wal-replay",
    "multi-table-compaction",
];

const FIXTURE_ROOT_ENV: &str = "BEDROCK_LEVELDB_FIXTURE_ROOT";
const REQUIRE_FIXTURES_ENV: &str = "BEDROCK_LEVELDB_REQUIRE_HISTORICAL_FIXTURES";

fn fixture_root() -> PathBuf {
    env::var_os(FIXTURE_ROOT_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"))
}

fn require_historical_fixtures() -> bool {
    env::var(REQUIRE_FIXTURES_ENV)
        .ok()
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

fn fixture_path(name: &str) -> PathBuf {
    fixture_root().join(name)
}

fn open_fixture_if_present(name: &str) -> Option<Db> {
    let path = fixture_path(name);
    if !path.join("CURRENT").is_file() {
        if require_historical_fixtures() {
            panic!(
                "required Mojang LevelDB fixture {name} is absent at {}; set {FIXTURE_ROOT_ENV} to the corpus root or provide the fixture",
                path.display()
            );
        }
        eprintln!(
            "skipping local Mojang LevelDB fixture {name}: {} is absent; set {REQUIRE_FIXTURES_ENV}=1 to make missing corpus entries fail",
            path.display()
        );
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
    assert!(
        visited != 0,
        "historical fixture {name} unexpectedly contains no visible records"
    );
}

#[test]
fn required_historical_fixture_matrix_is_complete() {
    if !require_historical_fixtures() {
        return;
    }
    let missing = HISTORICAL_FIXTURES
        .iter()
        .filter(|name| !fixture_path(name).join("CURRENT").is_file())
        .copied()
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "historical Mojang LevelDB corpus is incomplete under {}: missing {missing:?}",
        fixture_root().display()
    );
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
