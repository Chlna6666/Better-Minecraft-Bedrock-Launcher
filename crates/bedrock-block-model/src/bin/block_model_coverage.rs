use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use bedrock_block_model::{
    BlockStateQuery, BlockStateValue, ModelFamily, canonical_block_name_for_state,
    model_family_for_block_name, model_shape_for_block_state,
};
use serde::Serialize;
use serde_json::Value;
use walkdir::WalkDir;

type Result<T> = std::result::Result<T, String>;

const DEFAULT_LIMIT: usize = 80;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let options = Options::parse(env::args().skip(1))?;
    let states = discover_block_states(&options.roots)?;
    let report = build_report(&states);
    if options.json {
        let output = serde_json::to_string_pretty(&report)
            .map_err(|source| format!("failed to serialize coverage report: {source}"))?;
        println!("{output}");
    } else {
        print_text_report(&report, options.limit);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Options {
    roots: Vec<PathBuf>,
    json: bool,
    limit: usize,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut roots = Vec::new();
        let mut json = false;
        let mut limit = DEFAULT_LIMIT;
        let mut args = args.into_iter();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--json" => json = true,
                "--limit" => {
                    let Some(value) = args.next() else {
                        return Err(usage("--limit requires a number"));
                    };
                    limit = value
                        .parse::<usize>()
                        .map_err(|_| usage("--limit requires a positive integer"))?;
                }
                "-h" | "--help" => return Err(usage("")),
                _ if arg.starts_with('-') => {
                    return Err(usage(&format!("unknown option: {arg}")));
                }
                _ => roots.push(PathBuf::from(arg)),
            }
        }

        if roots.is_empty() {
            return Err(usage("missing bedrock-samples or resource-pack path"));
        }

        Ok(Self { roots, json, limit })
    }
}

fn usage(message: &str) -> String {
    let prefix = if message.is_empty() {
        String::new()
    } else {
        format!("{message}\n\n")
    };
    format!(
        "{prefix}usage: cargo run --bin block_model_coverage -- [--json] [--limit N] <bedrock-samples-or-pack-root>..."
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DiscoveredBlockState {
    name: String,
    states: BTreeMap<String, BlockStateValue>,
    source: String,
}

fn discover_block_states(roots: &[PathBuf]) -> Result<Vec<DiscoveredBlockState>> {
    let mut discovered = BTreeMap::new();
    for root in roots {
        if !root.exists() {
            return Err(format!("input path does not exist: {}", root.display()));
        }
        discover_vanilla_metadata(root, &mut discovered)?;
        discover_resource_pack_blocks(root, &mut discovered)?;
        discover_custom_block_files(root, &mut discovered)?;
    }
    Ok(discovered.into_values().collect())
}

fn discover_vanilla_metadata(
    root: &Path,
    discovered: &mut BTreeMap<String, DiscoveredBlockState>,
) -> Result<()> {
    for path in json_files_named(root, "mojang-blocks.json")? {
        let value = read_json_value(&path)?;
        collect_mojang_blocks(&value, &path, discovered);
    }
    Ok(())
}

fn discover_resource_pack_blocks(
    root: &Path,
    discovered: &mut BTreeMap<String, DiscoveredBlockState>,
) -> Result<()> {
    for path in json_files_named(root, "blocks.json")? {
        if !looks_like_resource_pack_blocks_json(&path) {
            continue;
        }
        let value = read_json_value(&path)?;
        collect_legacy_blocks_json(&value, &path, discovered);
    }
    Ok(())
}

fn discover_custom_block_files(
    root: &Path,
    discovered: &mut BTreeMap<String, DiscoveredBlockState>,
) -> Result<()> {
    for path in json_files(root)? {
        if !path_is_under_block_definition_folder(&path) {
            continue;
        }
        let value = read_json_value(&path)?;
        collect_custom_block(&value, &path, discovered);
    }
    Ok(())
}

fn json_files_named(root: &Path, file_name: &str) -> Result<Vec<PathBuf>> {
    Ok(json_files(root)?
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case(file_name))
        })
        .collect())
}

fn json_files(root: &Path) -> Result<Vec<PathBuf>> {
    if root.is_file() {
        return Ok(is_json_file(root)
            .then(|| root.to_path_buf())
            .into_iter()
            .collect());
    }

    let mut paths = Vec::new();
    for entry in WalkDir::new(root) {
        let entry =
            entry.map_err(|source| format!("failed to walk {}: {source}", root.display()))?;
        let path = entry.path();
        if is_json_file(path) {
            paths.push(path.to_path_buf());
        }
    }
    Ok(paths)
}

fn is_json_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

