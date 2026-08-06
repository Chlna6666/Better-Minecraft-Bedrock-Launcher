#![allow(clippy::too_many_lines)]

use bedrock_leveldb::{
    CachePolicy, CompressionPolicy, Db, OpenOptions, ReadOptions, ReadStrategy, ScanMode,
    VisitorControl, WriteBatch, WriteOptions,
};
use bytes::Bytes;
use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use std::cmp::Ordering;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

const VALUE_TYPE_VALUE: u8 = 1;
const LEVELDB_TABLE_MAGIC: u64 = 0xdb47_7524_8b80_fb57;
const FULL_RECORD: u8 = 1;

fn criterion() -> Criterion {
    Criterion::default()
        .sample_size(10)
        .measurement_time(Duration::from_secs(2))
}

fn open_temp_db(write_buffer_size: usize) -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open(
        dir.path(),
        OpenOptions {
            compression_policy: CompressionPolicy::None,
            write_buffer_size,
            ..OpenOptions::default()
        },
    )
    .expect("open db");
    (dir, db)
}

fn seed_overlay_db(entries: usize, value_size: usize) -> (tempfile::TempDir, Db) {
    let (dir, db) = open_temp_db(usize::MAX);
    write_entries(&db, entries, value_size);
    (dir, db)
}

fn seed_flushed_custom_db(entries: usize, value_size: usize) -> (tempfile::TempDir, Db) {
    let (dir, db) = seed_overlay_db(entries, value_size);
    db.flush().expect("flush");
    (dir, db)
}

fn write_entries(db: &Db, entries: usize, value_size: usize) {
    let mut batch = WriteBatch::new();
    for index in 0..entries {
        batch.put(
            Bytes::from(format!("chunk:{index:06}")),
            Bytes::from(vec![
                u8::try_from(index % 251).expect("small byte");
                value_size
            ]),
        );
    }
    db.write(batch, WriteOptions::default()).expect("seed");
}

