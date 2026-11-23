//! Offline BDS palette compiler for version-indexed, block-level deduplicated assets.
//!
//! Run with `cargo run --release -p bedrock-world --bin bedrock-block-catalog --
//! compile --world <BDS world> --metadata <capture.json> --log <bds.log> --out <data dir>`.
//! A version index stores the NBT storage version separately from identical semantic objects.
use bedrock_world::block::BlockState;
use bedrock_world::chunk::{
    BedrockDbKey, ChunkRecordTag, SubChunk, SubChunkDecodeMode, SubChunkFormat,
};
use bedrock_world::nbt::{NbtTag, parse_root};
use bedrock_world::storage::{
    BedrockLevelDbStorage, StorageReadOptions, StorageVisitorControl, WorldStorage,
};
use bedrock_world::BedrockWorldError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use xxhash_rust::xxh3::xxh3_128;

type RunResult<T> = Result<T, Box<dyn Error>>;
const BLOCK_MAGIC: &[u8; 8] = b"BMCBLB01";
const PALETTE_MAGIC: &[u8; 8] = b"BMCBLP01";

#[derive(Debug, Deserialize)]
struct Capture {
    game_version: String,
    bds_sha256: String,
    source_url: String,
    capture_method: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CatalogIndex {
    format_version: u32,
    versions: BTreeMap<String, VersionEntry>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct VersionEntry {
    manifest: String,
    storage_version: i32,
    blocks: usize,
    states: usize,
    bds_sha256: String,
    capture_method: String,
}

#[derive(Default)]
struct Extracted {
    permutations: BTreeMap<String, Vec<BlockState>>,
    defaults: BTreeMap<String, BlockState>,
    structures: usize,
}

fn main() {
    if let Err(error) = execute() {
        eprintln!("bedrock-block-catalog: {error}");
        std::process::exit(1);
    }
}

fn execute() -> RunResult<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("compile") {
        return Err(failure("usage: bedrock-block-catalog compile --world PATH --metadata FILE --log FILE --out DIR [--replace]"));
    }
    let mut options = BTreeMap::<String, String>::new();
    let mut replace = false;
    let mut cursor = 2;
    while cursor < args.len() {
        if args[cursor] == "--replace" {
            replace = true;
            cursor += 1;
            continue;
        }
        let value = args.get(cursor + 1).ok_or_else(|| failure(format!("missing value for {}", args[cursor])))?;
        options.insert(args[cursor].clone(), value.clone());
        cursor += 2;
    }
    let option = |name: &str| -> RunResult<PathBuf> {
        Ok(PathBuf::from(options.get(name).ok_or_else(|| failure(format!("required argument {name} is missing")))?))
    };
    let world = option("--world")?;
    let output = option("--out")?;
    let metadata: Capture = serde_json::from_slice(&fs::read(option("--metadata")?)?)?;
    if metadata.capture_method != "script_api_structure" {
        return Err(failure("capture method is not the verified BDS structure exporter"));
    }
    if metadata.game_version.split('.').count() != 4
        || !metadata.game_version.chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        return Err(failure("invalid or missing four-component BDS game version"));
    }
    if !metadata.source_url.starts_with("https://www.minecraft.net/bedrockdedicatedserver/bin-linux/") {
        return Err(failure("BDS metadata must reference the official Linux download"));
    }
    if metadata.bds_sha256.len() != 64 || !metadata.bds_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(failure("missing BDS archive SHA256"));
    }
    let log = fs::read_to_string(option("--log")?)?;
    let summary = log.lines().filter_map(|line| {
        line.split_once("BMCBL-DUMP-END ").and_then(|(_, tail)| serde_json::from_str::<serde_json::Value>(tail.trim()).ok())
    }).last().ok_or_else(|| failure("BDS log lacks a valid BMCBL-DUMP-END marker"))?;
    if log.contains("BMCBL-DUMP-FAIL ") {
        return Err(failure("script exporter reported failure"));
    }
    let expected_blocks = summary["blocks"].as_u64().ok_or_else(|| failure("capture report lacks blocks"))?;
    let expected_structures = summary["structures"].as_u64().ok_or_else(|| failure("capture report lacks structures"))?;
    let expected_placed = summary["placed"].as_u64().ok_or_else(|| failure("capture report lacks placed"))?;
    if expected_blocks < 1000 || expected_placed < 10000 {
        return Err(failure("capture below safety floor (1000 blocks / 10000 placed states)"));
    }
    let mut extracted = extract(&world)?;
    if extracted.structures as u64 != expected_structures {
        return Err(failure(format!(
            "missing persisted structures: found {}, expected {expected_structures}",
            extracted.structures
        )));
    }
    // Most blocks can be stored in a structure. This special block must be sampled
    // from a real SubChunk; do not silently invent its storage version.
    if !extracted.permutations.contains_key("minecraft:structure_void") {
        extract_structure_void(&world, &mut extracted)?;
    }
    let known_blocks = extracted.defaults.keys().collect::<BTreeSet<_>>();
    let sampled_blocks = extracted.permutations.keys().collect::<BTreeSet<_>>();
    if known_blocks != sampled_blocks || known_blocks.len() as u64 != expected_blocks {
        let missing = known_blocks.difference(&sampled_blocks).take(10).collect::<Vec<_>>();
        let unexpected = sampled_blocks.difference(&known_blocks).take(10).collect::<Vec<_>>();
        return Err(failure(format!(
            "incomplete default/permutation coverage; defaults={}, permutations={}, expected={expected_blocks}; missing={missing:?}, unexpected={unexpected:?}",
            known_blocks.len(), sampled_blocks.len()
        )));
    }
    let mut storage_version = None;
    let mut manifest = Vec::new();
    manifest.extend_from_slice(PALETTE_MAGIC);
    write_var(&mut manifest, extracted.permutations.len() as u64);
    let mut objects = BTreeMap::<String, Vec<u8>>::new();
    let mut total_states = 0;
    for (name, states) in &extracted.permutations {
        let default = extracted.defaults.get(name).ok_or_else(|| failure(format!("no default for {name}")))?;
        let mut unique = BTreeMap::<Vec<u8>, BlockState>::new();
        for state in states {
            let version = state.version.ok_or_else(|| failure(format!("{name} has no NBT version")))?;
            if let Some(expected) = storage_version {
                if expected != version {
                    return Err(failure(format!(
                        "mixed NBT storage versions: {expected} versus {version} ({name})"
                    )));
                }
            } else {
                storage_version = Some(version);
            }
            unique.insert(semantic_key(state)?, state.clone());
        }
        let canonical = unique.into_values().collect::<Vec<_>>();
        let default_index = canonical.iter().position(|state| state.states == default.states)
            .ok_or_else(|| failure(format!("default state absent from target palette: {name}")))?;
        let block_bytes = encode_block(&canonical)?;
        let digest = object_id(&block_bytes);
        if let Some(previous) = objects.insert(digest.clone(), block_bytes.clone()) {
            if previous != block_bytes {
                return Err(failure("XXH3-128 collision detected between different block objects"));
            }
        }
        write_text(&mut manifest, name);
        manifest.extend_from_slice(&xxh3_128(&block_bytes).to_le_bytes());
        write_var(&mut manifest, default_index as u64);
        total_states += canonical.len();
    }
    if total_states < 10000 {
        return Err(failure(format!("only {total_states} distinct states collected")));
    }
    let version_entry = VersionEntry {
        manifest: object_id(&manifest),
        storage_version: storage_version.ok_or_else(|| failure("empty palette"))?,
        blocks: extracted.permutations.len(),
        states: total_states,
        bds_sha256: metadata.bds_sha256,
        capture_method: metadata.capture_method,
    };
    let index_path = output.join("index.json");
    let mut index: CatalogIndex = if index_path.is_file() {
        serde_json::from_slice(&fs::read(&index_path)?)?
    } else {
        CatalogIndex { format_version: 1, ..CatalogIndex::default() }
    };
    if index.format_version != 1 {
        return Err(failure(format!("unsupported block-catalog index {}", index.format_version)));
    }
    if let Some(existing) = index.versions.get(&metadata.game_version) {
        if existing != &version_entry && !replace {
            return Err(failure(format!(
                "BDS {} already exists with different evidence; use --replace after review",
                metadata.game_version
            )));
        }
    }
    for (id, bytes) in &objects {
        store_object(&output.join("objects"), id, bytes)?;
    }
    store_object(&output.join("manifests"), &version_entry.manifest, &manifest)?;
    index.versions.insert(metadata.game_version.clone(), version_entry);
    fs::create_dir_all(&output)?;
    let temporary = output.join("index.json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(&index)?)?;
    fs::rename(temporary, index_path)?;
    println!(
        "BDS {}: {} block types, {} states, {} block objects, manifest {}; index contains {} versions",
        metadata.game_version, extracted.permutations.len(), total_states, objects.len(),
        object_id(&manifest), index.versions.len()
    );
    Ok(())
}

