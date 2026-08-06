#![cfg(feature = "backend-bedrock-leveldb")]

use bedrock_world::{
    BedrockDbKey, BedrockLevelDbStorage, BedrockWorld, ChunkPos, ChunkRecordTag, NbtTag,
    OpenOptions, StorageReadOptions, StorageVisitorControl, WorldStorage, read_level_dat_document,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

fn fixture_world_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample-bedrock-world")
}

#[test]
fn parses_copied_bedrock_world_through_leveldb_backend() {
    let world_path = fixture_world_path();
    if !world_path.join("level.dat").exists() || !world_path.join("db").join("CURRENT").exists() {
        eprintln!(
            "skipping large fixture test; copy a Bedrock world fixture to {}",
            world_path.display()
        );
        return;
    }

    let document = read_level_dat_document(&world_path.join("level.dat")).expect("read level.dat");
    assert!(document.header.actual_payload_len > 0);
    let root = match &document.root {
        NbtTag::Compound(root) => root,
        other => panic!("level.dat root must be a compound, got {other:?}"),
    };
    assert!(
        root.contains_key("LevelName"),
        "level.dat missing LevelName"
    );
    print_level_dat_summary(&document, root);

    let storage = Arc::new(BedrockLevelDbStorage::open(world_path.join("db")).expect("open db"));
    let db_summary = scan_db_summary(storage.as_ref()).expect("scan db summary");
    print_db_summary(&db_summary);
    assert!(
        db_summary.entry_count > 100,
        "expected the fixture world WAL to contain many records, got {}",
        db_summary.entry_count
    );
    assert!(
        db_summary.has_level_chunk_metadata,
        "fixture db did not expose LevelChunkMetaDataDictionary"
    );

    let world = BedrockWorld::from_storage(world_path, storage, OpenOptions::default());
    let first_chunk_pos = db_summary
        .first_data3d_pos
        .expect("fixture should contain a Data3D chunk");
    let parsed_chunk = world
        .parse_chunk_blocking(first_chunk_pos)
        .expect("parse sample chunk");
    print_parsed_chunk_summary(&parsed_chunk);
    assert!(
        parsed_chunk.report.parse_errors.is_empty(),
        "sample chunk parser reported errors: {:?}",
        parsed_chunk.report.parse_errors
    );
    assert!(
        parsed_chunk.report.entry_count > 0,
        "fixture sample chunk should include records"
    );
    assert!(
        parsed_chunk.report.subchunk_count > 0,
        "fixture sample chunk should include parsed subchunks"
    );

    let players = world.list_players_blocking().expect("list players");
    println!("players.count={}", players.len());
    for player in players.iter().take(16) {
        println!("player={player:?}");
    }
    assert!(
        players.is_empty() || players.iter().all(|player| player.storage_key().is_some()),
        "fixture players should be LevelDB-backed when present"
    );
}

fn print_level_dat_summary(
    document: &bedrock_world::LevelDatDocument,
    root: &indexmap::IndexMap<String, NbtTag>,
) {
    println!(
        "fixture.level_dat.header.version={}",
        document.header.version
    );
    println!(
        "fixture.level_dat.header.declared_len={}",
        document.header.declared_len
    );
    println!(
        "fixture.level_dat.header.actual_payload_len={}",
        document.header.actual_payload_len
    );
    println!("fixture.level_dat.warnings={:?}", document.warnings);

    for key in [
        "LevelName",
        "LastPlayed",
        "GameType",
        "Generator",
        "RandomSeed",
        "StorageVersion",
        "NetworkVersion",
        "lastOpenedWithVersion",
    ] {
        if let Some(value) = root.get(key) {
            println!("level_dat.{key}={}", nbt_preview(value));
        }
    }
}

fn print_parsed_chunk_summary(chunk: &bedrock_world::ParsedChunkData) {
    println!("parsed.sample_chunk.pos={:?}", chunk.pos);
    println!("parsed.sample_chunk.records={}", chunk.report.entry_count);
    println!(
        "parsed.sample_chunk.subchunks={}",
        chunk.report.subchunk_count
    );
    println!(
        "parsed.sample_chunk.subchunk_storages={}",
        chunk.report.subchunk_storage_count
    );
    println!(
        "parsed.sample_chunk.palette_states={}",
        chunk.report.palette_state_count
    );
    println!(
        "parsed.sample_chunk.block_entities={}",
        chunk.report.block_entity_count
    );
    println!(
        "parsed.sample_chunk.biomes.records={} storages={}",
        chunk.report.biome_record_count, chunk.report.biome_layer_count
    );
    println!("parsed.sample_chunk.key_kinds={:?}", chunk.report.key_kinds);
    println!("parsed.sample_chunk.errors={:?}", chunk.report.parse_errors);
}

