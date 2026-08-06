#[cfg(feature = "zlib")]
use bedrock_leveldb::{
    ChunkCoordinates, ChunkKey, ChunkRecordTag, Dimension, LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN,
    LEGACY_TERRAIN_VALUE_LEN, SubChunkIndex,
};
use bedrock_leveldb::{
    Db, OpenOptions, ReadOptions, ReadStrategy, ScanMode, ValueRef, VisitorControl,
};
use bytes::Bytes;
use std::cmp::Ordering;
use std::io::Write;
use std::path::Path;

const VALUE_TYPE_DELETION: u8 = 0;
const VALUE_TYPE_VALUE: u8 = 1;
const LEVELDB_TABLE_MAGIC: u64 = 0xdb47_7524_8b80_fb57;
const FULL_RECORD: u8 = 1;
#[cfg(feature = "zlib")]
const COMPRESSION_ZLIB: u8 = 2;
#[cfg(feature = "zlib")]
const COMPRESSION_DEFLATE: u8 = 4;

#[derive(Clone)]
struct NativeEntry {
    user_key: Vec<u8>,
    sequence: u64,
    value: Option<Vec<u8>>,
}

struct TableMeta {
    number: u64,
    file_size: u64,
    smallest: Vec<u8>,
    largest: Vec<u8>,
}

#[test]
fn native_table_point_prefix_and_deletion_records_are_read() {
    let temp = tempfile::tempdir().expect("tempdir");
    let entries = vec![
        NativeEntry::value(b"alpha", 3, b"one"),
        NativeEntry::delete(b"gone", 5),
        NativeEntry::value(b"gone", 4, b"old"),
        NativeEntry::value(b"player_1", 3, b"steve"),
        NativeEntry::value(b"player_2", 3, b"alex"),
    ];
    let meta = write_native_table(temp.path(), 3, &entries).expect("write table");
    write_native_manifest(temp.path(), &[meta], 2).expect("write manifest");

    let db = open_native_read_only(temp.path());

    assert_eq!(
        db.get(b"alpha").expect("get alpha"),
        Some(Bytes::from_static(b"one"))
    );
    assert_eq!(db.get(b"gone").expect("get gone"), None);
    let batch_values = db
        .get_many_owned(
            vec![
                Bytes::from_static(b"aardvark"),
                Bytes::from_static(b"gone"),
                Bytes::from_static(b"alpha"),
            ],
            ReadOptions::default(),
        )
        .expect("get many with tombstone");
    assert_eq!(
        batch_values,
        vec![None, None, Some(Bytes::from_static(b"one"))]
    );

    let mut prefix_values = Vec::new();
    db.for_each_prefix(b"player_", ReadOptions::default(), |key, value| {
        prefix_values.push((Bytes::copy_from_slice(key), value.clone()));
        Ok(VisitorControl::Continue)
    })
    .expect("prefix");
    prefix_values.sort();
    assert_eq!(
        prefix_values,
        vec![
            (
                Bytes::from_static(b"player_1"),
                Bytes::from_static(b"steve")
            ),
            (Bytes::from_static(b"player_2"), Bytes::from_static(b"alex")),
        ]
    );

    let mut keys = Vec::new();
    db.for_each_key(ReadOptions::default(), |key| {
        keys.push(Bytes::copy_from_slice(key));
        Ok(VisitorControl::Continue)
    })
    .expect("keys");
    assert!(!keys.iter().any(|key| key.as_ref() == b"gone"));
}