fn extract(world: &Path) -> RunResult<Extracted> {
    let storage = BedrockLevelDbStorage::open_read_only(world.join("db"))?;
    let mut collected = Extracted::default();
    storage.for_each_prefix(
        b"structuretemplate_bmcbl:", StorageReadOptions::default(),
        &mut |key, value| {
            let id = std::str::from_utf8(key).map_err(|error| {
                BedrockWorldError::Validation(format!("structure key is not UTF-8: {error}"))
            })?;
            let suffix = id.strip_prefix("structuretemplate_bmcbl:").ok_or_else(|| {
                BedrockWorldError::Validation(format!("unexpected structure key: {id}"))
            })?;
            let is_default = suffix.starts_with('d');
            let is_permutation = suffix.starts_with('p');
            if !is_default && !is_permutation {
                return Err(BedrockWorldError::Validation(format!("unknown structure kind: {id}")));
            }
            let root = parse_root(value)?;
            let list = structure_palette(&root)?;
            for state in list {
                if is_default {
                    if let Some(previous) = collected.defaults.insert(state.name.clone(), state.clone()) {
                        if previous != state {
                            return Err(BedrockWorldError::Validation(format!(
                                "conflicting default for {}", state.name
                            )));
                        }
                    }
                } else {
                    collected.permutations.entry(state.name.clone()).or_default().push(state);
                }
            }
            collected.structures += 1;
            Ok(StorageVisitorControl::Continue)
        },
    )?;
    if collected.structures == 0 {
        return Err(failure("no bmcbl structures in LevelDB"));
    }
    Ok(collected)
}

