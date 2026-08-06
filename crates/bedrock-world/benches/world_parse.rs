use bedrock_world::{
    BedrockLevelDbStorage, BedrockWorld, ChunkPos, Dimension, NbtReader, NbtTag, NbtWriter,
    OpenOptions,
    chunk::{SubChunkDecodeMode, parse_subchunk, parse_subchunk_with_mode},
    parse_level_dat_document, read_level_dat_document,
};
use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use indexmap::IndexMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn fixture_world_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample-bedrock-world")
}

fn criterion() -> Criterion {
    Criterion::default()
        .sample_size(10)
        .measurement_time(Duration::from_secs(4))
}

fn bench_level_dat(c: &mut Criterion) {
    let synthetic = synthetic_level_dat_bytes();
    let level_dat_path = fixture_world_path().join("level.dat");
    let mut group = c.benchmark_group("bedrock_world/level_dat");
    group.throughput(Throughput::Bytes(synthetic.len() as u64));
    group.bench_function("parse_synthetic", |b| {
        b.iter(|| parse_level_dat_document(black_box(&synthetic)).expect("parse level.dat"));
    });
    group.bench_function("nbt_events_synthetic", |b| {
        let nbt_payload = &synthetic[8..];
        b.iter(|| {
            black_box(
                NbtReader::new(black_box(nbt_payload))
                    .view()
                    .events()
                    .expect("borrowed events"),
            );
        });
    });
    group.bench_function("nbt_root_owned_synthetic", |b| {
        let nbt_payload = &synthetic[8..];
        b.iter(|| {
            black_box(
                NbtReader::new(black_box(nbt_payload))
                    .parse_root()
                    .expect("owned root"),
            );
        });
    });
    group.bench_function("nbt_root_ref_synthetic", |b| {
        let nbt_payload = &synthetic[8..];
        b.iter(|| {
            black_box(
                NbtReader::new(black_box(nbt_payload))
                    .parse_root_ref()
                    .expect("borrowed root"),
            );
        });
    });
    if level_dat_path.exists() {
        group.bench_function("read_fixture", |b| {
            b.iter(|| read_level_dat_document(black_box(&level_dat_path)).expect("read level.dat"));
        });
    } else {
        eprintln!(
            "skipping fixture level.dat benchmark; missing {}",
            level_dat_path.display()
        );
    }
    group.finish();
}

fn bench_large_fixture(c: &mut Criterion) {
    let world_path = fixture_world_path();
    if !world_path.join("level.dat").exists() || !world_path.join("db").join("CURRENT").exists() {
        eprintln!(
            "skipping large fixture Criterion benchmarks; missing {}",
            world_path.display()
        );
        return;
    }
    let storage = Arc::new(BedrockLevelDbStorage::open(world_path.join("db")).expect("open db"));

    c.bench_function("bedrock_world/db/open_lazy", |b| {
        b.iter(|| {
            black_box(
                BedrockLevelDbStorage::open(black_box(world_path.join("db"))).expect("open db"),
            );
        });
    });

    let pos = ChunkPos {
        x: 347,
        z: 207,
        dimension: Dimension::Overworld,
    };
    let world = BedrockWorld::from_storage(world_path, storage, OpenOptions::default());
    c.bench_function("bedrock_world/world/list_players", |b| {
        b.iter(|| black_box(world.list_players_blocking().expect("list players")));
    });
    let chunk = world
        .get_chunk_blocking(pos)
        .expect("load fixture sample chunk");
    let (subchunk_y, value) = chunk
        .records
        .iter()
        .find(|record| record.key.tag == bedrock_world::ChunkRecordTag::SubChunkPrefix)
        .map(|record| {
            (
                record.key.subchunk_y.unwrap_or_default(),
                record.value.clone(),
            )
        })
        .expect("fixture sample subchunk");
    c.bench_function("bedrock_world/subchunk/decode_palette_full_indices", |b| {
        b.iter(|| {
            black_box(
                parse_subchunk(black_box(subchunk_y), black_box(value.clone()))
                    .expect("decode full subchunk"),
            )
        });
    });
    c.bench_function("bedrock_world/subchunk/decode_palette_counts_only", |b| {
        b.iter(|| {
            black_box(
                parse_subchunk_with_mode(
                    black_box(subchunk_y),
                    black_box(value.clone()),
                    SubChunkDecodeMode::CountsOnly,
                )
                .expect("decode counts-only subchunk"),
            )
        });
    });

    c.bench_function("bedrock_world/chunk/parse_fixture_chunk", |b| {
        b.iter(|| {
            black_box(
                world
                    .parse_chunk_blocking(black_box(pos))
                    .expect("parse chunk"),
            )
        });
    });
}

fn synthetic_level_dat_bytes() -> Vec<u8> {
    let mut root = IndexMap::new();
    root.insert(
        "LevelName".to_string(),
        NbtTag::String("Benchmark World".to_string()),
    );
    root.insert("StorageVersion".to_string(), NbtTag::Int(10));
    root.insert("RandomSeed".to_string(), NbtTag::Long(12345));
    let payload = NbtWriter::write_root(&NbtTag::Compound(root)).expect("serialize level.dat NBT");
    let mut bytes = Vec::with_capacity(payload.len() + 8);
    bytes.extend_from_slice(&10_u32.to_le_bytes());
    bytes.extend_from_slice(
        &u32::try_from(payload.len())
            .expect("synthetic level.dat payload fits in u32")
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&payload);
    bytes
}

criterion_group!(
    name = benches;
    config = criterion();
    targets = bench_level_dat,
        bench_large_fixture
);
criterion_main!(benches);