#[derive(Default)]
struct DbSummary {
    entry_count: usize,
    total_key_bytes: usize,
    total_value_bytes: usize,
    key_kinds: BTreeMap<String, usize>,
    chunk_positions: BTreeMap<String, usize>,
    named_keys: Vec<String>,
    unknown_keys: Vec<String>,
    first_entries: Vec<String>,
    has_level_chunk_metadata: bool,
    first_data3d_pos: Option<ChunkPos>,
}

fn scan_db_summary(storage: &dyn WorldStorage) -> bedrock_world::Result<DbSummary> {
    let mut summary = DbSummary::default();
    storage.for_each_entry(StorageReadOptions::default(), &mut |key, value| {
        summary.entry_count += 1;
        summary.total_key_bytes += key.len();
        summary.total_value_bytes += value.len();
        summary.has_level_chunk_metadata |= key == b"LevelChunkMetaDataDictionary";
        let parsed_key = BedrockDbKey::decode(key);
        *summary
            .key_kinds
            .entry(parsed_key.summary_kind())
            .or_default() += 1;
        if let BedrockDbKey::Chunk(chunk_key) = &parsed_key {
            if summary.first_data3d_pos.is_none() && chunk_key.tag == ChunkRecordTag::Data3D {
                summary.first_data3d_pos = Some(chunk_key.pos);
            }
            *summary
                .chunk_positions
                .entry(format!(
                    "x={} z={} dim={:?}",
                    chunk_key.pos.x, chunk_key.pos.z, chunk_key.pos.dimension
                ))
                .or_default() += 1;
        } else {
            if matches!(parsed_key, BedrockDbKey::Unknown(_)) && summary.unknown_keys.len() < 24 {
                summary.unknown_keys.push(format!(
                    "{} value_len={}",
                    key_preview(key),
                    value.len()
                ));
            }
            if summary.named_keys.len() < 24 {
                summary.named_keys.push(format!("{parsed_key:?}"));
            }
        }
        if summary.first_entries.len() < 12 {
            summary.first_entries.push(format!(
                "parsed={parsed_key:?} raw_key={} value_len={}",
                key_preview(key),
                value.len()
            ));
        }
        Ok(StorageVisitorControl::Continue)
    })?;
    Ok(summary)
}

fn print_db_summary(summary: &DbSummary) {
    println!("db.entries.count={}", summary.entry_count);
    println!("db.entries.key_bytes={}", summary.total_key_bytes);
    println!("db.entries.value_bytes={}", summary.total_value_bytes);
    println!("db.key_kinds={:?}", summary.key_kinds);
    println!("db.chunk.positions.count={}", summary.chunk_positions.len());
    println!(
        "db.chunk.positions.first={:?}",
        summary.chunk_positions.iter().take(12).collect::<Vec<_>>()
    );
    println!("db.named_keys.first={:?}", summary.named_keys);
    println!("db.unknown_keys.first={:?}", summary.unknown_keys);
    for (index, entry) in summary.first_entries.iter().enumerate() {
        println!("db.entry[{index}].{entry}");
    }
}

fn nbt_preview(value: &NbtTag) -> String {
    match value {
        NbtTag::Byte(value) => value.to_string(),
        NbtTag::Short(value) => value.to_string(),
        NbtTag::Int(value) => value.to_string(),
        NbtTag::Long(value) => value.to_string(),
        NbtTag::Float(value) => value.to_string(),
        NbtTag::Double(value) => value.to_string(),
        NbtTag::String(value) => value.clone(),
        NbtTag::List(values) => format!("List(len={})", values.len()),
        NbtTag::Compound(values) => format!("Compound(len={})", values.len()),
        NbtTag::ByteArray(values) => format!("ByteArray(len={})", values.len()),
        NbtTag::IntArray(values) => format!("IntArray(len={})", values.len()),
        NbtTag::LongArray(values) => format!("LongArray(len={})", values.len()),
        NbtTag::ShortArray(values) => format!("ShortArray(len={})", values.len()),
        NbtTag::End => "End".to_string(),
    }
}

fn key_preview(key: &[u8]) -> String {
    if key
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
    {
        return String::from_utf8_lossy(key).into_owned();
    }
    key.iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