fn structure_palette(root: &NbtTag) -> bedrock_world::Result<Vec<BlockState>> {
    let structure = member(root, "structure")?;
    let palette = member(structure, "palette")?;
    let default = member(palette, "default")?;
    let entries = member(default, "block_palette")?;
    let NbtTag::List(entries) = entries else {
        return Err(BedrockWorldError::Validation("block_palette must be a list".to_owned()));
    };
    entries.iter().map(|entry| {
        let NbtTag::String(name) = member(entry, "name")? else {
            return Err(BedrockWorldError::Validation("block name must be string".into()));
        };
        let NbtTag::Compound(states) = member(entry, "states")? else {
            return Err(BedrockWorldError::Validation(format!("{name}: states must be compound")));
        };
        let NbtTag::Int(version) = member(entry, "version")? else {
            return Err(BedrockWorldError::Validation(format!("{name}: version must be int")));
        };
        Ok(BlockState {
            name: name.clone(),
            states: states.iter().map(|(name, value)| (name.clone(), value.clone())).collect(),
            version: Some(*version),
        })
    }).collect()
}

fn member<'a>(root: &'a NbtTag, name: &str) -> bedrock_world::Result<&'a NbtTag> {
    let NbtTag::Compound(fields) = root else {
        return Err(BedrockWorldError::Validation("expected NBT compound".into()));
    };
    fields.get(name).ok_or_else(|| BedrockWorldError::Validation(format!("missing NBT member {name}")))
}

fn extract_structure_void(world: &Path, result: &mut Extracted) -> RunResult<()> {
    let storage = BedrockLevelDbStorage::open_read_only(world.join("db"))?;
    let mut found: Option<BlockState> = None;
    storage.for_each_entry(StorageReadOptions::default(), &mut |key, value| {
        let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) else {
            return Ok(StorageVisitorControl::Continue);
        };
        if chunk_key.tag != ChunkRecordTag::SubChunkPrefix {
            return Ok(StorageVisitorControl::Continue);
        }
        let y = chunk_key.subchunk_y.unwrap_or(0);
        let chunk = SubChunk::read(y, value.clone(), SubChunkDecodeMode::CountsOnly)?;
        if let SubChunkFormat::Paletted { storages, .. } = chunk.format {
            for layer in storages {
                for state in layer.states {
                    if state.name == "minecraft:structure_void" {
                        if let Some(previous) = &found {
                            if previous != &state {
                                return Err(BedrockWorldError::Validation(
                                    "multiple conflicting structure_void states".into()
                                ));
                            }
                        }
                        found = Some(state);
                    }
                }
            }
        }
        Ok(StorageVisitorControl::Continue)
    })?;
    let state = found.ok_or_else(|| failure("structure_void was not persisted in any SubChunk"))?;
    result.defaults.insert(state.name.clone(), state.clone());
    result.permutations.insert(state.name.clone(), vec![state]);
    Ok(())
}

/// Semantic key retains NBT type information and never includes storage version.
fn semantic_key(state: &BlockState) -> RunResult<Vec<u8>> {
    let mut bytes = Vec::new();
    write_var(&mut bytes, state.states.len() as u64);
    for (name, value) in &state.states {
        write_text(&mut bytes, name);
        write_tag(&mut bytes, value)?;
    }
    Ok(bytes)
}