#[test]
fn native_uncompressed_prefix_ref_returns_borrowed_values() {
    let temp = tempfile::tempdir().expect("tempdir");
    let entries = vec![
        NativeEntry::value(b"player_1", 3, b"steve"),
        NativeEntry::value(b"player_2", 3, b"alex"),
    ];
    let meta = write_native_table(temp.path(), 3, &entries).expect("write table");
    write_native_manifest(temp.path(), &[meta], 2).expect("write manifest");

    let db = open_native_read_only(temp.path());
    let mut borrowed = 0usize;
    let mut values = Vec::new();
    db.for_each_prefix_ref(
        b"player_",
        ReadOptions {
            read_strategy: ReadStrategy::Borrowed,
            scan_mode: ScanMode::Sequential,
            ..ReadOptions::default()
        },
        |entry| {
            if matches!(entry.value, ValueRef::Borrowed(_)) {
                borrowed = borrowed.saturating_add(1);
            }
            values.push(Bytes::copy_from_slice(entry.value.as_bytes()));
            Ok(VisitorControl::Continue)
        },
    )
    .expect("prefix ref");

    values.sort();
    assert_eq!(
        values,
        vec![Bytes::from_static(b"alex"), Bytes::from_static(b"steve")]
    );
    assert_eq!(borrowed, 2);
}

#[test]
fn native_manifest_ranges_skip_unrelated_newer_corrupt_tables() {
    let temp = tempfile::tempdir().expect("tempdir");
    let good_entries = vec![NativeEntry::value(b"target", 7, b"value")];
    let good_meta = write_native_table(temp.path(), 3, &good_entries).expect("write table");

    std::fs::write(temp.path().join("000004.ldb"), b"not a leveldb table").expect("corrupt table");
    let corrupt_meta = TableMeta {
        number: 4,
        file_size: 19,
        smallest: internal_key(b"zzz", 1, VALUE_TYPE_VALUE),
        largest: internal_key(b"zzz", 1, VALUE_TYPE_VALUE),
    };
    write_native_manifest(temp.path(), &[good_meta, corrupt_meta], 2).expect("write manifest");

    let db = open_native_read_only(temp.path());
    assert_eq!(
        db.get(b"target").expect("get"),
        Some(Bytes::from_static(b"value"))
    );
}

