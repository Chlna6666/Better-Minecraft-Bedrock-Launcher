use bedrock_world::integrity::scan_world_compatibility_blocking;
use bedrock_world::nbt::NbtView;
use bedrock_world::{
    BedrockLevelDbStorage, BedrockWorld, BedrockWorldOpenOptions, BiomeDataRequirement,
    ChunkDataRequest, ChunkLoadOptions, ChunkPos, Dimension, ExactSurfaceSubchunkPolicy,
    StorageCachePolicy, StorageReadOptions, StorageScanMode, StorageThreadingOptions,
    StorageVisitorControl, WorldFormat, WorldParseOptions, WorldScanOptions, WorldStorage,
    WorldThreadingOptions, read_level_dat_document,
};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use walkdir::WalkDir;
use xxhash_rust::xxh3::Xxh3;

fn fixture_world_path() -> PathBuf {
    if let Some(path) = std::env::args_os().nth(1) {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample-bedrock-world")
}

#[allow(clippy::too_many_lines)]
fn main() {
    let world_path = fixture_world_path();
    if !world_path.join("level.dat").exists() || !world_path.join("db").exists() {
        eprintln!(
            "large fixture is missing at {}; benchmark skipped",
            world_path.display()
        );
        return;
    }

    let (fixture_hash_before, fixture_bytes) = fixture_hash(&world_path).expect("fixture hash");
    println!(
        "large_fixture.snapshot path={} fixture_hash={fixture_hash_before:032x} fixture_bytes={fixture_bytes} os={} arch={} cpu_parallelism={}",
        world_path.display(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(1, usize::from)
    );

    let start = Instant::now();
    let level_dat = read_level_dat_document(&world_path.join("level.dat")).expect("read level.dat");
    println!(
        "large_fixture.level_dat elapsed_ms={} version={} payload_len={}",
        start.elapsed().as_millis(),
        level_dat.header.version,
        level_dat.header.actual_payload_len
    );

    let start = Instant::now();
    let storage = BedrockLevelDbStorage::open_read_only(world_path.join("db")).expect("open db");
    println!(
        "large_fixture.db.open_lazy elapsed_ms={} mmap_enabled={}",
        start.elapsed().as_millis(),
        cfg!(feature = "leveldb-mmap")
    );

    let dynamic_world = BedrockWorld::from_storage(
        world_path.clone(),
        Arc::new(storage.clone()) as Arc<dyn WorldStorage>,
        BedrockWorldOpenOptions::default(),
    );
    let generic_world = BedrockWorld::from_typed_storage(
        world_path.clone(),
        storage.clone(),
        BedrockWorldOpenOptions::default(),
    );

    let compatibility = scan_world_compatibility_blocking(
        &storage,
        WorldFormat::LevelDb,
        StorageReadOptions {
            threading: StorageThreadingOptions::Single,
            scan_mode: StorageScanMode::Sequential,
            ..StorageReadOptions::default()
        },
    )
    .expect("compatibility scan");
    let parse_errors = dynamic_world
        .parse_world_blocking(WorldParseOptions::summary())
        .expect("parse world summary")
        .report
        .parse_errors
        .len();
    println!(
        "large_fixture.compatibility records={} chunks={} parse_errors={} corrupt_chunks={} subchunk_versions={:?}",
        compatibility.records_scanned,
        compatibility.chunks_scanned,
        parse_errors,
        compatibility.corrupt_chunks,
        compatibility.subchunk_versions
    );

    let options = WorldScanOptions {
        threading: WorldThreadingOptions::Single,
        ..WorldScanOptions::default()
    };
    let start = Instant::now();
    let key_kinds = dynamic_world
        .classify_keys_blocking(options)
        .expect("classify keys");
    let elapsed = start.elapsed();
    let total_entries = key_kinds.values().copied().sum::<usize>();
    println!(
        "large_fixture.classify_keys.single elapsed_ms={} entries={} entries_per_sec={:.2}",
        elapsed.as_millis(),
        total_entries,
        u32::try_from(total_entries).map_or(f64::INFINITY, f64::from) / elapsed.as_secs_f64()
    );

    let start = Instant::now();
    let key_scan = storage
        .for_each_key(
            StorageReadOptions {
                threading: StorageThreadingOptions::Single,
                scan_mode: StorageScanMode::Sequential,
                ..StorageReadOptions::default()
            },
            &mut |_key| Ok(StorageVisitorControl::Continue),
        )
        .expect("key scan");
    println!(
        "large_fixture.key_scan.generic elapsed_ms={} entries={} entries_per_sec={:.2} worker_threads={} tables_scanned={}",
        start.elapsed().as_millis(),
        key_scan.visited,
        u32::try_from(key_scan.visited).map_or(f64::INFINITY, f64::from)
            / start.elapsed().as_secs_f64(),
        key_scan.worker_threads,
        key_scan.tables_scanned
    );

    let start = Instant::now();
    let prefix_scan = storage
        .for_each_prefix_ref(
            b"player_",
            StorageReadOptions {
                threading: StorageThreadingOptions::Single,
                scan_mode: StorageScanMode::Sequential,
                ..StorageReadOptions::default()
            },
            &mut |_entry| Ok(StorageVisitorControl::Continue),
        )
        .expect("prefix ref scan");
    println!(
        "large_fixture.prefix_ref_scan.players elapsed_ms={} entries={} entries_per_sec={:.2} worker_threads={} prefix_scans=1",
        start.elapsed().as_millis(),
        prefix_scan.visited,
        u32::try_from(prefix_scan.visited).map_or(f64::INFINITY, f64::from)
            / start.elapsed().as_secs_f64(),
        prefix_scan.worker_threads
    );

    let start = Instant::now();
    let players = dynamic_world.list_players_blocking().expect("list players");
    println!(
        "large_fixture.players.dynamic elapsed_ms={} count={}",
        start.elapsed().as_millis(),
        players.len()
    );

    let start = Instant::now();
    let generic_players = generic_world
        .list_players_blocking()
        .expect("list players generic");
    println!(
        "large_fixture.players.generic elapsed_ms={} count={}",
        start.elapsed().as_millis(),
        generic_players.len()
    );

    let pos = compatibility
        .chunks
        .first()
        .map(|chunk| chunk.pos)
        .unwrap_or(ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        });
    let start = Instant::now();
    let chunk = dynamic_world
        .parse_chunk_blocking(pos)
        .expect("parse chunk");
    println!(
        "large_fixture.sample_chunk elapsed_ms={} records={} subchunks={} block_entities={} parse_errors={}",
        start.elapsed().as_millis(),
        chunk.report.entry_count,
        chunk.report.subchunk_count,
        chunk.report.block_entity_count,
        chunk.report.parse_errors.len()
    );

    let render_positions = compatibility
        .chunks
        .iter()
        .take(256)
        .map(|chunk| chunk.pos)
        .collect::<Vec<_>>();
    measure_render_cache_condition(
        &world_path,
        &render_positions,
        "logical_cold",
        StorageCachePolicy::Bypass,
        false,
    );
    measure_render_cache_condition(
        &world_path,
        &render_positions,
        "logical_warm",
        StorageCachePolicy::Use,
        true,
    );

    let start = Instant::now();
    let (chunks, layer_stats) = generic_world
        .query_chunk_data_with_stats_blocking(
            render_positions,
            ChunkLoadOptions {
                data_request: ChunkDataRequest::new().layer(64),
                threading: WorldThreadingOptions::Single,
                storage_cache_policy: StorageCachePolicy::Use,
                ..ChunkLoadOptions::default()
            },
        )
        .expect("query fixed layer batch");
    println!(
        "large_fixture.chunk_layer_batch.generic elapsed_ms={} chunks={} worker_threads={} exact_get_batches={} keys_requested={} keys_found={} db_read_ms={} decode_ms={} biome_parse_us={} subchunk_parse_us={} surface_scan_us={} block_entity_parse_us={}",
        start.elapsed().as_millis(),
        chunks.len(),
        layer_stats.worker_threads,
        layer_stats.exact_get_batches,
        layer_stats.keys_requested,
        layer_stats.keys_found,
        layer_stats.db_read_ms,
        layer_stats.decode_ms,
        layer_stats.biome_parse_us,
        layer_stats.subchunk_parse_us,
        layer_stats.surface_scan_us,
        layer_stats.block_entity_parse_us
    );

    let level_bytes = std::fs::read(world_path.join("level.dat")).expect("read level.dat raw");
    let declared_len = u32::from_le_bytes(
        level_bytes[4..8]
            .try_into()
            .expect("level.dat header length"),
    ) as usize;
    let payload_end = 8 + declared_len.min(level_bytes.len().saturating_sub(8));
    let start = Instant::now();
    let events = NbtView::new(&level_bytes[8..payload_end])
        .events()
        .expect("nbt events");
    println!(
        "large_fixture.nbt_events.level_dat elapsed_ms={} events={} payload_len={}",
        start.elapsed().as_millis(),
        events.len(),
        payload_end.saturating_sub(8)
    );

    let (fixture_hash_after, fixture_bytes_after) =
        fixture_hash(&world_path).expect("fixture hash after benchmark");
    assert_eq!(fixture_hash_after, fixture_hash_before, "fixture changed");
    assert_eq!(fixture_bytes_after, fixture_bytes, "fixture size changed");
}

fn measure_render_cache_condition(
    world_path: &Path,
    positions: &[ChunkPos],
    label: &str,
    cache_policy: StorageCachePolicy,
    reuse_handle: bool,
) {
    const SAMPLE_COUNT: usize = 7;
    if positions.is_empty() {
        return;
    }
    let shared_storage = reuse_handle.then(|| {
        BedrockLevelDbStorage::open_read_only(world_path.join("db")).expect("warm read-only db")
    });
    let mut elapsed = Vec::with_capacity(SAMPLE_COUNT);
    let mut db_read_ms = 0_u128;
    let mut decode_ms = 0_u128;
    if let Some(storage) = &shared_storage {
        let world = BedrockWorld::from_typed_storage(
            world_path,
            storage.clone(),
            BedrockWorldOpenOptions::default(),
        );
        let _ = run_render_sample(&world, positions, cache_policy);
    }
    for _ in 0..SAMPLE_COUNT {
        let storage = shared_storage.clone().unwrap_or_else(|| {
            BedrockLevelDbStorage::open_read_only(world_path.join("db")).expect("cold read-only db")
        });
        let world = BedrockWorld::from_typed_storage(
            world_path,
            storage,
            BedrockWorldOpenOptions::default(),
        );
        let started = Instant::now();
        let stats = run_render_sample(&world, positions, cache_policy);
        elapsed.push(started.elapsed());
        db_read_ms = db_read_ms.saturating_add(stats.db_read_ms);
        decode_ms = decode_ms.saturating_add(stats.decode_ms);
    }
    elapsed.sort_unstable();
    let p50 = percentile(&elapsed, 50);
    let p95 = percentile(&elapsed, 95);
    let throughput = positions.len() as f64 / p50.as_secs_f64();
    println!(
        "large_fixture.render cache={label} samples={SAMPLE_COUNT} chunks={} p50_ms={:.3} p95_ms={:.3} chunks_per_sec={throughput:.2} disk_db_read_ms_avg={:.3} cpu_decode_ms_avg={:.3}",
        positions.len(),
        p50.as_secs_f64() * 1_000.0,
        p95.as_secs_f64() * 1_000.0,
        db_read_ms as f64 / SAMPLE_COUNT as f64,
        decode_ms as f64 / SAMPLE_COUNT as f64,
    );
}

fn run_render_sample(
    world: &BedrockWorld<BedrockLevelDbStorage>,
    positions: &[ChunkPos],
    cache_policy: StorageCachePolicy,
) -> bedrock_world::ChunkLoadStats {
    world
        .query_chunk_data_with_stats_blocking(
            positions.iter().copied(),
            ChunkLoadOptions {
                data_request: ChunkDataRequest::new()
                    .surface_columns(ExactSurfaceSubchunkPolicy::Full)
                    .biome(BiomeDataRequirement::SurfaceColumns),
                threading: WorldThreadingOptions::Single,
                storage_cache_policy: cache_policy,
                ..ChunkLoadOptions::default()
            },
        )
        .expect("render exact batch")
        .1
}

fn percentile(samples: &[Duration], percentile: usize) -> Duration {
    let index = (samples.len() * percentile).div_ceil(100).saturating_sub(1);
    samples[index.min(samples.len() - 1)]
}

fn fixture_hash(root: &Path) -> std::io::Result<(u128, u64)> {
    let mut files = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
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