/// Compact per-block state matrix with shared property names and typed value domains.
///
/// BMCBLB01, property-count, [name, value-count, typed values]*,
/// state-count, then one unsigned LEB128 index per (state, property).
/// Index 0 = property missing, 1..N = dictionary value. No version is stored.
fn encode_block(states: &[BlockState]) -> RunResult<Vec<u8>> {
    let mut fields = BTreeSet::new();
    for state in states {
        fields.extend(state.states.keys().cloned());
    }
    let fields = fields.into_iter().collect::<Vec<_>>();
    let mut out = Vec::new();
    out.extend_from_slice(BLOCK_MAGIC);
    write_var(&mut out, fields.len() as u64);
    let mut domains = Vec::<BTreeMap<Vec<u8>, u64>>::new();
    for name in &fields {
        write_text(&mut out, name);
        let mut values = BTreeSet::<Vec<u8>>::new();
        for state in states {
            if let Some(tag) = state.states.get(name) {
                let mut encoded = Vec::new();
                write_tag(&mut encoded, tag)?;
                values.insert(encoded);
            }
        }
        write_var(&mut out, values.len() as u64);
        let mut lookup = BTreeMap::new();
        for (position, encoded) in values.into_iter().enumerate() {
            out.extend_from_slice(&encoded);
            lookup.insert(encoded, position as u64 + 1);
        }
        domains.push(lookup);
    }
    write_var(&mut out, states.len() as u64);
    for state in states {
        for (field, domain) in fields.iter().zip(&domains) {
            let ordinal = if let Some(value) = state.states.get(field) {
                let mut encoded = Vec::new();
                write_tag(&mut encoded, value)?;
                *domain.get(&encoded).ok_or_else(|| failure("property dictionary was corrupted"))?
            } else {
                0
            };
            write_var(&mut out, ordinal);
        }
    }
    Ok(out)
}

fn write_tag(into: &mut Vec<u8>, value: &NbtTag) -> RunResult<()> {
    match value {
        NbtTag::Byte(v) => into.extend_from_slice(&[1, *v as u8]),
        NbtTag::Short(v) => { into.push(2); into.extend_from_slice(&v.to_le_bytes()); }
        NbtTag::Int(v) => { into.push(3); into.extend_from_slice(&v.to_le_bytes()); }
        NbtTag::Long(v) => { into.push(4); into.extend_from_slice(&v.to_le_bytes()); }
        NbtTag::Float(v) => { into.push(5); into.extend_from_slice(&v.to_le_bytes()); }
        NbtTag::Double(v) => { into.push(6); into.extend_from_slice(&v.to_le_bytes()); }
        NbtTag::String(v) => { into.push(8); write_text(into, v); }
        _ => return Err(failure("BlockState contains an unsupported NBT property type")),
    }
    Ok(())
}

fn write_text(into: &mut Vec<u8>, value: &str) {
    write_var(into, value.len() as u64);
    into.extend_from_slice(value.as_bytes());
}

fn write_var(into: &mut Vec<u8>, mut value: u64) {
    while value >= 128 {
        into.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    into.push(value as u8);
}

fn object_id(bytes: &[u8]) -> String {
    format!("{:032x}", xxh3_128(bytes))
}

fn store_object(dir: &Path, id: &str, bytes: &[u8]) -> RunResult<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{id}.bin"));
    if path.exists() {
        if fs::read(&path)? != bytes {
            return Err(failure(format!("object digest collision or corruption: {}", path.display())));
        }
    } else {
        fs::write(path, bytes)?;
    }
    Ok(())
}

fn failure(message: impl Into<String>) -> Box<dyn Error> {
    std::io::Error::other(message.into()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn equal_content_has_equal_digest_regardless_of_storage_version() {
        let mut first = BlockState {
            name: "minecraft:example".into(),
            states: BTreeMap::from([
                ("open_bit".into(), NbtTag::Byte(1)),
                ("facing_direction".into(), NbtTag::Int(2)),
            ]),
            version: Some(17),
        };
        let original = encode_block(&[first.clone()]).unwrap();
        first.version = Some(26);
        assert_eq!(original, encode_block(&[first]).unwrap());
        assert_eq!(object_id(&original), object_id(&original));
    }
    #[test]
    fn type_distinctions_are_preserved() {
        let mut state = BlockState {
            name: "minecraft:example".into(),
            states: BTreeMap::from([("p".into(), NbtTag::Byte(1))]),
            version: Some(42),
        };
        let byte = encode_block(&[state.clone()]).unwrap();
        state.states.insert("p".into(), NbtTag::Int(1));
        let integer = encode_block(&[state]).unwrap();
        assert_ne!(byte, integer);
    }
}