#[cfg(feature = "zlib")]
#[test]
fn native_compressed_tables_get_many_reads_legacy_records_in_order() {
    let temp = tempfile::tempdir().expect("tempdir");
    let legacy_terrain_key = Bytes::from(
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
    let terrain = vec![7_u8; LEGACY_TERRAIN_VALUE_LEN];
    let mut legacy_subchunk = vec![0_u8; LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN];
    legacy_subchunk[0] = 2;
    legacy_subchunk[1 + 1_125] = 99;
    let modern_subchunk = b"\x08\x00modern-paletted-native".to_vec();

    let zlib_meta = write_native_table_compressed_data(
        temp.path(),
        3,
        &[NativeEntry::value(&legacy_terrain_key, 7, &terrain)],
        COMPRESSION_ZLIB,
    )
    .expect("write zlib table");
    let deflate_meta = write_native_table_compressed_data(
        temp.path(),
        4,
        &[
            NativeEntry::value(&legacy_subchunk_key, 7, &legacy_subchunk),
            NativeEntry::value(&modern_subchunk_key, 7, &modern_subchunk),
        ],
        COMPRESSION_DEFLATE,
    )
    .expect("write deflate table");
    write_native_manifest(temp.path(), &[zlib_meta, deflate_meta], 2).expect("write manifest");

    let db = open_native_read_only(temp.path());
    let values = db
        .get_many_owned(
            vec![
                Bytes::from_static(b"missing"),
                legacy_terrain_key.clone(),
                legacy_subchunk_key,
                modern_subchunk_key,
                legacy_terrain_key,
            ],
            ReadOptions::default(),
        )
        .expect("get many");

    assert!(values[0].is_none());
    assert_eq!(values[1], Some(Bytes::from(terrain.clone())));
    assert_eq!(values[2], Some(Bytes::from(legacy_subchunk)));
    assert_eq!(values[3], Some(Bytes::from(modern_subchunk)));
    assert_eq!(values[4], Some(Bytes::from(terrain)));
}

impl NativeEntry {
    fn value(key: &[u8], sequence: u64, value: &[u8]) -> Self {
        Self {
            user_key: key.to_vec(),
            sequence,
            value: Some(value.to_vec()),
        }
    }

    fn delete(key: &[u8], sequence: u64) -> Self {
        Self {
            user_key: key.to_vec(),
            sequence,
            value: None,
        }
    }

    fn internal_key(&self) -> Vec<u8> {
        internal_key(
            &self.user_key,
            self.sequence,
            if self.value.is_some() {
                VALUE_TYPE_VALUE
            } else {
                VALUE_TYPE_DELETION
            },
        )
    }
}

fn open_native_read_only(path: &Path) -> Db {
    Db::open(
        path,
        OpenOptions {
            read_only: true,
            create_if_missing: false,
            ..OpenOptions::default()
        },
    )
    .expect("open native")
}

fn write_native_table(
    root: &Path,
    number: u64,
    entries: &[NativeEntry],
) -> std::io::Result<TableMeta> {
    let mut internal_entries = entries
        .iter()
        .map(|entry| {
            (
                entry.internal_key(),
                entry.value.clone().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    internal_entries.sort_by(|left, right| compare_internal_keys(&left.0, &right.0));

    let smallest = internal_entries
        .first()
        .expect("native table needs entries")
        .0
        .clone();
    let largest = internal_entries
        .last()
        .expect("native table needs entries")
        .0
        .clone();

    let data_block = block(&internal_entries);
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

    let path = root.join(format!("{number:06}.ldb"));
    std::fs::write(path, &table)?;

    Ok(TableMeta {
        number,
        file_size: u64::try_from(table.len()).expect("small fixture"),
        smallest,
        largest,
    })
}

#[cfg(feature = "zlib")]
fn write_native_table_compressed_data(
    root: &Path,
    number: u64,
    entries: &[NativeEntry],
    data_compression: u8,
) -> std::io::Result<TableMeta> {
    let mut internal_entries = entries
        .iter()
        .map(|entry| {
            (
                entry.internal_key(),
                entry.value.clone().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    internal_entries.sort_by(|left, right| compare_internal_keys(&left.0, &right.0));

    let smallest = internal_entries
        .first()
        .expect("native table needs entries")
        .0
        .clone();
    let largest = internal_entries
        .last()
        .expect("native table needs entries")
        .0
        .clone();

    let data_block = block(&internal_entries);
    let encoded_data_block = compress_native_block(&data_block, data_compression)?;
    let index_offset = u64::try_from(encoded_data_block.len() + 5).expect("small fixture");
    let mut index_value = Vec::new();
    put_varint64(0, &mut index_value);
    put_varint64(
        u64::try_from(encoded_data_block.len()).expect("small fixture"),
        &mut index_value,
    );
    let index_block = block(&[(largest.clone(), index_value)]);

    let mut table = Vec::new();
    table.extend_from_slice(&encoded_data_block);
    push_block_trailer_with_compression(&mut table, &encoded_data_block, data_compression);
    table.extend_from_slice(&index_block);
    push_block_trailer(&mut table, &index_block);
    push_footer(
        &mut table,
        index_offset,
        u64::try_from(index_block.len()).expect("small fixture"),
    );

    let path = root.join(format!("{number:06}.ldb"));
    std::fs::write(path, &table)?;

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
    push_block_trailer_with_compression(out, payload, 0);
}

fn push_block_trailer_with_compression(out: &mut Vec<u8>, payload: &[u8], compression: u8) {
    out.push(compression);
    let checksum = masked_crc32c(&[payload, &[compression]]);
    out.extend_from_slice(&checksum.to_le_bytes());
}

#[cfg(feature = "zlib")]
fn compress_native_block(payload: &[u8], compression: u8) -> std::io::Result<Vec<u8>> {
    match compression {
        COMPRESSION_ZLIB => {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(payload)?;
            encoder.finish()
        }
        COMPRESSION_DEFLATE => {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(payload)?;
            encoder.finish()
        }
        other => panic!("unsupported compressed test tag {other}"),
    }
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
    let checksum = masked_crc32c(&[&[FULL_RECORD], payload]);
    file.write_all(&checksum.to_le_bytes())?;
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