fn bench_writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("bedrock_leveldb/write");
    group.throughput(Throughput::Elements(1_000));
    group.bench_function("batch_1000_overlay", |b| {
        b.iter_batched(
            || open_temp_db(usize::MAX),
            |(_dir, db)| {
                let mut batch = WriteBatch::new();
                for index in 0..1_000 {
                    batch.put(
                        Bytes::from(format!("key:{index:06}")),
                        Bytes::from(vec![42; 128]),
                    );
                }
                db.write(black_box(batch), WriteOptions::default())
                    .expect("write");
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn bench_point_get(c: &mut Criterion) {
    let entries = 4_096;
    let (_overlay_dir, overlay_db) = seed_overlay_db(entries, 256);
    let (_custom_dir, custom_db) = seed_flushed_custom_db(entries, 256);
    let (native_dir, native_db) = seed_native_db(entries, 256, 1);

    let mut group = c.benchmark_group("bedrock_leveldb/get_point");
    group.throughput(Throughput::Elements(1));
    for (name, db) in [
        ("overlay_hot", &overlay_db),
        ("custom_table", &custom_db),
        ("native_table", &native_db),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| {
                black_box(db.get(black_box(b"chunk:002048")).expect("get"));
            });
        });
    }
    group.bench_function("native_table_ref_shared", |b| {
        b.iter(|| {
            black_box(
                native_db
                    .get_with_ref(
                        black_box(b"chunk:002048"),
                        ReadOptions {
                            read_strategy: ReadStrategy::Shared,
                            ..ReadOptions::default()
                        },
                    )
                    .expect("get ref"),
            );
        });
    });
    drop(native_dir);
    group.finish();
}

fn bench_get_many(c: &mut Criterion) {
    let entries = 4_096;
    let (native_dir, native_db) = seed_native_db(entries, 256, 1);
    let dense_keys = (1_920..2_176)
        .map(|index| Bytes::from(format!("chunk:{index:06}")))
        .collect::<Vec<_>>();
    let sparse_keys = (1_920..2_176)
        .flat_map(|index| {
            [
                Bytes::from(format!("chunk:{index:06}-missing")),
                Bytes::from(format!("chunk:{index:06}")),
            ]
        })
        .collect::<Vec<_>>();

    let mut group = c.benchmark_group("bedrock_leveldb/get_many");
    for (name, keys, cache_policy) in [
        (
            "native_table_256_dense_bypass",
            &dense_keys,
            CachePolicy::Bypass,
        ),
        ("native_table_256_dense_use", &dense_keys, CachePolicy::Use),
        (
            "native_table_512_sparse_bypass",
            &sparse_keys,
            CachePolicy::Bypass,
        ),
        (
            "native_table_512_sparse_use",
            &sparse_keys,
            CachePolicy::Use,
        ),
    ] {
        group.throughput(Throughput::Elements(
            u64::try_from(keys.len()).expect("key count"),
        ));
        group.bench_with_input(BenchmarkId::from_parameter(name), keys, |b, keys| {
            b.iter(|| {
                black_box(
                    native_db
                        .get_many_owned(
                            black_box(keys.clone()),
                            ReadOptions {
                                cache_policy,
                                ..ReadOptions::default()
                            },
                        )
                        .expect("get many"),
                );
            });
        });
    }
    drop(native_dir);
    group.finish();
}

fn bench_scans(c: &mut Criterion) {
    let entries = 4_096;
    let (_custom_dir, custom_db) = seed_flushed_custom_db(entries, 256);
    let (_native_dir, native_db) = seed_native_db(entries, 256, 8);

    let mut group = c.benchmark_group("bedrock_leveldb/scan");
    group.throughput(Throughput::Elements(
        u64::try_from(entries).expect("entry count"),
    ));
    for (name, db, options) in [
        ("custom_for_each_key", &custom_db, ReadOptions::default()),
        ("custom_for_each_entry", &custom_db, ReadOptions::default()),
        ("native_for_each_prefix", &native_db, ReadOptions::default()),
        (
            "native_for_each_prefix_key",
            &native_db,
            ReadOptions::default(),
        ),
        (
            "native_parallel_tables",
            &native_db,
            ReadOptions {
                scan_mode: ScanMode::ParallelTables,
                ..ReadOptions::default()
            },
        ),
    ] {
        group.bench_with_input(
            BenchmarkId::from_parameter(name),
            &options,
            |b, read_options| {
                b.iter(|| {
                    let mut bytes = 0usize;
                    if name.ends_with("key") {
                        db.for_each_key(read_options.clone(), |key| {
                            bytes = bytes.saturating_add(key.len());
                            Ok(VisitorControl::Continue)
                        })
                        .expect("scan keys");
                    } else if name.ends_with("prefix_key") {
                        db.for_each_prefix_key(b"chunk:00", read_options.clone(), |key| {
                            bytes = bytes.saturating_add(key.len());
                            Ok(VisitorControl::Continue)
                        })
                        .expect("scan prefix keys");
                    } else if name.ends_with("prefix") {
                        db.for_each_prefix(b"chunk:00", read_options.clone(), |_key, value| {
                            bytes = bytes.saturating_add(value.len());
                            Ok(VisitorControl::Continue)
                        })
                        .expect("scan prefix");
                    } else {
                        db.for_each_entry(read_options.clone(), |_key, value| {
                            bytes = bytes.saturating_add(value.len());
                            Ok(VisitorControl::Continue)
                        })
                        .expect("scan entries");
                    }
                    black_box(bytes);
                });
            },
        );
    }
    group.bench_function("native_prefix_ref_shared", |b| {
        b.iter(|| {
            let mut bytes = 0usize;
            native_db
                .for_each_prefix_ref(b"chunk:00", ReadOptions::default(), |entry| {
                    bytes = bytes.saturating_add(entry.value.len());
                    Ok(VisitorControl::Continue)
                })
                .expect("scan prefix refs");
            black_box(bytes);
        });
    });
    group.bench_function("native_prefix_ref_borrowed_mmap", |b| {
        b.iter(|| {
            let mut bytes = 0usize;
            native_db
                .for_each_prefix_ref(
                    b"chunk:00",
                    ReadOptions {
                        read_strategy: ReadStrategy::Borrowed,
                        scan_mode: ScanMode::Sequential,
                        ..ReadOptions::default()
                    },
                    |entry| {
                        bytes = bytes.saturating_add(entry.value.len());
                        Ok(VisitorControl::Continue)
                    },
                )
                .expect("scan borrowed prefix refs");
            black_box(bytes);
        });
    });
    group.finish();
}

fn bench_recover(c: &mut Criterion) {
    let mut group = c.benchmark_group("bedrock_leveldb/recover");
    group.throughput(Throughput::Elements(1_000));
    group.bench_function("wal_1000_overlay", |b| {
        b.iter_batched(
            || {
                let (dir, db) = seed_overlay_db(1_000, 128);
                drop(db);
                dir
            },
            |dir| {
                let db = Db::open(
                    black_box(dir.path()),
                    OpenOptions {
                        compression_policy: CompressionPolicy::None,
                        ..OpenOptions::default()
                    },
                )
                .expect("recover");
                black_box(db.stats_fast().expect("stats"));
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    name = benches;
    config = criterion();
    targets = bench_writes, bench_point_get, bench_get_many, bench_scans, bench_recover
);
criterion_main!(benches);

fn seed_native_db(entries: usize, value_size: usize, tables: usize) -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut metas = Vec::new();
    let per_table = entries / tables;
    for table_index in 0..tables {
        let start = table_index * per_table;
        let end = if table_index + 1 == tables {
            entries
        } else {
            start + per_table
        };
        let native_entries = (start..end)
            .map(|index| {
                (
                    internal_key(format!("chunk:{index:06}").as_bytes(), 1, VALUE_TYPE_VALUE),
                    vec![u8::try_from(index % 251).expect("small byte"); value_size],
                )
            })
            .collect::<Vec<_>>();
        metas.push(
            write_native_table(
                dir.path(),
                u64::try_from(table_index + 3).expect("small table number"),
                &native_entries,
            )
            .expect("native table"),
        );
    }
    write_native_manifest(dir.path(), &metas, 2).expect("native manifest");
    let db = Db::open(
        dir.path(),
        OpenOptions {
            read_only: true,
            create_if_missing: false,
            cache_size: 0,
            ..OpenOptions::default()
        },
    )
    .expect("open native");
    (dir, db)
}

struct TableMeta {
    number: u64,
    file_size: u64,
    smallest: Vec<u8>,
    largest: Vec<u8>,
}

fn write_native_table(
    root: &Path,
    number: u64,
    entries: &[(Vec<u8>, Vec<u8>)],
) -> std::io::Result<TableMeta> {
    let mut entries = entries.to_vec();
    entries.sort_by(|left, right| compare_internal_keys(&left.0, &right.0));
    let smallest = entries.first().expect("entries").0.clone();
    let largest = entries.last().expect("entries").0.clone();
    let data_block = block(&entries);
    let index_offset = u64::try_from(data_block.len() + 5).expect("small fixture");
    let mut index_value = Vec::new();
    put_varint64(0, &mut index_value);
    put_varint64(
        u64::try_from(data_block.len()).expect("small fixture"),
        &mut index_value,
    );
    let index_block = block(&[(largest.clone(), index_value)]);

    let mut table = Vec::new();
    table.extend_from_slice(&data_block);
    push_block_trailer(&mut table, &data_block);
    table.extend_from_slice(&index_block);
    push_block_trailer(&mut table, &index_block);
    push_footer(
        &mut table,
        index_offset,
        u64::try_from(index_block.len()).expect("small fixture"),
    );
    std::fs::write(root.join(format!("{number:06}.ldb")), &table)?;

    Ok(TableMeta {
        number,
        file_size: u64::try_from(table.len()).expect("small fixture"),
        smallest,
        largest,
    })
}

fn write_native_manifest(
    root: &Path,
    tables: &[TableMeta],
    log_number: u64,
) -> std::io::Result<()> {
    let mut edit = Vec::new();
    put_varint32(1, &mut edit);
    put_length_prefixed_slice(b"leveldb.BytewiseComparator", &mut edit);
    put_varint32(2, &mut edit);
    put_varint64(log_number, &mut edit);
    put_varint32(3, &mut edit);
    put_varint64(100, &mut edit);
    put_varint32(4, &mut edit);
    put_varint64(1000, &mut edit);
    for table in tables {
        put_varint32(7, &mut edit);
        put_varint32(0, &mut edit);
        put_varint64(table.number, &mut edit);
        put_varint64(table.file_size, &mut edit);
        put_length_prefixed_slice(&table.smallest, &mut edit);
        put_length_prefixed_slice(&table.largest, &mut edit);
    }

    let manifest_name = "MANIFEST-000001";
    let mut manifest = std::fs::File::create(root.join(manifest_name))?;
    write_log_record(&mut manifest, &edit)?;
    std::fs::write(root.join("CURRENT"), format!("{manifest_name}\n"))?;
    Ok(())
}

fn block(entries: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut restarts = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        restarts.push(u32::try_from(out.len()).expect("small block"));
        put_varint32(0, &mut out);
        put_varint32(u32::try_from(key.len()).expect("small key"), &mut out);
        put_varint32(u32::try_from(value.len()).expect("small value"), &mut out);
        out.extend_from_slice(key);
        out.extend_from_slice(value);
    }
    for restart in &restarts {
        out.extend_from_slice(&restart.to_le_bytes());
    }
    out.extend_from_slice(
        &u32::try_from(restarts.len())
            .expect("small restart count")
            .to_le_bytes(),
    );
    out
}

fn push_block_trailer(out: &mut Vec<u8>, payload: &[u8]) {
    out.push(0);
    out.extend_from_slice(&masked_crc32c(&[payload, &[0]]).to_le_bytes());
}

fn push_footer(out: &mut Vec<u8>, index_offset: u64, index_size: u64) {
    let mut handles = Vec::new();
    put_varint64(0, &mut handles);
    put_varint64(0, &mut handles);
    put_varint64(index_offset, &mut handles);
    put_varint64(index_size, &mut handles);
    handles.resize(40, 0);
    out.extend_from_slice(&handles);
    out.extend_from_slice(&LEVELDB_TABLE_MAGIC.to_le_bytes());
}

fn write_log_record(file: &mut std::fs::File, payload: &[u8]) -> std::io::Result<()> {
    file.write_all(&masked_crc32c(&[&[FULL_RECORD], payload]).to_le_bytes())?;
    file.write_all(
        &u16::try_from(payload.len())
            .expect("small manifest record")
            .to_le_bytes(),
    )?;
    file.write_all(&[FULL_RECORD])?;
    file.write_all(payload)
}

fn internal_key(user_key: &[u8], sequence: u64, value_type: u8) -> Vec<u8> {
    let mut key = user_key.to_vec();
    key.extend_from_slice(&((sequence << 8) | u64::from(value_type)).to_le_bytes());
    key
}

fn compare_internal_keys(left: &[u8], right: &[u8]) -> Ordering {
    let (left_user, left_tag) = split_internal_key(left);
    let (right_user, right_tag) = split_internal_key(right);
    left_user
        .cmp(right_user)
        .then_with(|| right_tag.cmp(&left_tag))
}

fn split_internal_key(key: &[u8]) -> (&[u8], u64) {
    let split = key.len().checked_sub(8).expect("internal key trailer");
    let mut tag = [0; 8];
    tag.copy_from_slice(&key[split..]);
    (&key[..split], u64::from_le_bytes(tag))
}

fn put_length_prefixed_slice(value: &[u8], out: &mut Vec<u8>) {
    put_varint32(u32::try_from(value.len()).expect("small slice"), out);
    out.extend_from_slice(value);
}

fn put_varint32(mut value: u32, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(u8::try_from(value & 0x7f).expect("masked varint32 byte") | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).expect("final varint32 byte"));
}

fn put_varint64(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(u8::try_from(value & 0x7f).expect("masked varint64 byte") | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).expect("final varint64 byte"));
}

fn masked_crc32c(chunks: &[&[u8]]) -> u32 {
    let mut crc = !0_u32;
    for chunk in chunks {
        crc = update_crc32c(crc, chunk);
    }
    (!crc).rotate_right(15).wrapping_add(0xa282_ead8)
}

fn update_crc32c(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0x82f6_3b78 & mask);
        }
    }
    crc
}
