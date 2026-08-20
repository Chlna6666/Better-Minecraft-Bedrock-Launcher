#![cfg(feature = "bedrock-leveldb")]

use bedrock_world::{BedrockWorld, WorldScanOptions};
use std::env;
use std::path::{Path, PathBuf};

const HISTORICAL_WORLD_FIXTURES: &[&str] = &[
    "bedrock-0.6.1",
    "bedrock-0.14",
    "bedrock-0.16",
    "bedrock-1.0",
    "bedrock-1.12",
    "bedrock-1.13",
    "bedrock-1.16",
    "bedrock-1.17",
    "bedrock-1.18.0",
    "bedrock-1.18.30",
    "bedrock-1.19",
    "bedrock-1.20",
    "bedrock-1.21",
    "bedrock-1.26",
    "future-unknown",
];

const FIXTURE_ROOT_ENV: &str = "BEDROCK_WORLD_FIXTURE_ROOT";
const REQUIRE_FIXTURES_ENV: &str = "BEDROCK_WORLD_REQUIRE_HISTORICAL_FIXTURES";

fn fixture_root() -> PathBuf {
    env::var_os(FIXTURE_ROOT_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"))
}

fn require_historical_fixtures() -> bool {
    env::var(REQUIRE_FIXTURES_ENV).ok().is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn fixture_path(name: &str) -> PathBuf {
    fixture_root().join(name)
}

fn is_world_fixture(path: &Path) -> bool {
    path.join("level.dat").is_file()
        && (path.join("db").join("CURRENT").is_file() || path.join("chunks.dat").is_file())
}

fn available_fixture(name: &str) -> Option<PathBuf> {
    let path = fixture_path(name);
    if is_world_fixture(&path) {
        return Some(path);
    }
    if require_historical_fixtures() {
        panic!(
            "required Bedrock world fixture {name} is absent or incomplete at {}; expected level.dat plus db/CURRENT or chunks.dat",
            path.display()
        );
    }
    eprintln!(
        "skipping historical Bedrock world fixture {name}: {} is absent or incomplete; set {REQUIRE_FIXTURES_ENV}=1 to enforce the corpus",
        path.display()
    );
    None
}

#[test]
fn required_historical_world_matrix_is_complete() {
    if !require_historical_fixtures() {
        return;
    }
    let missing = HISTORICAL_WORLD_FIXTURES
        .iter()
        .filter(|name| !is_world_fixture(&fixture_path(name)))
        .copied()
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "historical Bedrock world corpus is incomplete under {}: missing {missing:?}",
        fixture_root().display()
    );
}

#[test]
fn historical_world_matrix_opens_and_exposes_persisted_records() {
    for name in HISTORICAL_WORLD_FIXTURES {
        let Some(path) = available_fixture(name) else {
            continue;
        };
        let world = BedrockWorld::open_auto_blocking(&path)
            .unwrap_or_else(|error| panic!("open historical world fixture {name}: {error}"));
        let level = world.read_level_dat_blocking().unwrap_or_else(|error| {
            panic!("read level.dat for historical fixture {name}: {error}")
        });
        assert!(
            level.header.actual_payload_len != 0,
            "historical fixture {name} has an empty level.dat payload"
        );

        let versions = world.versions_blocking().unwrap_or_else(|error| {
            panic!("scan version evidence for historical fixture {name}: {error}")
        });
        assert_eq!(
            versions.world_format,
            world.format(),
            "historical fixture {name} version evidence disagrees with detected world format"
        );

        let key_counts = world
            .classify_keys_blocking(WorldScanOptions::default())
            .unwrap_or_else(|error| panic!("classify keys for historical fixture {name}: {error}"));
        let visible_keys = key_counts.values().copied().sum::<usize>();
        assert!(
            visible_keys != 0,
            "historical fixture {name} exposes no persisted world records"
        );

        let _chunk_positions = world
            .list_chunk_positions_blocking(WorldScanOptions::default())
            .unwrap_or_else(|error| {
                panic!("list chunk positions for historical fixture {name}: {error}")
            });

        if *name == "future-unknown" {
            assert!(
                versions.has_future_storage(),
                "future-unknown fixture must expose at least one unknown/future persisted record"
            );
        }
    }
}
