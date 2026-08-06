use bedrock_render::{
    ChunkFingerprintInput, Dimension, TILE_AUTHORITY_FLAG_NON_EMPTY, TileAuthorityBlobReader,
    TileAuthorityCache, TileAuthorityCacheKey, TileAuthorityCommit, TileAuthorityEntry,
    TileAuthorityIndexSnapshot, tile_payload_fingerprint, validate_chunk_fingerprints_parallel,
};
use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use std::fs;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

static FIXTURE_ID: AtomicUsize = AtomicUsize::new(0);

struct AuthorityFixture {
    cache_root: PathBuf,
    cache: TileAuthorityCache,
    key: TileAuthorityCacheKey,
    snapshot: TileAuthorityIndexSnapshot,
    reader: Option<TileAuthorityBlobReader>,
}

impl AuthorityFixture {
    fn new(payload_len: usize) -> Self {
        let cache_root = unique_cache_root();
        let cache = TileAuthorityCache::new(&cache_root);
        let key = TileAuthorityCacheKey {
            world_id: "criterion-world".to_string(),
            world_signature: "criterion-signature".to_string(),
            renderer_signature: "criterion-renderer".to_string(),
            mode_slug: "surface".to_string(),
            renderer_version: 52,
            palette_version: 16,
            dimension: Dimension::Overworld,
            chunks_per_tile: 8,
            blocks_per_pixel: 1,
            pixels_per_block: 4,
        };
        let payload = vec![0x11; payload_len];
        let snapshot = cache
            .commit_tile(&key, None, authority_commit(0, 0, payload), false)
            .expect("create authority fixture");
        let reader =
            TileAuthorityBlobReader::open(&cache, &key).expect("open authority fixture reader");
        Self {
            cache_root,
            cache,
            key,
            snapshot,
            reader,
        }
    }

    fn refresh_with_payload(&mut self, payload: Vec<u8>) {
        self.snapshot = self
            .cache
            .commit_tile(
                &self.key,
                Some(&self.snapshot),
                authority_commit(0, 0, payload),
                false,
            )
            .expect("refresh authority fixture");
    }

    fn tile_entry(&self) -> TileAuthorityEntry {
        self.snapshot.tile(0, 0).expect("fixture tile entry")
    }
}

impl Drop for AuthorityFixture {
    fn drop(&mut self) {
        self.reader.take();
        let _ = fs::remove_dir_all(&self.cache_root);
    }
}

fn authority_commit(tile_x: i32, tile_z: i32, payload: Vec<u8>) -> TileAuthorityCommit {
    TileAuthorityCommit {
        entry: TileAuthorityEntry {
            tile_x,
            tile_z,
            width: 16,
            height: 16,
            pixel_len: 16 * 16 * 4,
            blob_offset: 0,
            blob_len: u64::try_from(payload.len()).expect("payload length fits u64"),
            payload_hash: tile_payload_fingerprint(&payload),
            validation_value: 0,
            flags: TILE_AUTHORITY_FLAG_NON_EMPTY,
        },
        encoded_blob: payload,
        dependencies: Vec::new(),
        chunk_states: Vec::new(),
        chunk_tile_refs: Vec::new(),
    }
}

fn fingerprint_inputs() -> Vec<ChunkFingerprintInput> {
    (0_u8..128)
        .map(|index| ChunkFingerprintInput {
            position: bedrock_render::ChunkPos {
                x: i32::from(index),
                z: i32::from(index / 8),
                dimension: Dimension::Overworld,
            },
            revision: u64::from(index),
            bytes: Arc::<[u8]>::from(vec![index; 16 * 1024]),
        })
        .collect()
}

fn unique_cache_root() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let fixture_id = FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "bedrock-render-cache-bench-{}-{nanos}-{fixture_id}",
        std::process::id()
    ))
}

fn authority_cache_benches(c: &mut Criterion) {
    let fixture = AuthorityFixture::new(64 * 1024);
    let entry = fixture.tile_entry();
    let reader = fixture.reader.as_ref().expect("fixture reader");

    c.bench_function("bedrock_render/cache/authority_index_lookup", |bench| {
        bench.iter(|| black_box(fixture.snapshot.tile(0, 0)));
    });
    c.bench_function(
        "bedrock_render/cache/authority_payload_pread_64k",
        |bench| {
            bench.iter(|| {
                let payload = reader.read_entry(entry).expect("read fixture tile");
                black_box(payload);
            });
        },
    );
    c.bench_function(
        "bedrock_render/cache/authority_refresh_new_payload_64k",
        |bench| {
            bench.iter_batched(
                || AuthorityFixture::new(64 * 1024),
                |mut fixture| {
                    fixture.refresh_with_payload(vec![0x22; 64 * 1024]);
                    black_box(fixture.snapshot.generation);
                },
                BatchSize::SmallInput,
            );
        },
    );

    let inputs = fingerprint_inputs();
    c.bench_function("bedrock_render/cache/xxh3_128_validate_128x16k", |bench| {
        bench.iter_batched(
            || inputs.clone(),
            |inputs| black_box(validate_chunk_fingerprints_parallel(inputs)),
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(cache_benches, authority_cache_benches);
criterion_main!(cache_benches);