fn looks_like_resource_pack_blocks_json(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    parent.join("textures").exists()
        || path
            .components()
            .any(|component| component.as_os_str().eq_ignore_ascii_case("resource_pack"))
}

fn path_is_under_block_definition_folder(path: &Path) -> bool {
    let components: Vec<_> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect();
    components.windows(2).any(|items| {
        matches!(
            items,
            ["definitions", "blocks"] | ["behavior_pack", "blocks"]
        )
    }) || components
        .windows(2)
        .any(|items| matches!(items, [parent, "blocks"] if !is_resource_subfolder(parent)))
}

fn is_resource_subfolder(name: &str) -> bool {
    matches!(
        name,
        "textures" | "models" | "sounds" | "ui" | "render_controllers" | "particles" | "biomes"
    )
}

fn collect_mojang_blocks(
    value: &Value,
    path: &Path,
    discovered: &mut BTreeMap<String, DiscoveredBlockState>,
) {
    let property_values = vanilla_property_values(value);
    let Some(items) = value.get("data_items").and_then(Value::as_array) else {
        return;
    };

    for item in items {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        let property_names = item
            .get("properties")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|property| property.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();

        for states in expand_property_states(&property_names, &property_values) {
            insert_discovered(name, states, path, "vanilla_data", discovered);
        }
    }
}

fn vanilla_property_values(value: &Value) -> BTreeMap<String, Vec<BlockStateValue>> {
    let mut values = BTreeMap::new();
    let Some(properties) = value.get("block_properties").and_then(Value::as_array) else {
        return values;
    };

    for property in properties {
        let Some(name) = property.get("name").and_then(Value::as_str) else {
            continue;
        };
        let property_values = property
            .get("values")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("value").and_then(block_state_value_from_json))
            .collect::<Vec<_>>();
        values.insert(name.to_owned(), property_values);
    }

    values
}

fn expand_property_states(
    property_names: &[&str],
    property_values: &BTreeMap<String, Vec<BlockStateValue>>,
) -> Vec<BTreeMap<String, BlockStateValue>> {
    let mut combinations = vec![BTreeMap::new()];
    for property_name in property_names {
        let Some(values) = property_values.get(*property_name) else {
            continue;
        };
        if values.is_empty() {
            continue;
        }
        let mut next = Vec::with_capacity(combinations.len() * values.len());
        for states in &combinations {
            for value in values {
                let mut expanded = states.clone();
                expanded.insert((*property_name).to_owned(), value.clone());
                next.push(expanded);
            }
        }
        combinations = next;
    }
    combinations
}

fn collect_legacy_blocks_json(
    value: &Value,
    path: &Path,
    discovered: &mut BTreeMap<String, DiscoveredBlockState>,
) {
    let Some(object) = value.as_object() else {
        return;
    };

    for key in object.keys() {
        if is_metadata_key(key) {
            continue;
        }
        insert_discovered(key, BTreeMap::new(), path, "blocks_json", discovered);
    }
}

fn collect_custom_block(
    value: &Value,
    path: &Path,
    discovered: &mut BTreeMap<String, DiscoveredBlockState>,
) {
    let Some(block) = value.get("minecraft:block") else {
        return;
    };
    let Some(description) = block.get("description") else {
        return;
    };
    let Some(identifier) = description.get("identifier").and_then(Value::as_str) else {
        return;
    };
    let state_values = custom_state_values(description);
    let property_names = state_values.keys().map(String::as_str).collect::<Vec<_>>();
    for states in expand_property_states(&property_names, &state_values) {
        insert_discovered(identifier, states, path, "custom_block", discovered);
    }
}

fn custom_state_values(description: &Value) -> BTreeMap<String, Vec<BlockStateValue>> {
    let mut values = BTreeMap::new();
    let Some(states) = description.get("states").and_then(Value::as_object) else {
        return values;
    };

    for (name, value) in states {
        let items = value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(block_state_value_from_json)
            .collect::<Vec<_>>();
        values.insert(name.clone(), items);
    }
    values
}

fn insert_discovered(
    name: &str,
    states: BTreeMap<String, BlockStateValue>,
    path: &Path,
    source_kind: &str,
    discovered: &mut BTreeMap<String, DiscoveredBlockState>,
) {
    let name = normalize_block_name(name);
    let key = state_key(&name, &states);
    discovered
        .entry(key)
        .or_insert_with(|| DiscoveredBlockState {
            name,
            states,
            source: format!("{source_kind}:{}", path.display()),
        });
}

