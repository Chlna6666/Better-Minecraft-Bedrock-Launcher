//! Read-only benchmark for a pinned Minecraft Bedrock world database snapshot.

use bedrock_leveldb::{
    CachePolicy, Db, LevelDbOpenOptions, ReadOptions, ScanMode, ThreadingOptions, VisitorControl,
};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use xxhash_rust::xxh3::Xxh3;

const SAMPLE_COUNT: usize = 7;

fn main() {
    let fixture_root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("sample-bedrock-world")
        });
    let db_path = if fixture_root.join("db").is_dir() {
        fixture_root.join("db")
    } else {
        fixture_root.clone()
    };
    if !db_path.join("CURRENT").exists() {
        eprintln!(
            "read-only LevelDB fixture is missing at {}; benchmark skipped",
            db_path.display()
        );
        return;
    }

    let hash_root = db_path.parent().unwrap_or(&db_path);
    let (fixture_hash_before, fixture_bytes) = fixture_hash(hash_root).expect("fixture hash");
    println!(
        "leveldb_fixture.snapshot path={} fixture_hash={fixture_hash_before:032x} fixture_bytes={fixture_bytes} os={} arch={} cpu_parallelism={} chunks=not_applicable parse_errors=not_applicable",
        hash_root.display(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(1, usize::from)
    );

    measure_condition(&db_path, "logical_cold", CachePolicy::Bypass, false);
    measure_condition(&db_path, "logical_warm", CachePolicy::Use, true);

    let (fixture_hash_after, fixture_bytes_after) =
        fixture_hash(hash_root).expect("fixture hash after benchmark");
    assert_eq!(fixture_hash_after, fixture_hash_before, "fixture changed");
    assert_eq!(fixture_bytes_after, fixture_bytes, "fixture size changed");
}

fn measure_condition(db_path: &Path, label: &str, cache_policy: CachePolicy, reuse_handle: bool) {
    let shared = reuse_handle.then(|| open_read_only(db_path));
    if let Some(db) = &shared {
        let _ = scan_once(db, cache_policy);
    }
    let mut elapsed = Vec::with_capacity(SAMPLE_COUNT);
    let mut records = 0_usize;
    let mut bytes = 0_usize;
    for _ in 0..SAMPLE_COUNT {
        let owned;
        let db = if let Some(shared) = &shared {
            shared
        } else {
            owned = open_read_only(db_path);
            &owned
        };
        let started = Instant::now();
        let (sample_records, sample_bytes) = scan_once(db, cache_policy);
        elapsed.push(started.elapsed());
        records = sample_records;
        bytes = sample_bytes;
    }
    elapsed.sort_unstable();
    let p50 = percentile(&elapsed, 50);
    let p95 = percentile(&elapsed, 95);
    println!(
        "leveldb_fixture.scan cache={label} samples={SAMPLE_COUNT} records={records} bytes={bytes} p50_ms={:.3} p95_ms={:.3} records_per_sec={:.2} mib_per_sec={:.2}",
        p50.as_secs_f64() * 1_000.0,
        p95.as_secs_f64() * 1_000.0,
        records as f64 / p50.as_secs_f64(),
        bytes as f64 / (1024.0 * 1024.0) / p50.as_secs_f64(),
    );
}

fn open_read_only(path: &Path) -> Db {
    Db::open(
        path,
        LevelDbOpenOptions {
            read_only: true,
            create_if_missing: false,
            ..LevelDbOpenOptions::default()
        },
    )
    .expect("open read-only Mojang LevelDB")
}

fn scan_once(db: &Db, cache_policy: CachePolicy) -> (usize, usize) {
    let mut bytes = 0_usize;
    let outcome = db
        .for_each_entry(
            ReadOptions {
                cache_policy,
                threading: ThreadingOptions::Single,
                scan_mode: ScanMode::Sequential,
                ..ReadOptions::default()
            },
            |key, value| {
                bytes = bytes.saturating_add(key.len()).saturating_add(value.len());
                Ok(VisitorControl::Continue)
            },
        )
        .expect("scan read-only fixture");
    (outcome.visited, bytes)
}

fn percentile(samples: &[Duration], percentile: usize) -> Duration {
    let index = (samples.len() * percentile).div_ceil(100).saturating_sub(1);
    samples[index.min(samples.len() - 1)]
}

fn fixture_hash(root: &Path) -> std::io::Result<(u128, u64)> {
    let mut files = Vec::new();
    collect_files(root, &mut files)?;
    files.sort();
    let mut hash = Xxh3::new();
    let mut total_bytes = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    for path in files {
        let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
        hash.update(relative.as_bytes());
        let mut file = File::open(&path)?;
        let len = file.metadata()?.len();
        total_bytes = total_bytes.saturating_add(len);
        hash.update(&len.to_le_bytes());
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
    }
    Ok((hash.digest128(), total_bytes))
}

fn collect_files(root: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(&path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}
