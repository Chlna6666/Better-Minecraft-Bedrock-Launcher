use bedrock_world::{
    BedrockLevelDbStorage, BedrockWorld, BiomeDataRequirement, ChunkDataRequest, ChunkLoadOptions,
    ChunkPos, Dimension, ExactSurfaceSubchunkPolicy, NbtView, OpenOptions, StorageCachePolicy,
    StorageReadOptions, StorageScanMode, StorageThreadingOptions, StorageVisitorControl,
    WorldScanOptions, WorldStorage, WorldThreadingOptions, read_level_dat_document,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

fn fixture_world_path() -> PathBuf {
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

    let start = Instant::now();
    let level_dat = read_level_dat_document(&world_path.join("level.dat")).expect("read level.dat");
    println!(
        "large_fixture.level_dat elapsed_ms={} version={} payload_len={}",
        start.elapsed().as_millis(),
        level_dat.header.version,
        level_dat.header.actual_payload_len
    );

    let start = Instant::now();
    let storage = BedrockLevelDbStorage::open(world_path.join("db")).expect("open db");
    println!(
        "large_fixture.db.open_lazy elapsed_ms={} mmap_enabled={}",
        start.elapsed().as_millis(),
        cfg!(feature = "leveldb-mmap")
    );

    let dynamic_world = BedrockWorld::from_storage(
        world_path.clone(),
        Arc::new(storage.clone()) as Arc<dyn WorldStorage>,
        OpenOptions::default(),
    );
    let generic_world = BedrockWorld::from_typed_storage(
        world_path.clone(),
        storage.clone(),
        OpenOptions::default(),
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

    let pos = ChunkPos {
        x: 347,
        z: 207,
        dimension: Dimension::Overworld,
    };
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

    let render_positions = render_tile_positions(pos, 16);
    for (cache_label, storage_cache_policy) in [
        ("bypass", StorageCachePolicy::Bypass),
        ("use", StorageCachePolicy::Use),
    ] {
        for pass in ["cold", "warm"] {
            let start = Instant::now();
            let (chunks, render_stats) = generic_world
                .query_chunk_data_with_stats_blocking(
                    render_positions.clone(),
                    ChunkLoadOptions {
                        data_request: ChunkDataRequest::new()
                            .surface_columns(ExactSurfaceSubchunkPolicy::Full)
                            .biome(BiomeDataRequirement::SurfaceColumns),
                        threading: WorldThreadingOptions::Single,
                        storage_cache_policy,
                        ..ChunkLoadOptions::default()
                    },
                )
                .expect("render exact batch");
            println!(
                "large_fixture.render_exact_batch.generic cache_policy={} pass={} elapsed_ms={} chunks={} worker_threads={} prefix_scans={} exact_get_batches={} keys_requested={} keys_found={} db_read_ms={} decode_ms={} biome_parse_us={} subchunk_parse_us={} surface_scan_us={} block_entity_parse_us={}",
                cache_label,
                pass,
                start.elapsed().as_millis(),
                chunks.len(),
                render_stats.worker_threads,
                render_stats.prefix_scans,
                render_stats.exact_get_batches,
                render_stats.keys_requested,
                render_stats.keys_found,
                render_stats.db_read_ms,
                render_stats.decode_ms,
                render_stats.biome_parse_us,
                render_stats.subchunk_parse_us,
                render_stats.surface_scan_us,
                render_stats.block_entity_parse_us
            );
        }
    }

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
}

fn render_tile_positions(origin: ChunkPos, size: i32) -> Vec<ChunkPos> {
    (0..size)
        .flat_map(|z_offset| {
            (0..size).map(move |x_offset| ChunkPos {
                x: origin.x + x_offset,
                z: origin.z + z_offset,
                dimension: origin.dimension,
            })
        })
        .collect()
}