fn normalize_block_name(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

fn state_key(name: &str, states: &BTreeMap<String, BlockStateValue>) -> String {
    let mut key = name.to_owned();
    if states.is_empty() {
        return key;
    }
    key.push('[');
    for (index, (state_name, value)) in states.iter().enumerate() {
        if index > 0 {
            key.push(',');
        }
        key.push_str(state_name);
        key.push('=');
        key.push_str(&state_value_string(value));
    }
    key.push(']');
    key
}

fn state_value_string(value: &BlockStateValue) -> String {
    match value {
        BlockStateValue::Bool(value) => value.to_string(),
        BlockStateValue::Int(value) => value.to_string(),
        BlockStateValue::String(value) => value.clone(),
    }
}

fn block_state_value_from_json(value: &Value) -> Option<BlockStateValue> {
    match value {
        Value::Bool(value) => Some(BlockStateValue::Bool(*value)),
        Value::Number(value) => value.as_i64().map(BlockStateValue::Int),
        Value::String(value) => Some(BlockStateValue::String(value.clone())),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn is_metadata_key(key: &str) -> bool {
    matches!(
        key,
        "format_version"
            | "resource_pack_name"
            | "texture_name"
            | "terrain_name"
            | "num_mip_levels"
            | "padding"
    )
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
enum CoverageStatus {
    CoveredFamily,
    FallbackFullBlock,
    Unknown,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
struct CountStats {
    blocks: usize,
    states: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
struct MutableStats {
    blocks: BTreeSet<String>,
    states: usize,
}

impl MutableStats {
    fn observe(&mut self, name: &str) {
        self.blocks.insert(name.to_owned());
        self.states += 1;
    }

    fn counts(&self) -> CountStats {
        CountStats {
            blocks: self.blocks.len(),
            states: self.states,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct CoverageReport {
    summary: BTreeMap<String, CountStats>,
    families: BTreeMap<String, CountStats>,
    fallback_full_blocks: Vec<String>,
    unknown_blocks: Vec<String>,
    sources: BTreeMap<String, usize>,
}

fn build_report(states: &[DiscoveredBlockState]) -> CoverageReport {
    let mut summary: BTreeMap<CoverageStatus, MutableStats> = BTreeMap::new();
    let mut families: BTreeMap<String, MutableStats> = BTreeMap::new();
    let mut sources = BTreeMap::new();

    for state in states {
        let query = BlockStateQuery {
            name: state.name.clone(),
            states: state.states.clone(),
        };
        let canonical_name = canonical_block_name_for_state(&query);
        let canonical_query = BlockStateQuery {
            name: canonical_name.clone(),
            states: state.states.clone(),
        };
        let family = model_family_for_block_name(&canonical_name);
        let status = coverage_status(family, &canonical_query);

        summary.entry(status).or_default().observe(&canonical_name);
        families
            .entry(format!("{family:?}"))
            .or_default()
            .observe(&canonical_name);
        *sources.entry(state.source.clone()).or_insert(0) += 1;
    }

    let fallback_full_blocks = summary
        .get(&CoverageStatus::FallbackFullBlock)
        .map(|stats| stats.blocks.iter().cloned().collect())
        .unwrap_or_default();
    let unknown_blocks = summary
        .get(&CoverageStatus::Unknown)
        .map(|stats| stats.blocks.iter().cloned().collect())
        .unwrap_or_default();

    CoverageReport {
        summary: summary
            .into_iter()
            .map(|(status, stats)| (format!("{status:?}"), stats.counts()))
            .collect(),
        families: families
            .into_iter()
            .map(|(family, stats)| (family, stats.counts()))
            .collect(),
        fallback_full_blocks,
        unknown_blocks,
        sources,
    }
}

fn coverage_status(family: ModelFamily, state: &BlockStateQuery) -> CoverageStatus {
    let Some(shape) = model_shape_for_block_state(state) else {
        return CoverageStatus::Unknown;
    };
    if shape.is_empty() {
        return CoverageStatus::Unknown;
    }
    if family == ModelFamily::FullBlock {
        CoverageStatus::FallbackFullBlock
    } else {
        CoverageStatus::CoveredFamily
    }
}

fn print_text_report(report: &CoverageReport, limit: usize) {
    println!("bedrock-block-model coverage");
    println!();
    println!("summary:");
    for (status, stats) in &report.summary {
        println!(
            "  {status}: {} blocks, {} states",
            stats.blocks, stats.states
        );
    }
    println!();
    println!("families:");
    for (family, stats) in &report.families {
        println!(
            "  {family}: {} blocks, {} states",
            stats.blocks, stats.states
        );
    }
    print_block_list(
        "fallback full-block review list",
        &report.fallback_full_blocks,
        limit,
    );
    print_block_list("unknown shape list", &report.unknown_blocks, limit);
}

fn print_block_list(title: &str, blocks: &[String], limit: usize) {
    println!();
    println!("{title}: {} blocks", blocks.len());
    for block in blocks.iter().take(limit) {
        println!("  {block}");
    }
    if blocks.len() > limit {
        println!("  ... {} more", blocks.len() - limit);
    }
}

fn read_json_value(path: &Path) -> Result<Value> {
    let content = fs::read_to_string(path)
        .map_err(|source| format!("failed to read {}: {source}", path.display()))?;
    let relaxed_content = strip_json_comments_and_trailing_commas(&content);
    serde_json::from_str(&relaxed_content)
        .map_err(|source| format!("failed to parse JSON {}: {source}", path.display()))
}

fn strip_json_comments_and_trailing_commas(content: &str) -> String {
    let mut without_comments = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(character) = chars.next() {
        if in_string {
            without_comments.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        if character == '"' {
            in_string = true;
            without_comments.push(character);
            continue;
        }

        if character == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    chars.next();
                    for next_character in chars.by_ref() {
                        if next_character == '\n' {
                            without_comments.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    for next_character in chars.by_ref() {
                        if previous == '*' && next_character == '/' {
                            break;
                        }
                        if next_character == '\n' {
                            without_comments.push('\n');
                        }
                        previous = next_character;
                    }
                    continue;
                }
                _ => {}
            }
        }

        without_comments.push(character);
    }

    remove_trailing_commas(&without_comments)
}

fn remove_trailing_commas(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(character) = chars.next() {
        if in_string {
            output.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        if character == '"' {
            in_string = true;
            output.push(character);
            continue;
        }

        if character == ',' {
            let mut lookahead = chars.clone();
            while matches!(lookahead.peek(), Some(next) if next.is_whitespace()) {
                lookahead.next();
            }
            if matches!(lookahead.peek(), Some('}' | ']')) {
                continue;
            }
        }

        output.push(character);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mojang_blocks_should_expand_property_values() {
        let value: Value = serde_json::from_str(
            r#"{
                "block_properties": [
                    {
                        "name": "powered_bit",
                        "values": [{ "value": false }, { "value": true }]
                    },
                    {
                        "name": "facing_direction",
                        "values": [{ "value": 0 }, { "value": 1 }, { "value": 2 }]
                    }
                ],
                "data_items": [{
                    "name": "minecraft:test_block",
                    "properties": [
                        { "name": "powered_bit" },
                        { "name": "facing_direction" }
                    ]
                }]
            }"#,
        )
        .expect("test JSON should parse");
        let mut discovered = BTreeMap::new();

        collect_mojang_blocks(&value, Path::new("mojang-blocks.json"), &mut discovered);

        assert_eq!(discovered.len(), 6);
        assert!(
            discovered.contains_key("minecraft:test_block[facing_direction=2,powered_bit=true]")
        );
    }

    #[test]
    fn legacy_blocks_json_should_ignore_metadata_keys() {
        let value: Value = serde_json::from_str(
            r#"{
                "format_version": "1.21.0",
                "stone": { "textures": "stone" },
                "minecraft:oak_stairs": { "textures": "oak_planks" }
            }"#,
        )
        .expect("test JSON should parse");
        let mut discovered = BTreeMap::new();

        collect_legacy_blocks_json(&value, Path::new("blocks.json"), &mut discovered);

        assert!(discovered.contains_key("minecraft:stone"));
        assert!(discovered.contains_key("minecraft:oak_stairs"));
        assert_eq!(discovered.len(), 2);
    }

    #[test]
    fn block_definition_folder_detection_should_ignore_texture_blocks_folder() {
        assert!(path_is_under_block_definition_folder(Path::new(
            "behavior_pack/blocks/example.json"
        )));
        assert!(path_is_under_block_definition_folder(Path::new(
            "definitions/blocks/example.json"
        )));
        assert!(!path_is_under_block_definition_folder(Path::new(
            "resource_pack/textures/blocks/example.texture_set.json"
        )));
        assert!(!path_is_under_block_definition_folder(Path::new(
            "resource_pack/models/blocks/example.json"
        )));
    }

    #[test]
    fn coverage_report_should_split_detail_and_fallback_families() {
        let states = vec![
            DiscoveredBlockState {
                name: "minecraft:oak_stairs".to_owned(),
                states: BTreeMap::new(),
                source: "test".to_owned(),
            },
            DiscoveredBlockState {
                name: "minecraft:stone".to_owned(),
                states: BTreeMap::new(),
                source: "test".to_owned(),
            },
        ];

        let report = build_report(&states);

        assert_eq!(
            report.summary["CoveredFamily"],
            CountStats {
                blocks: 1,
                states: 1
            }
        );
        assert_eq!(report.fallback_full_blocks, vec!["minecraft:stone"]);
    }
}
