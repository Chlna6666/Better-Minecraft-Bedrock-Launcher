use bedrock_render::BedrockRenderError;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const BLOCK_JSON: &str = "data/colors/bedrock-block-color.json";
const BIOME_JSON: &str = "data/colors/bedrock-biome-color.json";
const ALIAS_JSON: &str = "data/colors/bedrock-resource-pack-aliases.json";
const PALETTE_VERSION: &str = "1.21.x";
const PALETTE_SCHEMA_VERSION: u8 = 3;

type Result<T> = bedrock_render::Result<T>;

#[derive(Debug, Clone, Copy)]
enum Command {
    Audit,
    GenerateCleanRoom,
    Normalize,
    DeriveFromResourcePack,
}

#[derive(Debug)]
struct Config {
    command: Command,
    check: bool,
    block_json: PathBuf,
    biome_json: PathBuf,
    #[cfg_attr(not(feature = "png"), allow(dead_code))]
    resource_pack: Option<PathBuf>,
    #[cfg_attr(not(feature = "png"), allow(dead_code))]
    out: Option<PathBuf>,
    write: bool,
    #[cfg_attr(not(feature = "png"), allow(dead_code))]
    write_defaults: bool,
}

fn main() -> Result<()> {
    let config = Config::parse()?;
    match config.command {
        Command::Audit => audit(&config),
        Command::GenerateCleanRoom => generate_clean_room(&config),
        Command::Normalize => normalize(&config),
        Command::DeriveFromResourcePack => derive_from_resource_pack(&config),
    }
}

impl Config {
    fn parse() -> Result<Self> {
        let mut args = std::env::args().skip(1);
        let Some(command) = args.next() else {
            print_usage();
            return Err(validation("missing command"));
        };
        let command = match command.as_str() {
            "audit" => Command::Audit,
            "generate-clean-room" => Command::GenerateCleanRoom,
            "normalize" => Command::Normalize,
            "derive-from-resource-pack" => Command::DeriveFromResourcePack,
            "--help" | "-h" => {
                print_usage();
                return Ok(Self {
                    command: Command::Audit,
                    check: true,
                    block_json: PathBuf::from(BLOCK_JSON),
                    biome_json: PathBuf::from(BIOME_JSON),
                    resource_pack: None,
                    out: None,
                    write: false,
                    write_defaults: false,
                });
            }
            other => {
                print_usage();
                return Err(validation(format!("unknown command: {other}")));
            }
        };

        let mut check = false;
        let mut block_json = PathBuf::from(BLOCK_JSON);
        let mut biome_json = PathBuf::from(BIOME_JSON);
        let mut resource_pack = None;
        let mut out = None;
        let mut write = false;
        let mut write_defaults = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--check" => check = true,
                "--write" => write = true,
                "--write-defaults" => write_defaults = true,
                "--block-json" => {
                    block_json = PathBuf::from(next_arg(&mut args, "--block-json")?);
                }
                "--biome-json" => {
                    biome_json = PathBuf::from(next_arg(&mut args, "--biome-json")?);
                }
                "--pack" => {
                    resource_pack = Some(PathBuf::from(next_arg(&mut args, "--pack")?));
                }
                "--out" => {
                    out = Some(PathBuf::from(next_arg(&mut args, "--out")?));
                }
                other => return Err(validation(format!("unknown argument: {other}"))),
            }
        }
        if check && write {
            return Err(validation("--check and --write are mutually exclusive"));
        }
        if matches!(command, Command::GenerateCleanRoom) && !check && !write {
            return Err(validation(
                "generate-clean-room requires either --check or --write",
            ));
        }
        if matches!(command, Command::DeriveFromResourcePack) {
            if resource_pack.is_none() {
                return Err(validation(
                    "derive-from-resource-pack requires --pack <path>",
                ));
            }
            if out.is_none() && !write_defaults {
                return Err(validation(
                    "derive-from-resource-pack requires --out <path> or --write-defaults",
                ));
            }
            if let Some(out_path) = &out {
                ensure_target_output(out_path)?;
            }
        }

        Ok(Self {
            command,
            check,
            block_json,
            biome_json,
            resource_pack,
            out,
            write,
            write_defaults,
        })
    }
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  cargo run --example palette_tool -- audit [--check] [--pack <path>]");
    eprintln!("  cargo run --example palette_tool -- generate-clean-room --check|--write");
    eprintln!("  cargo run --example palette_tool -- normalize [--check]");
    eprintln!(
        "  cargo run --example palette_tool --features png -- derive-from-resource-pack --pack <path> (--out target/<file>.json|--write-defaults)"
    );
    eprintln!("Optional paths: --block-json <path> --biome-json <path>");
}

fn next_arg(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| validation(format!("missing value for {name}")))
}

fn audit(config: &Config) -> Result<()> {
    let block = read_json(&config.block_json)?;
    let biome = read_json(&config.biome_json)?;
    let normalized_block = normalize_block_document(&block)?;
    let normalized_biome = normalize_biome_document(&biome)?;
    ensure_source_metadata(&block, "block")?;
    ensure_source_metadata(&biome, "biome")?;
    ensure_untainted_sources(&block, "block")?;
    ensure_untainted_sources(&biome, "biome")?;
    ensure_clean_room_source(&block, "block")?;
    ensure_clean_room_source(&biome, "biome")?;
    ensure_fallback_reasons(&block)?;
    ensure_tint_mask_metadata(&block)?;
    ensure_semantic_colors(&block, &biome)?;
    if let Some(pack_path) = config.resource_pack.as_deref() {
        audit_resource_pack_coverage(pack_path, &block)?;
    }
    if config.check {
        ensure_canonical(&config.block_json, &block, &normalized_block)?;
        ensure_canonical(&config.biome_json, &biome, &normalized_biome)?;
    }
    println!("palette audit ok");
    Ok(())
}

fn generate_clean_room(config: &Config) -> Result<()> {
    let block = read_json(&config.block_json)?;
    let biome = read_json(&config.biome_json)?;
    let generated_block = generate_clean_room_block_document(&block)?;
    let generated_biome = generate_clean_room_biome_document(&biome)?;
    if config.check {
        ensure_canonical(&config.block_json, &block, &generated_block)?;
        ensure_canonical(&config.biome_json, &biome, &generated_biome)?;
        println!("clean-room palette JSON is current");
        return Ok(());
    }
    if !config.write {
        return Err(validation("generate-clean-room requires --write"));
    }
    write_json(&config.block_json, &generated_block)?;
    write_json(&config.biome_json, &generated_biome)?;
    println!("clean-room palette JSON generated");
    Ok(())
}

fn normalize(config: &Config) -> Result<()> {
    let block = read_json(&config.block_json)?;
    let biome = read_json(&config.biome_json)?;
    let normalized_block = normalize_block_document(&block)?;
    let normalized_biome = normalize_biome_document(&biome)?;
    if config.check {
        ensure_canonical(&config.block_json, &block, &normalized_block)?;
        ensure_canonical(&config.biome_json, &biome, &normalized_biome)?;
        println!("palette JSON is normalized");
        return Ok(());
    }
    write_json(&config.block_json, &normalized_block)?;
    write_json(&config.biome_json, &normalized_biome)?;
    println!("palette JSON normalized");
    Ok(())
}

#[cfg(feature = "png")]
fn derive_from_resource_pack(config: &Config) -> Result<()> {
    let pack_path = config
        .resource_pack
        .as_deref()
        .ok_or_else(|| validation("derive-from-resource-pack requires --pack <path>"))?;
    let block_source = read_json(&config.block_json)?;
    let biome_source = read_json(&config.biome_json)?;
    let derived_block = normalize_block_document(&derive_block_document_from_resource_pack(
        pack_path,
        &block_source,
    )?)?;
    let derived_biome = normalize_biome_document(&derive_biome_document_from_resource_pack(
        pack_path,
        &biome_source,
    )?)?;
    if let Some(out_path) = config.out.as_deref() {
        write_json(out_path, &derived_block)?;
        println!(
            "derived local block reference palette: {} blocks -> {}",
            derived_block
                .get("blocks")
                .and_then(Value::as_object)
                .map_or(0, Map::len),
            out_path.display()
        );
    }
    if config.write_defaults {
        write_json(&config.block_json, &derived_block)?;
        write_json(&config.biome_json, &derived_biome)?;
        println!(
            "resource-pack derived defaults generated: {} blocks, {} biomes",
            derived_block
                .get("blocks")
                .and_then(Value::as_object)
                .map_or(0, Map::len),
            derived_biome
                .get("biomes")
                .and_then(Value::as_object)
                .map_or(0, Map::len)
        );
    }
    Ok(())
}

#[cfg(feature = "png")]
#[allow(clippy::too_many_lines)]
fn derive_block_document_from_resource_pack(
    pack_path: &Path,
    block_source: &Value,
) -> Result<Value> {
    let terrain_texture = locate_terrain_texture(pack_path)?;
    let blocks_json_path = locate_blocks_json(pack_path)?;
    let terrain_json = read_jsonc(&terrain_texture)?;
    let blocks_json = read_jsonc(&blocks_json_path)?;
    let texture_paths = texture_paths_from_terrain_json(&terrain_json)?;
    let block_textures = block_texture_specs_from_blocks_json(&blocks_json)?;
    let aliases = read_alias_table()?;
    let mut blocks = Map::new();
    let mut matched = 0_usize;
    let mut alias_matched = 0_usize;
    let mut fallback = 0_usize;
    let mut inventory = block_inventory(block_source)?;
    inventory.extend(block_inventory_from_blocks_json(&blocks_json)?);
    for block_name in inventory {
        let mut entry = Map::new();
        let texture_spec = resource_pack_texture_spec(&block_textures, &block_name);
        if let Some(mask) = resource_pack_tint_mask(
            pack_path,
            &texture_paths,
            &block_textures,
            &aliases,
            &texture_spec,
            &block_name,
        )? {
            entry.insert("default".to_string(), color_value(mask.color));
            entry.insert(
                "resource_pack_tint_mask".to_string(),
                color_value(mask.color),
            );
            if let Some(reason) = mask.reason {
                entry.insert(
                    "resource_pack_tint_reason".to_string(),
                    Value::String(reason.to_string()),
                );
            }
            entry.insert(
                "resource_pack_tint_source".to_string(),
                Value::String(mask.source.to_string()),
            );
            blocks.insert(block_name, Value::Object(entry));
            if mask.source == "texture_average" {
                matched += 1;
            } else {
                fallback += 1;
            }
            continue;
        }
        if let Some(color) = average_first_existing_texture(
            pack_path,
            &texture_paths,
            &texture_spec.top,
            special_texture_choice(&block_name),
        )? {
            entry.insert("default".to_string(), color_value(color));
            entry.insert("resource_pack_top".to_string(), color_value(color));
            insert_side_color(pack_path, &texture_paths, &texture_spec, &mut entry)?;
            insert_state_colors(
                pack_path,
                &texture_paths,
                &block_name,
                &texture_spec,
                &mut entry,
            )?;
            insert_special_resource_pack_colors(
                pack_path,
                &texture_paths,
                &block_textures,
                &block_name,
                &mut entry,
            )?;
            blocks.insert(block_name, Value::Object(entry));
            matched += 1;
        } else if let Some(alias) = average_alias_texture(
            pack_path,
            &texture_paths,
            &block_textures,
            &aliases,
            &block_name,
        )? {
            entry.insert("default".to_string(), color_value(alias.color));
            entry.insert("resource_pack_alias".to_string(), color_value(alias.color));
            entry.insert(
                "resource_pack_alias_rule".to_string(),
                Value::String(alias.rule),
            );
            entry.insert(
                "resource_pack_alias_target".to_string(),
                Value::String(alias.target),
            );
            if let Some(side) = alias.side {
                entry.insert("resource_pack_side".to_string(), color_value(side));
                insert_pillar_axis_alias_state_colors(&block_name, alias.color, side, &mut entry);
            }
            insert_special_resource_pack_colors(
                pack_path,
                &texture_paths,
                &block_textures,
                &block_name,
                &mut entry,
            )?;
            blocks.insert(block_name, Value::Object(entry));
            alias_matched += 1;
        } else {
            let reason = unresolved_alias_reason(&aliases, &block_name)
                .unwrap_or_else(|| "resource-pack-missing-texture".to_string());
            let fallback_color = clean_room_block_color(&block_name);
            entry.insert("default".to_string(), color_value(fallback_color));
            entry.insert(
                "clean_room_fallback".to_string(),
                color_value(fallback_color),
            );
            entry.insert("fallback_reason".to_string(), Value::String(reason));
            insert_special_resource_pack_colors(
                pack_path,
                &texture_paths,
                &block_textures,
                &block_name,
                &mut entry,
            )?;
            blocks.insert(block_name, Value::Object(entry));
            fallback += 1;
        }
    }
    println!(
        "resource-pack block colors matched={matched} alias={alias_matched} fallback={fallback}"
    );
    Ok(json!({
        "schema_version": PALETTE_SCHEMA_VERSION,
        "minecraft_bedrock_version": PALETTE_VERSION,
        "sources": block_sources(),
        "blocks": blocks,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TintMaskKind {
    Grass,
    Foliage,
    Water,
}

#[cfg(feature = "png")]
struct TintMask {
    color: Rgba,
    source: &'static str,
    reason: Option<&'static str>,
}

#[cfg(feature = "png")]
fn resource_pack_tint_mask(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    block_textures: &BTreeMap<String, BlockTextureSpec>,
    aliases: &AliasTable,
    texture_spec: &BlockTextureSpec,
    block_name: &str,
) -> Result<Option<TintMask>> {
    let Some(kind) = tint_mask_kind(block_name) else {
        return Ok(None);
    };
    if kind == TintMaskKind::Water {
        return Ok(Some(TintMask {
            color: Rgba::new(112, 136, 190, 210),
            source: "fallback_mask",
            reason: Some("water-biome-mask-fixed"),
        }));
    }
    let texture_names = tint_texture_names(block_textures, aliases, texture_spec, block_name);
    if let Some(color) = average_first_existing_texture(
        pack_path,
        texture_paths,
        &texture_names,
        TextureChoice::First,
    )? {
        return Ok(Some(TintMask {
            color,
            source: "texture_average",
            reason: None,
        }));
    }
    Ok(Some(TintMask {
        color: fallback_tint_mask(kind),
        source: "fallback_mask",
        reason: Some("tinted-block-texture-unresolved"),
    }))
}

fn tint_texture_names(
    block_textures: &BTreeMap<String, BlockTextureSpec>,
    aliases: &AliasTable,
    texture_spec: &BlockTextureSpec,
    block_name: &str,
) -> Vec<String> {
    let mut texture_names = texture_spec.top.clone();
    for target in aliases.targets_for_block(block_name) {
        texture_names.extend(alias_texture_names(block_textures, &target.target));
    }
    dedup_preserve_order(&mut texture_names);
    texture_names
}

fn tint_mask_kind(block_name: &str) -> Option<TintMaskKind> {
    let name = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    if matches!(name, "water" | "flowing_water") {
        return Some(TintMaskKind::Water);
    }
    if clean_room_grass_tint_mask(name) {
        return Some(TintMaskKind::Grass);
    }
    if is_foliage_tint_mask(name) {
        return Some(TintMaskKind::Foliage);
    }
    None
}

fn is_foliage_tint_mask(name: &str) -> bool {
    if name.contains("leaf_litter") {
        return false;
    }
    name.contains("leaves")
        || name.contains("leaf")
        || name.contains("leave")
        || name.contains("foliage")
}

#[cfg(feature = "png")]
fn fallback_tint_mask(kind: TintMaskKind) -> Rgba {
    match kind {
        TintMaskKind::Grass => Rgba::new(180, 180, 180, 255),
        TintMaskKind::Foliage => Rgba::new(180, 180, 180, 238),
        TintMaskKind::Water => Rgba::new(112, 136, 190, 210),
    }
}

#[cfg(feature = "png")]
fn derive_biome_document_from_resource_pack(
    pack_path: &Path,
    biome_source: &Value,
) -> Result<Value> {
    let biomes_client = locate_biomes_client(pack_path)
        .map(|path| read_jsonc(&path))
        .transpose()?;
    let grass_default =
        colormap_average(pack_path, "grass.png")?.unwrap_or_else(|| Rgba::new(121, 192, 90, 255));
    let foliage_default =
        colormap_average(pack_path, "foliage.png")?.unwrap_or_else(|| Rgba::new(72, 160, 48, 255));
    let water_default = biomes_client
        .as_ref()
        .and_then(|value| client_biome_color(value, "default", "water_surface_color"))
        .unwrap_or_else(|| Rgba::new(68, 175, 245, 255));
    let mut biomes = Map::new();
    let inventory = clean_room_biome_inventory(biome_source)?;
    for (name, id) in inventory {
        let clean = clean_room_biome_colors(&name, id);
        let water = biomes_client
            .as_ref()
            .and_then(|value| client_biome_color(value, &name, "water_surface_color"))
            .unwrap_or(clean.water);
        let mut biome = Map::new();
        biome.insert("id".to_string(), Value::from(id));
        biome.insert("rgb".to_string(), color_value(clean.rgb));
        biome.insert(
            "grass".to_string(),
            color_value(vanilla_grass_tint(&name, clean.grass)),
        );
        biome.insert(
            "leaves".to_string(),
            color_value(vanilla_foliage_tint(&name, clean.leaves)),
        );
        biome.insert("water".to_string(), color_value(water));
        biomes.insert(name, Value::Object(biome));
    }
    Ok(json!({
        "schema_version": PALETTE_SCHEMA_VERSION,
        "minecraft_bedrock_version": PALETTE_VERSION,
        "sources": biome_sources(),
        "defaults": {
            "rgb": color_value(Rgba::new(106, 145, 72, 255)),
            "grass": color_value(grass_default),
            "leaves": color_value(foliage_default),
            "water": color_value(water_default),
        },
        "biomes": biomes,
    }))
}

#[cfg(not(feature = "png"))]
fn derive_from_resource_pack(_config: &Config) -> Result<()> {
    Err(validation(
        "derive-from-resource-pack requires `cargo run --example palette_tool --features png -- ...`",
    ))
}

fn read_json(path: &Path) -> Result<Value> {
    let content = fs::read_to_string(path).map_err(|error| {
        BedrockRenderError::io(format!("failed to read {}", path.display()), error)
    })?;
    serde_json::from_str(&content)
        .map_err(|error| validation(format!("invalid JSON in {}: {error}", path.display())))
}

fn read_jsonc(path: &Path) -> Result<Value> {
    let content = fs::read_to_string(path).map_err(|error| {
        BedrockRenderError::io(format!("failed to read {}", path.display()), error)
    })?;
    json5::from_str(&content)
        .map_err(|error| validation(format!("invalid JSON5 in {}: {error}", path.display())))
}

#[cfg(feature = "png")]
fn colormap_average(pack_path: &Path, file_name: &str) -> Result<Option<Rgba>> {
    let path = pack_path.join("textures").join("colormap").join(file_name);
    if path.exists() {
        average_texture(&path).map(Some)
    } else {
        Ok(None)
    }
}

#[cfg(feature = "png")]
fn client_biome_color(value: &Value, name: &str, key: &str) -> Option<Rgba> {
    let biomes = value.get("biomes")?.as_object()?;
    let short_name = name.strip_prefix("minecraft:").unwrap_or(name);
    biomes
        .get(&format!("minecraft:{short_name}"))
        .or_else(|| biomes.get(short_name))
        .or_else(|| biomes.get("default"))?
        .get(key)
        .and_then(Value::as_str)
        .and_then(parse_hex_rgba)
}

#[cfg(feature = "png")]
fn parse_hex_rgba(value: &str) -> Option<Rgba> {
    let value = value
        .trim()
        .trim_start_matches('#')
        .trim_start_matches("0x");
    let color = u32::from_str_radix(value, 16).ok()?;
    match value.len() {
        6 => Some(Rgba::new(
            u8_from_u32((color >> 16) & 0xff),
            u8_from_u32((color >> 8) & 0xff),
            u8_from_u32(color & 0xff),
            255,
        )),
        8 => Some(Rgba::new(
            u8_from_u32((color >> 24) & 0xff),
            u8_from_u32((color >> 16) & 0xff),
            u8_from_u32((color >> 8) & 0xff),
            u8_from_u32(color & 0xff),
        )),
        _ => None,
    }
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, canonical_json(value)).map_err(|error| {
        BedrockRenderError::io(format!("failed to write {}", path.display()), error)
    })
}

fn ensure_canonical(path: &Path, original: &Value, normalized: &Value) -> Result<()> {
    if canonical_json(original) != canonical_json(normalized) {
        return Err(validation(format!(
            "{} is not normalized; run `cargo run --example palette_tool -- normalize`",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_source_metadata(value: &Value, label: &str) -> Result<()> {
    let Some(sources) = value.get("sources").and_then(Value::as_array) else {
        return Err(validation(format!(
            "{label} palette is missing sources metadata"
        )));
    };
    if sources.is_empty() {
        return Err(validation(format!(
            "{label} palette sources metadata is empty"
        )));
    }
    for source in sources {
        let Some(source) = source.as_object() else {
            return Err(validation(format!(
                "{label} palette source entry must be an object"
            )));
        };
        for key in ["id", "kind", "license", "retrieved_at", "usage"] {
            if !source.get(key).is_some_and(Value::is_string) {
                return Err(validation(format!(
                    "{label} palette source entry is missing string field `{key}`"
                )));
            }
        }
    }
    Ok(())
}

fn ensure_untainted_sources(value: &Value, label: &str) -> Result<()> {
    let sources = value
        .get("sources")
        .and_then(Value::as_array)
        .ok_or_else(|| validation(format!("{label} palette is missing sources metadata")))?;
    for source in sources {
        let source_text = serde_json::to_string(source)
            .map_err(|error| validation(format!("failed to inspect source metadata: {error}")))?
            .to_ascii_lowercase();
        for marker in [
            concat!("legacy", "-current"),
            concat!("bedrock", "-level"),
            "agpl",
            concat!("wiki", "-icon-derived"),
            concat!("wiki", "-color-values"),
        ] {
            if source_text.contains(marker) {
                return Err(validation(format!(
                    "{label} palette source metadata contains tainted marker `{marker}`"
                )));
            }
        }
    }
    Ok(())
}

fn ensure_clean_room_source(value: &Value, label: &str) -> Result<()> {
    let sources = value
        .get("sources")
        .and_then(Value::as_array)
        .ok_or_else(|| validation(format!("{label} palette is missing sources metadata")))?;
    let has_clean_room = sources.iter().any(|source| {
        source
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id == "bedrock-render-clean-room-v1")
    });
    if !has_clean_room {
        return Err(validation(format!(
            "{label} palette must include bedrock-render-clean-room-v1 source metadata"
        )));
    }
    Ok(())
}

fn ensure_fallback_reasons(value: &Value) -> Result<()> {
    let blocks = value
        .get("blocks")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("block palette JSON must contain a blocks object"))?;
    for (name, entry) in blocks {
        let Some(map) = entry.as_object() else {
            continue;
        };
        if map.contains_key("clean_room_fallback")
            && map
                .get("fallback_reason")
                .and_then(Value::as_str)
                .is_none_or(|reason| reason.trim().is_empty())
        {
            return Err(validation(format!(
                "block `{name}` uses clean_room_fallback without fallback_reason"
            )));
        }
    }
    Ok(())
}

fn audit_resource_pack_coverage(pack_path: &Path, block_source: &Value) -> Result<()> {
    let terrain_texture = locate_terrain_texture(pack_path)?;
    let blocks_json_path = locate_blocks_json(pack_path)?;
    let terrain_json = read_jsonc(&terrain_texture)?;
    let blocks_json = read_jsonc(&blocks_json_path)?;
    let texture_paths = texture_paths_from_terrain_json(&terrain_json)?;
    let block_textures = block_texture_specs_from_blocks_json(&blocks_json)?;
    let aliases = read_alias_table()?;
    let mut inventory = block_inventory(block_source)?;
    inventory.extend(block_inventory_from_blocks_json(&blocks_json)?);

    let mut direct = 0_usize;
    let mut alias = 0_usize;
    let mut fallback = 0_usize;
    let mut unresolved_aliases = Vec::new();
    for block_name in inventory {
        let texture_spec = resource_pack_texture_spec(&block_textures, &block_name);
        if let Some(kind) = tint_mask_kind(&block_name) {
            let texture_names =
                tint_texture_names(&block_textures, &aliases, &texture_spec, &block_name);
            if kind != TintMaskKind::Water
                && first_existing_texture_path(
                    pack_path,
                    &texture_paths,
                    &texture_names,
                    TextureChoice::First,
                )
                .is_some()
            {
                direct += 1;
            } else {
                fallback += 1;
            }
            continue;
        }
        if first_existing_texture_path(
            pack_path,
            &texture_paths,
            &texture_spec.top,
            special_texture_choice(&block_name),
        )
        .is_some()
        {
            direct += 1;
            continue;
        }
        let mut alias_resolved = false;
        let targets = aliases.targets_for_block(&block_name);
        for target in &targets {
            let texture_names = alias_texture_names(&block_textures, &target.target);
            if first_existing_texture_path(
                pack_path,
                &texture_paths,
                &texture_names,
                TextureChoice::First,
            )
            .is_some()
            {
                alias_resolved = true;
                break;
            }
        }
        if alias_resolved {
            alias += 1;
        } else {
            if !targets.is_empty() {
                unresolved_aliases.push(format!(
                    "{} ({})",
                    block_name,
                    targets
                        .iter()
                        .take(3)
                        .map(|target| format!("{}={}", target.rule, target.target))
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            fallback += 1;
        }
    }
    println!("resource-pack audit coverage direct={direct} alias={alias} fallback={fallback}");
    if !unresolved_aliases.is_empty() {
        println!(
            "resource-pack alias targets did not resolve for {} blocks, first: {}",
            unresolved_aliases.len(),
            unresolved_aliases
                .iter()
                .take(12)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

fn ensure_tint_mask_metadata(value: &Value) -> Result<()> {
    let blocks = value
        .get("blocks")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("block palette JSON must contain a blocks object"))?;
    for (name, entry) in blocks {
        let Some(map) = entry.as_object() else {
            continue;
        };
        if !map.contains_key("resource_pack_tint_mask") {
            continue;
        }
        let Some(source) = map.get("resource_pack_tint_source").and_then(Value::as_str) else {
            return Err(validation(format!(
                "block `{name}` uses resource_pack_tint_mask without resource_pack_tint_source"
            )));
        };
        match source {
            "texture_average" => {}
            "fallback_mask" => {
                if map
                    .get("resource_pack_tint_reason")
                    .and_then(Value::as_str)
                    .is_none_or(|reason| reason.trim().is_empty())
                {
                    return Err(validation(format!(
                        "block `{name}` uses fallback tint mask without resource_pack_tint_reason"
                    )));
                }
            }
            _ => {
                return Err(validation(format!(
                    "block `{name}` has unsupported resource_pack_tint_source `{source}`"
                )));
            }
        }
    }
    Ok(())
}

fn ensure_semantic_colors(block: &Value, biome: &Value) -> Result<()> {
    let bamboo = required_block_color(block, "minecraft:bamboo")?;
    ensure_green_dominant(bamboo, "minecraft:bamboo")?;
    let bamboo_planks = required_block_color(block, "minecraft:bamboo_planks")?;
    ensure_yellow_material(bamboo_planks, "minecraft:bamboo_planks")?;
    let path = required_block_color(block, "minecraft:grass_path")?;
    ensure_yellow_brown(path, "minecraft:grass_path")?;
    let leaf_litter = required_block_color(block, "minecraft:leaf_litter")?;
    ensure_yellow_brown(leaf_litter, "minecraft:leaf_litter")?;
    let grass_mask = required_block_color(block, "minecraft:grass_block")?;
    ensure_neutral_mask(grass_mask, "minecraft:grass_block")?;
    let leaves_mask = required_block_color(block, "minecraft:oak_leaves")?;
    ensure_neutral_mask(leaves_mask, "minecraft:oak_leaves")?;
    let water_mask = required_block_color(block, "minecraft:water")?;
    ensure_water_mask(water_mask, "minecraft:water")?;
    let red_sand = required_block_color(block, "minecraft:red_sand")?;
    ensure_red_sand(red_sand, "minecraft:red_sand")?;
    ensure_sand_state_colors(block)?;

    let plains = final_tinted(grass_mask, required_biome_color(biome, "plains", "grass")?);
    let desert = final_tinted(grass_mask, required_biome_color(biome, "desert", "grass")?);
    let jungle = final_tinted(grass_mask, required_biome_color(biome, "jungle", "grass")?);
    let swamp = final_tinted(
        grass_mask,
        required_biome_color(biome, "swampland", "grass")?,
    );
    let cherry = final_tinted(
        grass_mask,
        required_biome_color(biome, "cherry_grove", "grass")?,
    );
    let pale = final_tinted(
        grass_mask,
        required_biome_color(biome, "pale_garden", "grass")?,
    );
    let ocean = final_tinted(water_mask, required_biome_color(biome, "ocean", "water")?);
    let deep_ocean = final_tinted(
        water_mask,
        required_biome_color(biome, "deep_ocean", "water")?,
    );
    let river = final_tinted(water_mask, required_biome_color(biome, "river", "water")?);
    if color_distance(bamboo, plains) < 45 || color_distance(bamboo, bamboo_planks) < 120 {
        return Err(validation(
            "bamboo must stay visually distinct from grass tint and bamboo material",
        ));
    }
    for (label, color) in [
        ("plains grass", plains),
        ("desert grass", desert),
        ("jungle grass", jungle),
        ("swamp grass", swamp),
        ("cherry grass", cherry),
        ("pale garden grass", pale),
    ] {
        let brightness = luminance(color);
        if !(70..=140).contains(&brightness) {
            return Err(validation(format!(
                "{label} brightness is outside the allowed biome tint range"
            )));
        }
    }
    if !(desert.red > desert.green && desert.green > desert.blue) {
        return Err(validation("desert grass tint should read yellow-green"));
    }
    ensure_green_dominant(jungle, "jungle grass after tint")?;
    if color_distance(plains, desert) < 40
        || color_distance(jungle, swamp) < 35
        || color_distance(cherry, pale) < 25
    {
        return Err(validation(
            "key biome grass tints are not visually separated enough",
        ));
    }
    for (label, color) in [
        ("ocean water", ocean),
        ("deep ocean water", deep_ocean),
        ("river water", river),
    ] {
        if luminance(color) > 80 {
            return Err(validation(format!(
                "{label} should stay deep and saturated"
            )));
        }
        if !(color.blue > color.green && color.green >= color.red) {
            return Err(validation(format!("{label} should read as blue water")));
        }
    }
    Ok(())
}

fn required_block_color(value: &Value, name: &str) -> Result<Rgba> {
    let blocks = value
        .get("blocks")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("block palette JSON must contain a blocks object"))?;
    let entry = blocks
        .get(name)
        .ok_or_else(|| validation(format!("block palette is missing `{name}`")))?;
    let color = entry
        .get("default")
        .or_else(|| entry.get("resource_pack_top"))
        .or_else(|| entry.get("resource_pack_alias"))
        .or_else(|| entry.get("resource_pack_tint_mask"))
        .or_else(|| entry.get("clean_room"))
        .or_else(|| entry.get("clean_room_fallback"))
        .or_else(|| entry.as_object().and_then(|map| map.values().next()))
        .ok_or_else(|| validation(format!("block `{name}` is missing a color")))?;
    parse_rgba_value(color, name)
}

fn required_biome_color(value: &Value, name: &str, key: &str) -> Result<Rgba> {
    let biomes = value
        .get("biomes")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("biome palette JSON must contain a biomes object"))?;
    let biome = biomes
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| validation(format!("biome palette is missing `{name}`")))?;
    let color = biome
        .get(key)
        .ok_or_else(|| validation(format!("biome `{name}` is missing `{key}`")))?;
    parse_rgba_value(color, &format!("{name}.{key}"))
}

fn parse_rgba_value(value: &Value, label: &str) -> Result<Rgba> {
    let channels = value
        .as_array()
        .ok_or_else(|| validation(format!("{label} color must be an array")))?;
    if channels.len() != 4 {
        return Err(validation(format!("{label} color must have 4 channels")));
    }
    let channel = |index| {
        channels
            .get(index)
            .and_then(Value::as_u64)
            .and_then(|value| u8::try_from(value).ok())
            .ok_or_else(|| validation(format!("{label} channel {index} is outside 0..=255")))
    };
    Ok(Rgba::new(
        channel(0)?,
        channel(1)?,
        channel(2)?,
        channel(3)?,
    ))
}

fn ensure_green_dominant(color: Rgba, label: &str) -> Result<()> {
    if color.green <= color.red.saturating_add(20) || color.green <= color.blue.saturating_add(30) {
        return Err(validation(format!("{label} should be green-dominant")));
    }
    Ok(())
}

fn ensure_yellow_material(color: Rgba, label: &str) -> Result<()> {
    if !(color.red >= color.green.saturating_sub(28) && color.green > color.blue.saturating_add(45))
    {
        return Err(validation(format!(
            "{label} should read as yellow bamboo material"
        )));
    }
    Ok(())
}

fn ensure_yellow_brown(color: Rgba, label: &str) -> Result<()> {
    if !(color.red > color.green && color.green > color.blue && color.red > color.blue + 50) {
        return Err(validation(format!("{label} should be yellow-brown")));
    }
    Ok(())
}

fn ensure_neutral_mask(color: Rgba, label: &str) -> Result<()> {
    let max_channel = color.red.max(color.green).max(color.blue);
    let min_channel = color.red.min(color.green).min(color.blue);
    if min_channel < 120 || max_channel > 220 || max_channel.saturating_sub(min_channel) > 24 {
        return Err(validation(format!("{label} should be a neutral tint mask")));
    }
    Ok(())
}

fn ensure_water_mask(color: Rgba, label: &str) -> Result<()> {
    if !(color.blue > color.green
        && color.green > color.red
        && color.blue.saturating_sub(color.red) >= 36
        && (190..=225).contains(&color.alpha))
    {
        return Err(validation(format!(
            "{label} should be a blue water tint mask"
        )));
    }
    Ok(())
}

fn ensure_red_sand(color: Rgba, label: &str) -> Result<()> {
    if !(color.red > color.green
        && color.green > color.blue
        && color.red.saturating_sub(color.blue) >= 100)
    {
        return Err(validation(format!("{label} should read as red sand")));
    }
    Ok(())
}

fn ensure_sand_state_colors(value: &Value) -> Result<()> {
    let blocks = value
        .get("blocks")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("block palette JSON must contain a blocks object"))?;
    let sand = blocks
        .get("minecraft:sand")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("block palette is missing `minecraft:sand`"))?;
    let state_colors = sand
        .get("state_colors")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("minecraft:sand is missing state_colors"))?;
    for key in ["sand_type", "sand_color", "variant", "data", "damage"] {
        let rules = state_colors
            .get(key)
            .and_then(Value::as_array)
            .ok_or_else(|| validation(format!("minecraft:sand.state_colors.{key} is missing")))?;
        let has_numeric_red = rules.iter().any(|rule| {
            rule.as_object().is_some_and(|rule| {
                rule.get("min").and_then(Value::as_i64) == Some(1)
                    && rule.get("max").and_then(Value::as_i64) == Some(1)
                    && rule.get("color").is_some()
            })
        });
        let has_named_red = rules.iter().any(|rule| {
            rule.as_object()
                .and_then(|rule| rule.get("values"))
                .and_then(Value::as_array)
                .is_some_and(|values| {
                    values.iter().any(|value| value.as_str() == Some("red"))
                        && values
                            .iter()
                            .any(|value| value.as_str() == Some("red_sand"))
                })
        });
        if !has_numeric_red || !has_named_red {
            return Err(validation(format!(
                "minecraft:sand.state_colors.{key} must include red sand numeric and named rules"
            )));
        }
    }
    Ok(())
}

fn final_tinted(mask: Rgba, tint: Rgba) -> Rgba {
    Rgba::new(
        multiply_channel(mask.red, tint.red),
        multiply_channel(mask.green, tint.green),
        multiply_channel(mask.blue, tint.blue),
        255,
    )
}

fn multiply_channel(mask: u8, tint: u8) -> u8 {
    let value = (u16::from(mask) * u16::from(tint)) / 255;
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn luminance(color: Rgba) -> u16 {
    (u16::from(color.red) * 30 + u16::from(color.green) * 59 + u16::from(color.blue) * 11) / 100
}

fn color_distance(left: Rgba, right: Rgba) -> u16 {
    u16::from(left.red.abs_diff(right.red))
        + u16::from(left.green.abs_diff(right.green))
        + u16::from(left.blue.abs_diff(right.blue))
}

fn ensure_target_output(path: &Path) -> Result<()> {
    if path
        .components()
        .any(|component| component.as_os_str().to_string_lossy() == "target")
    {
        return Ok(());
    }
    Err(validation(
        "local resource-pack derived palettes must be written under target/",
    ))
}

fn generate_clean_room_block_document(value: &Value) -> Result<Value> {
    let mut blocks = Map::new();
    for block_name in block_inventory(value)? {
        let mut entry = Map::new();
        entry.insert(
            "clean_room".to_string(),
            color_value(clean_room_block_color(&block_name)),
        );
        blocks.insert(block_name, Value::Object(entry));
    }
    Ok(json!({
        "schema_version": PALETTE_SCHEMA_VERSION,
        "minecraft_bedrock_version": minecraft_version(value),
        "sources": block_sources(),
        "blocks": blocks,
    }))
}

fn generate_clean_room_biome_document(value: &Value) -> Result<Value> {
    let mut biomes = Map::new();
    let inventory = clean_room_biome_inventory(value)?;
    let mut ids = BTreeSet::new();
    for (name, id) in inventory {
        if !ids.insert(id) {
            return Err(validation(format!("duplicate clean-room biome id {id}")));
        }
        let colors = clean_room_biome_colors(&name, id);
        let mut biome = Map::new();
        biome.insert("id".to_string(), Value::from(id));
        biome.insert("rgb".to_string(), color_value(colors.rgb));
        biome.insert("grass".to_string(), color_value(colors.grass));
        biome.insert("leaves".to_string(), color_value(colors.leaves));
        biome.insert("water".to_string(), color_value(colors.water));
        biomes.insert(name, Value::Object(biome));
    }
    Ok(json!({
        "schema_version": PALETTE_SCHEMA_VERSION,
        "minecraft_bedrock_version": minecraft_version(value),
        "sources": biome_sources(),
        "defaults": {
            "rgb": color_value(Rgba::new(106, 145, 72, 255)),
            "grass": color_value(Rgba::new(98, 151, 64, 255)),
            "leaves": color_value(Rgba::new(62, 124, 50, 255)),
            "water": color_value(Rgba::new(28, 76, 158, 255)),
        },
        "biomes": biomes,
    }))
}

fn block_inventory(value: &Value) -> Result<BTreeSet<String>> {
    let blocks = value.get("blocks").unwrap_or(value);
    let Some(blocks) = blocks.as_object() else {
        return Err(validation(
            "block palette JSON must contain an object `blocks` map",
        ));
    };
    let mut names = blocks
        .keys()
        .filter(|name| !name.trim().is_empty())
        .map(|name| normalize_block_name(name))
        .collect::<BTreeSet<_>>();
    for name in [
        "minecraft:air",
        "minecraft:cave_air",
        "minecraft:void_air",
        "minecraft:structure_void",
        "minecraft:light",
        "minecraft:light_block",
        "minecraft:terracotta",
    ] {
        names.insert(name.to_string());
    }
    Ok(names)
}

#[allow(dead_code)]
fn block_texture_names(value: &Value, block_name: &str) -> Vec<String> {
    let Some(blocks) = value.get("blocks").unwrap_or(value).as_object() else {
        return Vec::new();
    };
    let mut names = Vec::new();
    if let Some(entry) = blocks.get(block_name).and_then(Value::as_object) {
        names.extend(entry.keys().cloned());
    }
    let short_name = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    names.push(short_name.to_string());
    names.sort();
    names.dedup();
    names
}

#[derive(Clone, Copy)]
struct BiomeColors {
    rgb: Rgba,
    grass: Rgba,
    leaves: Rgba,
    water: Rgba,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rgba {
    red: u8,
    green: u8,
    blue: u8,
    alpha: u8,
}

impl Rgba {
    const fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }
}

fn color_value(color: Rgba) -> Value {
    Value::Array(vec![
        Value::from(color.red),
        Value::from(color.green),
        Value::from(color.blue),
        Value::from(color.alpha),
    ])
}

fn clean_room_biome_inventory(value: &Value) -> Result<Vec<(String, u32)>> {
    let mut by_id = BTreeMap::new();
    for (name, id) in CLEAN_ROOM_BIOMES {
        by_id.insert(*id, (*name).to_string());
    }
    if let Some(biomes) = value.get("biomes").and_then(Value::as_object) {
        for (name, entry) in biomes {
            if name == "default" {
                continue;
            }
            let Some(id) = entry.get("id").and_then(Value::as_u64) else {
                continue;
            };
            let id = u32::try_from(id)
                .map_err(|_| validation(format!("biome `{name}` id is outside u32 range")))?;
            by_id.entry(id).or_insert_with(|| name.clone());
        }
    }
    Ok(by_id
        .into_iter()
        .map(|(id, name)| (name, id))
        .collect::<Vec<_>>())
}

const CLEAN_ROOM_BIOMES: &[(&str, u32)] = &[
    ("ocean", 0),
    ("plains", 1),
    ("desert", 2),
    ("extreme_hills", 3),
    ("forest", 4),
    ("taiga", 5),
    ("swampland", 6),
    ("river", 7),
    ("hell", 8),
    ("the_end", 9),
    ("legacy_frozen_ocean", 10),
    ("frozen_river", 11),
    ("ice_plains", 12),
    ("ice_mountains", 13),
    ("mushroom_island", 14),
    ("mushroom_island_shore", 15),
    ("beach", 16),
    ("desert_hills", 17),
    ("forest_hills", 18),
    ("taiga_hills", 19),
    ("extreme_hills_edge", 20),
    ("jungle", 21),
    ("jungle_hills", 22),
    ("jungle_edge", 23),
    ("deep_ocean", 24),
    ("stone_beach", 25),
    ("cold_beach", 26),
    ("birch_forest", 27),
    ("birch_forest_hills", 28),
    ("roofed_forest", 29),
    ("cold_taiga", 30),
    ("cold_taiga_hills", 31),
    ("mega_taiga", 32),
    ("mega_taiga_hills", 33),
    ("extreme_hills_plus_trees", 34),
    ("savanna", 35),
    ("savanna_plateau", 36),
    ("mesa", 37),
    ("mesa_plateau_stone", 38),
    ("mesa_plateau", 39),
    ("warm_ocean", 40),
    ("deep_warm_ocean", 41),
    ("lukewarm_ocean", 42),
    ("deep_lukewarm_ocean", 43),
    ("cold_ocean", 44),
    ("deep_cold_ocean", 45),
    ("frozen_ocean", 46),
    ("deep_frozen_ocean", 47),
    ("bamboo_jungle", 48),
    ("bamboo_jungle_hills", 49),
    ("the_void", 127),
    ("sunflower_plains", 129),
    ("desert_mutated", 130),
    ("extreme_hills_mutated", 131),
    ("flower_forest", 132),
    ("taiga_mutated", 133),
    ("swampland_mutated", 134),
    ("ice_plains_spikes", 140),
    ("jungle_mutated", 149),
    ("jungle_edge_mutated", 151),
    ("birch_forest_mutated", 155),
    ("birch_forest_hills_mutated", 156),
    ("roofed_forest_mutated", 157),
    ("cold_taiga_mutated", 158),
    ("redwood_taiga_mutated", 160),
    ("redwood_taiga_hills_mutated", 161),
    ("extreme_hills_plus_trees_mutated", 162),
    ("savanna_mutated", 163),
    ("savanna_plateau_mutated", 164),
    ("mesa_bryce", 165),
    ("mesa_plateau_stone_mutated", 166),
    ("mesa_plateau_mutated", 167),
    ("soulsand_valley", 178),
    ("crimson_forest", 179),
    ("warped_forest", 180),
    ("basalt_deltas", 181),
    ("lofty_peaks", 182),
    ("snow_capped_peaks", 183),
    ("snowy_slopes", 184),
    ("mountain_grove", 185),
    ("mountain_meadow", 186),
    ("lush_caves", 187),
    ("dripstone_caves", 188),
    ("stony_peaks", 189),
    ("deep_dark", 190),
    ("mangrove_swamp", 191),
    ("cherry_grove", 192),
    ("pale_garden", 193),
];

fn normalize_block_name(name: &str) -> String {
    if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    }
}

// The generator keeps the full rule table in one place so palette audits can
// review the source policy without chasing cross-module state.
#[allow(clippy::too_many_lines)]
fn clean_room_biome_colors(name: &str, id: u32) -> BiomeColors {
    let short_name = name.strip_prefix("minecraft:").unwrap_or(name);
    let mut colors = if short_name.contains("ocean") {
        BiomeColors {
            rgb: Rgba::new(24, 52, 102, 255),
            grass: Rgba::new(96, 146, 70, 255),
            leaves: Rgba::new(58, 116, 52, 255),
            water: Rgba::new(24, 64, 142, 255),
        }
    } else if short_name.contains("river") {
        BiomeColors {
            rgb: Rgba::new(42, 82, 138, 255),
            grass: Rgba::new(101, 151, 72, 255),
            leaves: Rgba::new(62, 122, 55, 255),
            water: Rgba::new(28, 76, 162, 255),
        }
    } else if short_name.contains("desert")
        || short_name.contains("beach")
        || short_name.contains("savanna")
    {
        BiomeColors {
            rgb: Rgba::new(172, 148, 82, 255),
            grass: Rgba::new(172, 152, 70, 255),
            leaves: Rgba::new(118, 126, 62, 255),
            water: Rgba::new(30, 76, 150, 255),
        }
    } else if short_name.contains("mesa") {
        BiomeColors {
            rgb: Rgba::new(148, 82, 52, 255),
            grass: Rgba::new(150, 118, 60, 255),
            leaves: Rgba::new(104, 100, 56, 255),
            water: Rgba::new(28, 70, 140, 255),
        }
    } else if short_name.contains("frozen")
        || short_name.contains("ice")
        || short_name.contains("snow")
        || short_name.contains("cold")
    {
        BiomeColors {
            rgb: Rgba::new(150, 166, 176, 255),
            grass: Rgba::new(112, 136, 90, 255),
            leaves: Rgba::new(72, 112, 72, 255),
            water: Rgba::new(34, 80, 154, 255),
        }
    } else if short_name.contains("jungle") || short_name.contains("bamboo") {
        BiomeColors {
            rgb: Rgba::new(34, 98, 42, 255),
            grass: Rgba::new(54, 156, 42, 255),
            leaves: Rgba::new(34, 120, 36, 255),
            water: Rgba::new(28, 74, 144, 255),
        }
    } else if short_name.contains("taiga") || short_name.contains("redwood") {
        BiomeColors {
            rgb: Rgba::new(62, 94, 78, 255),
            grass: Rgba::new(92, 128, 78, 255),
            leaves: Rgba::new(54, 98, 58, 255),
            water: Rgba::new(26, 68, 142, 255),
        }
    } else if short_name.contains("swamp") || short_name.contains("mangrove") {
        BiomeColors {
            rgb: Rgba::new(56, 78, 52, 255),
            grass: Rgba::new(86, 116, 64, 255),
            leaves: Rgba::new(52, 80, 44, 255),
            water: Rgba::new(36, 58, 78, 255),
        }
    } else if short_name.contains("mushroom") {
        BiomeColors {
            rgb: Rgba::new(106, 82, 110, 255),
            grass: Rgba::new(98, 124, 70, 255),
            leaves: Rgba::new(62, 100, 52, 255),
            water: Rgba::new(26, 64, 132, 255),
        }
    } else if short_name.contains("hell")
        || short_name.contains("soul")
        || short_name.contains("crimson")
        || short_name.contains("warped")
        || short_name.contains("basalt")
    {
        BiomeColors {
            rgb: Rgba::new(66, 30, 40, 255),
            grass: Rgba::new(82, 74, 58, 255),
            leaves: Rgba::new(62, 56, 50, 255),
            water: Rgba::new(30, 48, 82, 255),
        }
    } else if short_name.contains("end") || short_name.contains("void") {
        BiomeColors {
            rgb: Rgba::new(70, 66, 86, 255),
            grass: Rgba::new(100, 112, 70, 255),
            leaves: Rgba::new(66, 90, 54, 255),
            water: Rgba::new(26, 60, 122, 255),
        }
    } else if short_name.contains("cave") || short_name.contains("deep_dark") {
        BiomeColors {
            rgb: Rgba::new(44, 50, 48, 255),
            grass: Rgba::new(76, 108, 62, 255),
            leaves: Rgba::new(44, 82, 44, 255),
            water: Rgba::new(22, 54, 112, 255),
        }
    } else if short_name.contains("peak")
        || short_name.contains("slope")
        || (short_name.contains("grove")
            && !short_name.contains("cherry")
            && !short_name.contains("pale"))
        || short_name.contains("hill")
        || short_name.contains("mountain")
        || short_name.contains("extreme")
    {
        BiomeColors {
            rgb: Rgba::new(96, 106, 96, 255),
            grass: Rgba::new(86, 116, 70, 255),
            leaves: Rgba::new(56, 92, 52, 255),
            water: Rgba::new(26, 62, 128, 255),
        }
    } else if short_name.contains("cherry") {
        BiomeColors {
            rgb: Rgba::new(156, 112, 130, 255),
            grass: Rgba::new(122, 150, 84, 255),
            leaves: Rgba::new(178, 104, 126, 255),
            water: Rgba::new(28, 70, 148, 255),
        }
    } else if short_name.contains("pale") {
        BiomeColors {
            rgb: Rgba::new(112, 114, 102, 255),
            grass: Rgba::new(108, 112, 88, 255),
            leaves: Rgba::new(112, 110, 92, 255),
            water: Rgba::new(28, 62, 122, 255),
        }
    } else if short_name.contains("forest") {
        BiomeColors {
            rgb: Rgba::new(56, 104, 54, 255),
            grass: Rgba::new(72, 138, 58, 255),
            leaves: Rgba::new(42, 106, 40, 255),
            water: Rgba::new(26, 68, 148, 255),
        }
    } else {
        BiomeColors {
            rgb: Rgba::new(106, 145, 72, 255),
            grass: Rgba::new(98, 151, 64, 255),
            leaves: Rgba::new(62, 124, 50, 255),
            water: Rgba::new(28, 76, 158, 255),
        }
    };
    let id_variant = i16::try_from(id % 5).unwrap_or(0);
    colors.rgb = vary_color(colors.rgb, short_name, 7 + id_variant);
    colors.grass = vary_color(colors.grass, short_name, 5);
    colors.leaves = vary_color(colors.leaves, short_name, 5);
    colors.water = vary_color(colors.water, short_name, 4);
    colors
}

#[cfg_attr(not(feature = "png"), allow(dead_code))]
fn vanilla_grass_tint(name: &str, fallback: Rgba) -> Rgba {
    let short_name = name.strip_prefix("minecraft:").unwrap_or(name);
    let base = if short_name.contains("beach") {
        Rgba::new(121, 192, 90, 255)
    } else if short_name.contains("desert") || short_name.contains("savanna") {
        Rgba::new(188, 174, 82, 255)
    } else if short_name.contains("mesa") {
        Rgba::new(176, 142, 74, 255)
    } else if short_name.contains("jungle") || short_name.contains("bamboo") {
        Rgba::new(89, 201, 60, 255)
    } else if short_name.contains("swamp") || short_name.contains("mangrove") {
        Rgba::new(106, 142, 76, 255)
    } else if short_name.contains("taiga") || short_name.contains("redwood") {
        Rgba::new(134, 184, 126, 255)
    } else if short_name.contains("frozen")
        || short_name.contains("ice")
        || short_name.contains("snow")
        || short_name.contains("cold")
    {
        Rgba::new(140, 178, 132, 255)
    } else if short_name.contains("forest") {
        Rgba::new(104, 176, 76, 255)
    } else if short_name.contains("cherry") {
        Rgba::new(148, 184, 104, 255)
    } else if short_name.contains("pale") {
        Rgba::new(152, 162, 132, 255)
    } else if short_name.contains("hell")
        || short_name.contains("soul")
        || short_name.contains("crimson")
        || short_name.contains("warped")
        || short_name.contains("basalt")
    {
        fallback
    } else {
        Rgba::new(121, 192, 90, 255)
    };
    vary_color(base, short_name, 3)
}

#[cfg_attr(not(feature = "png"), allow(dead_code))]
fn vanilla_foliage_tint(name: &str, fallback: Rgba) -> Rgba {
    let short_name = name.strip_prefix("minecraft:").unwrap_or(name);
    let base = if short_name.contains("jungle") || short_name.contains("bamboo") {
        Rgba::new(64, 170, 58, 255)
    } else if short_name.contains("swamp") || short_name.contains("mangrove") {
        Rgba::new(76, 118, 62, 255)
    } else if short_name.contains("taiga") || short_name.contains("redwood") {
        Rgba::new(86, 132, 82, 255)
    } else if short_name.contains("cherry") {
        Rgba::new(210, 126, 150, 255)
    } else if short_name.contains("pale") {
        Rgba::new(148, 148, 126, 255)
    } else if short_name.contains("desert")
        || short_name.contains("savanna")
        || short_name.contains("mesa")
    {
        Rgba::new(132, 142, 70, 255)
    } else if short_name.contains("hell")
        || short_name.contains("soul")
        || short_name.contains("crimson")
        || short_name.contains("warped")
        || short_name.contains("basalt")
    {
        fallback
    } else {
        Rgba::new(85, 164, 68, 255)
    };
    vary_color(base, short_name, 3)
}

// The clean-room material classifier is intentionally explicit: each branch is
// a project-authored category rule, not copied source data.
#[allow(clippy::too_many_lines)]
fn clean_room_block_color(name: &str) -> Rgba {
    let short_name = name.strip_prefix("minecraft:").unwrap_or(name);
    if matches!(
        short_name,
        "air" | "cave_air" | "void_air" | "structure_void" | "light" | "light_block"
    ) {
        return Rgba::new(0, 0, 0, 0);
    }
    if short_name == "decorated_pot" {
        return material_variant(Rgba::new(132, 94, 54, 255), short_name);
    }
    if matches!(short_name, "grass_path" | "dirt_path") {
        return material_variant(Rgba::new(154, 118, 62, 255), short_name);
    }
    if matches!(short_name, "bamboo" | "bamboo_sapling") {
        return material_variant(Rgba::new(68, 176, 32, 255), short_name);
    }
    if short_name.contains("bamboo")
        && (short_name.contains("planks")
            || short_name.contains("mosaic")
            || short_name.contains("block")
            || short_name.contains("door")
            || short_name.contains("trapdoor")
            || short_name.contains("fence")
            || short_name.contains("slab")
            || short_name.contains("stairs")
            || short_name.contains("button")
            || short_name.contains("pressure_plate")
            || short_name.contains("sign"))
    {
        return material_variant(Rgba::new(174, 150, 70, 255), short_name);
    }
    if let Some((_, color)) = dye_prefix(short_name) {
        return material_variant(color, short_name);
    }
    if short_name.contains("water") {
        return Rgba::new(168, 190, 224, 210);
    }
    if clean_room_grass_tint_mask(short_name) {
        return Rgba::new(214, 214, 214, 255);
    }
    if short_name.contains("leaves") || short_name.contains("leaf") {
        if short_name.contains("leaf_litter") {
            return material_variant(Rgba::new(132, 94, 54, 255), short_name);
        }
        return Rgba::new(206, 206, 206, 238);
    }
    let base = if short_name.contains("lava") || short_name.contains("magma") {
        Rgba::new(214, 72, 18, 255)
    } else if short_name.contains("snow") {
        Rgba::new(222, 228, 224, 255)
    } else if short_name.contains("ice") || short_name.contains("frosted") {
        Rgba::new(112, 174, 214, 218)
    } else if short_name.contains("bush")
        || short_name.contains("sapling")
        || short_name.contains("crop")
        || short_name.contains("wheat")
        || short_name.contains("carrot")
        || short_name.contains("potato")
        || short_name.contains("beetroot")
        || short_name.contains("cactus")
        || short_name.contains("sugar_cane")
        || short_name.contains("seagrass")
        || short_name.contains("kelp")
        || short_name.contains("lily_pad")
        || short_name.contains("moss")
        || short_name.contains("roots")
        || short_name.contains("azalea")
        || short_name.contains("propagule")
    {
        Rgba::new(58, 124, 46, 255)
    } else if short_name.contains("cherry") {
        Rgba::new(184, 126, 140, 255)
    } else if short_name.contains("mangrove") || short_name.contains("crimson") {
        Rgba::new(117, 58, 62, 255)
    } else if short_name.contains("warped") {
        Rgba::new(45, 132, 126, 255)
    } else if short_name.contains("log")
        || short_name.contains("wood")
        || short_name.contains("stem")
        || short_name.contains("hyphae")
        || short_name.contains("planks")
        || short_name.contains("fence")
        || short_name.contains("door")
        || short_name.contains("trapdoor")
        || short_name.contains("button")
        || short_name.contains("pressure_plate")
        || short_name.contains("sign")
        || short_name.contains("ladder")
        || short_name.contains("chest")
        || short_name.contains("barrel")
        || short_name.contains("bookshelf")
    {
        wood_family_color(short_name)
    } else if short_name.contains("flower")
        || short_name.contains("tulip")
        || short_name.contains("daisy")
        || short_name.contains("orchid")
        || short_name.contains("allium")
        || short_name.contains("cornflower")
        || short_name.contains("peony")
        || short_name.contains("lilac")
        || short_name.contains("rose")
        || short_name.contains("petals")
        || short_name.contains("blossom")
    {
        Rgba::new(178, 104, 78, 255)
    } else if short_name.contains("sand") || short_name.contains("beach") {
        Rgba::new(184, 168, 112, 255)
    } else if short_name.contains("mud")
        || short_name.contains("dirt")
        || short_name.contains("farmland")
        || short_name.contains("path")
        || short_name.contains("podzol")
        || short_name.contains("mycelium")
    {
        Rgba::new(104, 72, 50, 255)
    } else if short_name.contains("netherrack")
        || short_name.contains("nether")
        || short_name.contains("nylium")
        || short_name.contains("wart")
    {
        Rgba::new(90, 34, 40, 255)
    } else if short_name.contains("basalt")
        || short_name.contains("blackstone")
        || short_name.contains("deepslate")
        || short_name.contains("sculk")
    {
        Rgba::new(54, 55, 62, 255)
    } else if short_name.contains("stone")
        || short_name.contains("slate")
        || short_name.contains("tuff")
        || short_name.contains("calcite")
        || short_name.contains("dripstone")
        || short_name.contains("andesite")
        || short_name.contains("diorite")
        || short_name.contains("granite")
        || short_name.contains("cobble")
        || short_name.contains("brick")
        || short_name.contains("polished")
        || short_name.contains("smooth")
        || short_name.contains("chiseled")
        || short_name.contains("tile")
        || short_name.contains("ore")
    {
        stone_family_color(short_name)
    } else if short_name.contains("copper") {
        Rgba::new(150, 84, 60, 255)
    } else if short_name.contains("prismarine") {
        Rgba::new(62, 124, 120, 255)
    } else if short_name.contains("amethyst") {
        Rgba::new(132, 88, 176, 255)
    } else if short_name.contains("resin") {
        Rgba::new(194, 86, 24, 255)
    } else if short_name.contains("end_stone") || short_name.contains("end_brick") {
        Rgba::new(190, 190, 130, 255)
    } else if short_name.contains("obsidian") || short_name.contains("end_portal") {
        Rgba::new(29, 24, 42, 255)
    } else if short_name.contains("bedrock") {
        Rgba::new(80, 80, 83, 255)
    } else if short_name.contains("glass") {
        Rgba::new(150, 194, 208, 126)
    } else if short_name.contains("coral") {
        Rgba::new(170, 78, 100, 255)
    } else if short_name.contains("torch")
        || short_name.contains("lantern")
        || short_name.contains("rail")
        || short_name.contains("redstone")
        || short_name.contains("repeater")
        || short_name.contains("comparator")
        || short_name.contains("bell")
    {
        Rgba::new(150, 112, 60, 255)
    } else if short_name.contains("fire") || short_name.contains("shroomlight") {
        Rgba::new(202, 104, 42, 220)
    } else if short_name.contains("terracotta") {
        Rgba::new(126, 70, 52, 255)
    } else if short_name.contains("concrete") {
        Rgba::new(112, 113, 112, 255)
    } else if short_name.contains("wool") || short_name.contains("carpet") {
        Rgba::new(164, 166, 164, 255)
    } else {
        Rgba::new(108, 106, 100, 255)
    };
    material_variant(base, short_name)
}

fn clean_room_grass_tint_mask(name: &str) -> bool {
    matches!(name, "grass" | "short_grass" | "tall_grass")
        || name.contains("grass_block")
        || name.contains("fern")
        || name.contains("vine")
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
struct DyeMaterial {
    id: i32,
    name: &'static str,
    resource_token: &'static str,
    color: Rgba,
}

const DYE_MATERIALS: &[DyeMaterial] = &[
    DyeMaterial {
        id: 0,
        name: "white",
        resource_token: "white",
        color: Rgba::new(222, 226, 224, 255),
    },
    DyeMaterial {
        id: 1,
        name: "orange",
        resource_token: "orange",
        color: Rgba::new(211, 102, 28, 255),
    },
    DyeMaterial {
        id: 2,
        name: "magenta",
        resource_token: "magenta",
        color: Rgba::new(178, 75, 190, 255),
    },
    DyeMaterial {
        id: 3,
        name: "light_blue",
        resource_token: "light_blue",
        color: Rgba::new(73, 165, 205, 255),
    },
    DyeMaterial {
        id: 4,
        name: "yellow",
        resource_token: "yellow",
        color: Rgba::new(223, 181, 45, 255),
    },
    DyeMaterial {
        id: 5,
        name: "lime",
        resource_token: "lime",
        color: Rgba::new(112, 177, 45, 255),
    },
    DyeMaterial {
        id: 6,
        name: "pink",
        resource_token: "pink",
        color: Rgba::new(218, 128, 161, 255),
    },
    DyeMaterial {
        id: 7,
        name: "gray",
        resource_token: "gray",
        color: Rgba::new(76, 79, 78, 255),
    },
    DyeMaterial {
        id: 8,
        name: "light_gray",
        resource_token: "silver",
        color: Rgba::new(150, 151, 144, 255),
    },
    DyeMaterial {
        id: 9,
        name: "cyan",
        resource_token: "cyan",
        color: Rgba::new(37, 132, 139, 255),
    },
    DyeMaterial {
        id: 10,
        name: "purple",
        resource_token: "purple",
        color: Rgba::new(117, 65, 155, 255),
    },
    DyeMaterial {
        id: 11,
        name: "blue",
        resource_token: "blue",
        color: Rgba::new(57, 73, 151, 255),
    },
    DyeMaterial {
        id: 12,
        name: "brown",
        resource_token: "brown",
        color: Rgba::new(109, 74, 43, 255),
    },
    DyeMaterial {
        id: 13,
        name: "green",
        resource_token: "green",
        color: Rgba::new(82, 116, 49, 255),
    },
    DyeMaterial {
        id: 14,
        name: "red",
        resource_token: "red",
        color: Rgba::new(155, 54, 48, 255),
    },
    DyeMaterial {
        id: 15,
        name: "black",
        resource_token: "black",
        color: Rgba::new(31, 34, 38, 255),
    },
];

fn dye_prefix(name: &str) -> Option<(&'static str, Rgba)> {
    dye_material_for_prefixed_name(name).map(|material| (material.name, material.color))
}

fn dye_material_for_prefixed_name(name: &str) -> Option<&'static DyeMaterial> {
    DYE_MATERIALS
        .iter()
        .find(|material| name.starts_with(&format!("{}_", material.name)))
}

fn wood_family_color(name: &str) -> Rgba {
    if name.contains("birch") {
        Rgba::new(164, 146, 92, 255)
    } else if name.contains("spruce") {
        Rgba::new(86, 60, 36, 255)
    } else if name.contains("jungle") {
        Rgba::new(126, 82, 60, 255)
    } else if name.contains("acacia") {
        Rgba::new(138, 68, 40, 255)
    } else if name.contains("dark_oak") {
        Rgba::new(56, 36, 24, 255)
    } else if name.contains("mangrove") {
        Rgba::new(88, 42, 40, 255)
    } else if name.contains("cherry") {
        Rgba::new(184, 128, 140, 255)
    } else if name.contains("crimson") {
        Rgba::new(100, 36, 64, 255)
    } else if name.contains("warped") {
        Rgba::new(38, 110, 106, 255)
    } else {
        Rgba::new(126, 96, 56, 255)
    }
}

fn stone_family_color(name: &str) -> Rgba {
    if name.contains("granite") {
        Rgba::new(120, 82, 68, 255)
    } else if name.contains("diorite") || name.contains("calcite") || name.contains("quartz") {
        Rgba::new(166, 164, 156, 255)
    } else if name.contains("andesite") {
        Rgba::new(106, 108, 106, 255)
    } else if name.contains("tuff") {
        Rgba::new(76, 80, 74, 255)
    } else if name.contains("deepslate") {
        Rgba::new(58, 58, 64, 255)
    } else if name.contains("dripstone") {
        Rgba::new(108, 78, 62, 255)
    } else {
        Rgba::new(100, 102, 100, 255)
    }
}

fn material_variant(color: Rgba, name: &str) -> Rgba {
    let mut amount = 7_i16;
    if name.contains("slab")
        || name.contains("stairs")
        || name.contains("wall")
        || name.contains("fence")
    {
        amount = 4;
    }
    vary_color(color, name, amount)
}

fn vary_color(color: Rgba, key: &str, amount: i16) -> Rgba {
    if color.alpha == 0 || amount == 0 {
        return color;
    }
    let hash = stable_hash(key.as_bytes());
    let red_delta = hash_delta(hash, amount);
    let green_delta = hash_delta(hash.rotate_left(8), amount);
    let blue_delta = hash_delta(hash.rotate_left(16), amount);
    Rgba::new(
        adjust_channel(color.red, red_delta),
        adjust_channel(color.green, green_delta),
        adjust_channel(color.blue, blue_delta),
        color.alpha,
    )
}

fn hash_delta(hash: u32, amount: i16) -> i16 {
    let range = i32::from(amount) * 2 + 1;
    let value = i32::try_from(hash % u32::try_from(range).unwrap_or(1)).unwrap_or(0);
    i16::try_from(value - i32::from(amount)).unwrap_or(0)
}

fn adjust_channel(channel: u8, delta: i16) -> u8 {
    let value = i16::from(channel) + delta;
    u8::try_from(value.clamp(0, 255)).unwrap_or(u8::MAX)
}

fn stable_hash(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn locate_terrain_texture(pack_path: &Path) -> Result<PathBuf> {
    for relative in [
        "textures/terrain_texture.json",
        "terrain_texture.json",
        "resource_pack/textures/terrain_texture.json",
    ] {
        let path = pack_path.join(relative);
        if path.exists() {
            return Ok(path);
        }
    }
    Err(validation(format!(
        "could not find textures/terrain_texture.json under {}",
        pack_path.display()
    )))
}

fn locate_blocks_json(pack_path: &Path) -> Result<PathBuf> {
    for relative in ["blocks.json", "resource_pack/blocks.json"] {
        let path = pack_path.join(relative);
        if path.exists() {
            return Ok(path);
        }
    }
    Err(validation(format!(
        "could not find blocks.json under {}",
        pack_path.display()
    )))
}

#[cfg(feature = "png")]
fn locate_biomes_client(pack_path: &Path) -> Option<PathBuf> {
    for relative in ["biomes_client.json", "resource_pack/biomes_client.json"] {
        let path = pack_path.join(relative);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn texture_paths_from_terrain_json(value: &Value) -> Result<BTreeMap<String, Vec<String>>> {
    let texture_data = value
        .get("texture_data")
        .and_then(Value::as_object)
        .ok_or_else(|| validation("terrain_texture.json is missing texture_data object"))?;
    let mut textures = BTreeMap::new();
    for (name, entry) in texture_data {
        let mut paths = Vec::new();
        collect_texture_paths(entry, &mut paths);
        dedup_preserve_order(&mut paths);
        if !paths.is_empty() {
            textures.insert(name.clone(), paths);
        }
    }
    Ok(textures)
}

#[derive(Debug, Clone, Default)]
struct BlockTextureSpec {
    top: Vec<String>,
    side: Vec<String>,
}

impl BlockTextureSpec {
    fn dedup(&mut self) {
        dedup_preserve_order(&mut self.top);
        dedup_preserve_order(&mut self.side);
    }

    fn is_empty(&self) -> bool {
        self.top.is_empty() && self.side.is_empty()
    }
}

fn block_texture_specs_from_blocks_json(
    value: &Value,
) -> Result<BTreeMap<String, BlockTextureSpec>> {
    let Some(map) = value.as_object() else {
        return Err(validation("blocks.json root must be an object"));
    };
    let mut blocks = BTreeMap::new();
    for (name, entry) in map {
        if name == "format_version" {
            continue;
        }
        let mut spec = entry
            .get("textures")
            .map(block_texture_spec)
            .unwrap_or_default();
        spec.dedup();
        if !spec.is_empty() {
            blocks.insert(normalize_block_name(name), spec);
        }
    }
    Ok(blocks)
}

fn block_texture_spec(value: &Value) -> BlockTextureSpec {
    let mut spec = BlockTextureSpec::default();
    collect_block_texture_spec(value, &mut spec);
    if spec.top.is_empty() {
        spec.top.extend(spec.side.clone());
    }
    if spec.side.is_empty() {
        spec.side.extend(spec.top.clone());
    }
    spec.dedup();
    spec
}

fn collect_block_texture_spec(value: &Value, spec: &mut BlockTextureSpec) {
    match value {
        Value::String(name) => {
            spec.top.push(name.clone());
            spec.side.push(name.clone());
        }
        Value::Array(values) => {
            for value in values {
                collect_block_texture_spec(value, spec);
            }
        }
        Value::Object(map) => {
            collect_face_textures(map, &["up", "top"], &mut spec.top);
            collect_face_textures(
                map,
                &["side", "north", "south", "east", "west"],
                &mut spec.side,
            );
            collect_face_textures(map, &["*", "all"], &mut spec.top);
            collect_face_textures(map, &["*", "all"], &mut spec.side);
            if spec.top.is_empty() {
                spec.top.extend(spec.side.clone());
            }
            let known = [
                "up", "top", "side", "north", "south", "east", "west", "down", "*", "all",
            ];
            for (key, value) in map {
                if !known.contains(&key.as_str()) {
                    collect_block_texture_spec(value, spec);
                }
            }
        }
        _ => {}
    }
}

fn collect_face_textures(map: &Map<String, Value>, keys: &[&str], output: &mut Vec<String>) {
    for key in keys {
        if let Some(value) = map.get(*key) {
            collect_texture_aliases(value, output);
        }
    }
}

fn collect_texture_aliases(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::String(name) => output.push(name.clone()),
        Value::Array(values) => {
            for value in values {
                collect_texture_aliases(value, output);
            }
        }
        Value::Object(map) => {
            for key in ["texture", "textures", "path"] {
                if let Some(value) = map.get(key) {
                    collect_texture_aliases(value, output);
                }
            }
        }
        _ => {}
    }
}

fn resource_pack_texture_spec(
    block_textures: &BTreeMap<String, BlockTextureSpec>,
    block_name: &str,
) -> BlockTextureSpec {
    if let Some(spec) = block_textures.get(block_name) {
        return spec.clone();
    }
    let short_name = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    let mut spec = BlockTextureSpec {
        top: vec![short_name.to_string()],
        side: vec![short_name.to_string()],
    };
    spec.dedup();
    spec
}

#[derive(Debug, Clone)]
struct AliasTable {
    exact: BTreeMap<String, Vec<String>>,
    family: Vec<FamilyAliasRule>,
}

#[derive(Debug, Clone)]
struct FamilyAliasRule {
    id: String,
    contains: Option<String>,
    suffixes: Vec<String>,
    strip_suffix: bool,
    target_candidates: Vec<String>,
    materials: Vec<(String, String)>,
}

#[cfg(feature = "png")]
#[derive(Debug)]
struct AliasColor {
    rule: String,
    target: String,
    color: Rgba,
    side: Option<Rgba>,
}

fn read_alias_table() -> Result<AliasTable> {
    let path = PathBuf::from(ALIAS_JSON);
    if !path.exists() {
        return Ok(AliasTable {
            exact: BTreeMap::new(),
            family: Vec::new(),
        });
    }
    let value = read_json(&path)?;
    AliasTable::parse(&value)
}

impl AliasTable {
    fn parse(value: &Value) -> Result<Self> {
        let Some(root) = value.as_object() else {
            return Err(validation(
                "resource-pack alias table root must be an object",
            ));
        };
        let mut exact = BTreeMap::new();
        if let Some(map) = root.get("exact").and_then(Value::as_object) {
            for (name, value) in map {
                let targets = alias_targets_from_value(value);
                if !targets.is_empty() {
                    exact.insert(normalize_block_name(name), targets);
                }
            }
        }
        let mut family = Vec::new();
        if let Some(rules) = root.get("family_aliases").and_then(Value::as_array) {
            for rule in rules {
                let Some(map) = rule.as_object() else {
                    continue;
                };
                let id = map
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("resource-pack-family-alias")
                    .to_string();
                let contains = map
                    .get("contains")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let suffixes = string_array(map.get("suffixes"));
                let strip_suffix = map
                    .get("strip_suffix")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let target_candidates = string_array(map.get("target_candidates"));
                let mut materials = Vec::new();
                if let Some(material_map) = map.get("materials").and_then(Value::as_object) {
                    for (needle, target) in material_map {
                        if let Some(target) = target.as_str() {
                            materials.push((needle.clone(), target.to_string()));
                        }
                    }
                }
                materials.sort_by(|(left, _), (right, _)| {
                    right.len().cmp(&left.len()).then_with(|| left.cmp(right))
                });
                family.push(FamilyAliasRule {
                    id,
                    contains,
                    suffixes,
                    strip_suffix,
                    target_candidates,
                    materials,
                });
            }
        }
        Ok(Self { exact, family })
    }

    fn targets_for_block(&self, block_name: &str) -> Vec<AliasTarget> {
        let short_name = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
        let mut targets = Vec::new();
        if let Some(exact) = self.exact.get(block_name) {
            targets.extend(exact.iter().map(|target| AliasTarget {
                rule: "exact".to_string(),
                target: target.clone(),
            }));
        }
        for rule in &self.family {
            targets.extend(rule.targets_for_short_name(short_name));
        }
        targets
    }
}

impl FamilyAliasRule {
    fn targets_for_short_name(&self, short_name: &str) -> Vec<AliasTarget> {
        if self
            .contains
            .as_ref()
            .is_some_and(|needle| !short_name.contains(needle))
        {
            return Vec::new();
        }
        let matched_suffix = self
            .suffixes
            .iter()
            .find(|suffix| short_name.ends_with(suffix.as_str()));
        if !self.suffixes.is_empty() && matched_suffix.is_none() {
            return Vec::new();
        }
        let short_without_suffix = matched_suffix.map_or(short_name, |suffix| {
            short_name
                .strip_suffix(suffix)
                .unwrap_or(short_name)
                .trim_end_matches('_')
        });
        let mut targets = Vec::new();
        for (needle, target) in &self.materials {
            if short_name.contains(needle) {
                targets.push(self.alias_target(target, short_name, short_without_suffix));
            }
        }
        if self.strip_suffix && short_without_suffix != short_name {
            targets.push(self.alias_target(short_without_suffix, short_name, short_without_suffix));
        }
        for target in &self.target_candidates {
            targets.push(self.alias_target(target, short_name, short_without_suffix));
        }
        targets
    }

    fn alias_target(
        &self,
        target: &str,
        short_name: &str,
        short_without_suffix: &str,
    ) -> AliasTarget {
        AliasTarget {
            rule: self.id.clone(),
            target: target
                .replace("{short_without_suffix}", short_without_suffix)
                .replace("{short}", short_name),
        }
    }
}

#[derive(Debug, Clone)]
struct AliasTarget {
    rule: String,
    target: String,
}

fn alias_targets_from_value(value: &Value) -> Vec<String> {
    match value {
        Value::String(target) => vec![target.clone()],
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Value::Object(map) => map
            .get("target")
            .and_then(Value::as_str)
            .map(|target| vec![target.to_string()])
            .or_else(|| {
                map.get("target_candidates")
                    .map(|value| string_array(Some(value)))
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn block_inventory_from_blocks_json(value: &Value) -> Result<BTreeSet<String>> {
    let Some(map) = value.as_object() else {
        return Err(validation("blocks.json root must be an object"));
    };
    Ok(map
        .keys()
        .filter(|name| name.as_str() != "format_version")
        .map(|name| normalize_block_name(name))
        .collect())
}

fn collect_texture_paths(value: &Value, paths: &mut Vec<String>) {
    match value {
        Value::String(path) => paths.push(path.clone()),
        Value::Array(values) => {
            for value in values {
                collect_texture_paths(value, paths);
            }
        }
        Value::Object(map) => {
            for key in ["textures", "path"] {
                if let Some(value) = map.get(key) {
                    collect_texture_paths(value, paths);
                }
            }
        }
        _ => {}
    }
}

#[cfg(feature = "png")]
fn average_first_existing_texture(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    texture_names: &[String],
    choice: TextureChoice,
) -> Result<Option<Rgba>> {
    for texture_name in texture_names {
        if let Some(color) = average_direct_texture(pack_path, texture_name)? {
            return Ok(Some(color));
        }
        let Some(paths) = texture_paths.get(texture_name) else {
            continue;
        };
        for texture_path in select_texture_paths(paths, choice) {
            let path = pack_path.join(format!("{texture_path}.png"));
            if path.exists() {
                return average_texture(&path).map(Some);
            }
        }
    }
    Ok(None)
}

#[cfg(feature = "png")]
fn average_alias_texture(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    block_textures: &BTreeMap<String, BlockTextureSpec>,
    aliases: &AliasTable,
    block_name: &str,
) -> Result<Option<AliasColor>> {
    for target in aliases.targets_for_block(block_name) {
        let texture_names = alias_texture_names(block_textures, &target.target);
        if let Some(color) = average_first_existing_texture(
            pack_path,
            texture_paths,
            &texture_names,
            TextureChoice::First,
        )? {
            let side = average_alias_side_texture(
                pack_path,
                texture_paths,
                block_name,
                &target.target,
                color,
            )?;
            return Ok(Some(AliasColor {
                rule: target.rule,
                target: target.target,
                color,
                side,
            }));
        }
    }
    Ok(None)
}

#[cfg(feature = "png")]
fn average_alias_side_texture(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    block_name: &str,
    target: &str,
    top: Rgba,
) -> Result<Option<Rgba>> {
    let short_name = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    if !is_pillar_resource_block(short_name) {
        return Ok(None);
    }
    let side_texture_names = alias_side_texture_names(target);
    if side_texture_names.is_empty() {
        return Ok(None);
    }
    let Some(side) = average_first_existing_texture(
        pack_path,
        texture_paths,
        &side_texture_names,
        TextureChoice::First,
    )?
    else {
        return Ok(None);
    };
    if side == top {
        return Ok(None);
    }
    Ok(Some(side))
}

#[cfg(feature = "png")]
fn insert_special_resource_pack_colors(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    block_textures: &BTreeMap<String, BlockTextureSpec>,
    block_name: &str,
    entry: &mut Map<String, Value>,
) -> Result<()> {
    let short_name = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    insert_common_dyed_state_colors(pack_path, texture_paths, short_name, entry)?;
    insert_sand_state_colors(pack_path, texture_paths, short_name, entry)?;
    insert_candle_color(pack_path, texture_paths, short_name, entry)?;
    match short_name {
        "piston" => {
            if let Some(color) =
                average_special_texture(pack_path, texture_paths, &["piston_top_normal"])?
            {
                set_resource_pack_special_color(entry, color);
                insert_variant_color(entry, "piston_head", color);
            }
        }
        "sticky_piston" => {
            if let Some(color) =
                average_special_texture(pack_path, texture_paths, &["piston_top_sticky"])?
            {
                set_resource_pack_special_color(entry, color);
                insert_variant_color(entry, "sticky_piston_head", color);
            }
        }
        "pistonArmCollision" | "piston_arm_collision" => {
            if let Some(color) = average_special_texture(
                pack_path,
                texture_paths,
                &["textures/entity/pistonarm/pistonArm", "piston_top_normal"],
            )? {
                set_resource_pack_special_color(entry, color);
                insert_variant_color(entry, "piston_arm", color);
            }
        }
        "stickyPistonArmCollision" | "sticky_piston_arm_collision" => {
            if let Some(color) = average_special_texture(
                pack_path,
                texture_paths,
                &[
                    "textures/entity/pistonarm/pistonArmSticky",
                    "piston_top_sticky",
                ],
            )? {
                set_resource_pack_special_color(entry, color);
                insert_variant_color(entry, "sticky_piston_arm", color);
            }
        }
        "movingBlock" | "moving_block" => {
            if let Some(color) = average_special_texture(
                pack_path,
                texture_paths,
                &["textures/entity/pistonarm/pistonArm", "piston_top_normal"],
            )? {
                insert_variant_color(entry, "piston_arm", color);
            }
        }
        "standing_banner" | "wall_banner" => {
            insert_banner_variant_colors(pack_path, texture_paths, block_textures, entry)?;
        }
        "bed" => {
            insert_bed_variant_colors(pack_path, texture_paths, entry)?;
        }
        "decorated_pot" => {
            insert_decorated_pot_colors(pack_path, texture_paths, entry)?;
        }
        name if name.contains("chain") || name.contains("lantern") => {
            if let Some(color) = special_chain_or_lantern_color(pack_path, texture_paths, name)? {
                set_resource_pack_special_color(entry, color);
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(feature = "png")]
fn insert_sand_state_colors(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    short_name: &str,
    entry: &mut Map<String, Value>,
) -> Result<()> {
    if short_name != "sand" {
        return Ok(());
    }
    let sand = average_first_existing_texture(
        pack_path,
        texture_paths,
        &["sand".to_string()],
        TextureChoice::First,
    )?
    .unwrap_or(Rgba::new(219, 207, 163, 255));
    let red_sand = average_first_existing_texture(
        pack_path,
        texture_paths,
        &["red_sand".to_string()],
        TextureChoice::First,
    )?
    .unwrap_or(Rgba::new(190, 102, 33, 255));
    let rules = json!([
        {"min": 0, "max": 0, "color": color_value(sand)},
        {"values": ["normal", "sand"], "color": color_value(sand)},
        {"min": 1, "max": 1, "color": color_value(red_sand)},
        {"values": ["red", "red_sand"], "color": color_value(red_sand)}
    ]);
    for key in ["sand_type", "sand_color", "variant", "data", "damage"] {
        insert_state_color_rules(entry, key, rules.clone());
    }
    Ok(())
}

#[cfg(feature = "png")]
fn insert_common_dyed_state_colors(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    short_name: &str,
    entry: &mut Map<String, Value>,
) -> Result<()> {
    let Some(family) = dyed_state_family(short_name) else {
        return Ok(());
    };
    let mut colors = Vec::with_capacity(DYE_MATERIALS.len());
    for material in DYE_MATERIALS {
        let texture_names = dyed_state_texture_names(family, material);
        let color = average_first_existing_texture(
            pack_path,
            texture_paths,
            &texture_names,
            TextureChoice::First,
        )?
        .unwrap_or(material.color);
        colors.push((*material, color));
    }
    insert_dye_state_color_rules(entry, &colors);
    Ok(())
}

#[cfg(feature = "png")]
fn dyed_state_family(short_name: &str) -> Option<DyedStateFamily> {
    match short_name {
        "concrete" => Some(DyedStateFamily::Concrete),
        "concretePowder" | "concrete_powder" => Some(DyedStateFamily::ConcretePowder),
        "wool" | "carpet" => Some(DyedStateFamily::Wool),
        "stained_glass" => Some(DyedStateFamily::StainedGlass),
        "stained_glass_pane" => Some(DyedStateFamily::StainedGlassPane),
        "shulker_box" => Some(DyedStateFamily::ShulkerBox),
        "stained_hardened_clay" | "terracotta" => Some(DyedStateFamily::Terracotta),
        _ => None,
    }
}

#[cfg(feature = "png")]
#[derive(Clone, Copy)]
enum DyedStateFamily {
    Concrete,
    ConcretePowder,
    Wool,
    StainedGlass,
    StainedGlassPane,
    ShulkerBox,
    Terracotta,
}

#[cfg(feature = "png")]
fn dyed_state_texture_names(family: DyedStateFamily, material: &DyeMaterial) -> Vec<String> {
    let token = material.resource_token;
    match family {
        DyedStateFamily::Concrete => vec![format!("concrete_{token}")],
        DyedStateFamily::ConcretePowder => vec![format!("concrete_powder_{token}")],
        DyedStateFamily::Wool => vec![format!("wool_colored_{token}")],
        DyedStateFamily::StainedGlass => vec![format!("glass_{token}")],
        DyedStateFamily::StainedGlassPane => vec![format!("glass_pane_top_{token}")],
        DyedStateFamily::ShulkerBox => vec![format!("shulker_top_{token}")],
        DyedStateFamily::Terracotta => vec![format!("hardened_clay_stained_{token}")],
    }
}

#[cfg(feature = "png")]
fn insert_dye_state_color_rules(entry: &mut Map<String, Value>, colors: &[(DyeMaterial, Rgba)]) {
    for key in [
        "color",
        "color_bit",
        "color_index",
        "dye_color",
        "data",
        "damage",
        "variant",
    ] {
        insert_state_color_rules(entry, key, dye_state_rules(colors));
    }
}

#[cfg(feature = "png")]
fn dye_state_rules(colors: &[(DyeMaterial, Rgba)]) -> Value {
    let mut rules = Vec::with_capacity(colors.len() * 2);
    for (material, color) in colors {
        rules.push(json!({
            "min": material.id,
            "max": material.id,
            "color": color_value(*color),
        }));
        let mut values = vec![
            material.name.to_string(),
            material.resource_token.to_string(),
        ];
        if material.name == "light_gray" {
            values.push("silver".to_string());
        }
        values.sort();
        values.dedup();
        rules.push(json!({
            "values": values,
            "color": color_value(*color),
        }));
    }
    Value::Array(rules)
}

#[cfg(feature = "png")]
fn insert_state_color_rules(entry: &mut Map<String, Value>, key: &str, rules: Value) {
    let state_colors = entry
        .entry("state_colors".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(state_colors) = state_colors.as_object_mut() else {
        return;
    };
    state_colors.insert(key.to_string(), rules);
}

#[cfg(feature = "png")]
fn average_special_texture(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    texture_names: &[&str],
) -> Result<Option<Rgba>> {
    let texture_names = texture_names
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    average_first_existing_texture(
        pack_path,
        texture_paths,
        &texture_names,
        TextureChoice::First,
    )
}

#[cfg(feature = "png")]
fn insert_banner_variant_colors(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    _block_textures: &BTreeMap<String, BlockTextureSpec>,
    entry: &mut Map<String, Value>,
) -> Result<()> {
    let mut default_color = None;
    for (variant, texture) in BANNER_BASE_TEXTURES {
        if let Some(color) = average_special_texture(pack_path, texture_paths, &[*texture])? {
            if *variant == "banner_base_white" {
                default_color = Some(color);
            }
            insert_variant_color(entry, variant, color);
        }
    }
    if let Some(color) = default_color {
        set_resource_pack_special_color(entry, color);
    }
    Ok(())
}

#[cfg(feature = "png")]
fn insert_bed_variant_colors(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    entry: &mut Map<String, Value>,
) -> Result<()> {
    let mut red = None;
    let mut white = None;
    let mut colors = Vec::with_capacity(DYE_MATERIALS.len());
    for material in DYE_MATERIALS {
        let texture_names = vec![
            format!("textures/items/bed_{}", material.resource_token),
            format!("textures/items/bed_{}", material.name),
            format!("bed_{}", material.resource_token),
            format!("bed_{}", material.name),
        ];
        let color = average_first_existing_texture(
            pack_path,
            texture_paths,
            &texture_names,
            TextureChoice::First,
        )?
        .unwrap_or(material.color);
        if material.name == "red" {
            red = Some(color);
        } else if material.name == "white" {
            white = Some(color);
        }
        insert_variant_color(entry, &format!("bed_{}", material.name), color);
        colors.push((*material, color));
    }
    insert_dye_state_color_rules(entry, &colors);
    if let Some(color) = red.or(white) {
        set_resource_pack_special_color(entry, color);
    }
    Ok(())
}

#[cfg(feature = "png")]
fn insert_decorated_pot_colors(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    entry: &mut Map<String, Value>,
) -> Result<()> {
    if let Some(color) = average_special_texture(
        pack_path,
        texture_paths,
        &[
            "decorated_pot",
            "textures/blocks/decorated_pot",
            "textures/entity/decorated_pot/decorated_pot",
            "textures/entity/decorated_pot/decorated_pot_side",
            "pottery",
        ],
    )? {
        set_resource_pack_special_color(entry, color);
        insert_variant_color(entry, "decorated_pot_base", color);
        return Ok(());
    }

    let color = Rgba::new(132, 94, 54, 255);
    entry.insert("default".to_string(), color_value(color));
    entry.insert("clean_room_fallback".to_string(), color_value(color));
    entry.insert(
        "fallback_reason".to_string(),
        Value::String("resource-pack-special-texture-unresolved".to_string()),
    );
    insert_variant_color(entry, "decorated_pot_base", color);
    Ok(())
}

#[cfg(feature = "png")]
fn insert_candle_color(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    short_name: &str,
    entry: &mut Map<String, Value>,
) -> Result<()> {
    if !is_candle_block(short_name) {
        return Ok(());
    }
    let texture_names = vec![
        short_name.to_string(),
        format!("textures/blocks/{short_name}"),
        format!("textures/items/{short_name}"),
    ];
    if let Some(color) = average_first_existing_texture(
        pack_path,
        texture_paths,
        &texture_names,
        TextureChoice::First,
    )? {
        set_resource_pack_special_color(entry, color);
        return Ok(());
    }
    let color = candle_fallback_color(short_name);
    entry.insert("default".to_string(), color_value(color));
    entry.insert("clean_room_fallback".to_string(), color_value(color));
    entry.insert(
        "fallback_reason".to_string(),
        Value::String("resource-pack-candle-texture-unresolved".to_string()),
    );
    Ok(())
}

#[cfg(feature = "png")]
fn is_candle_block(short_name: &str) -> bool {
    short_name == "candle"
        || short_name == "candle_cake"
        || short_name.ends_with("_candle")
        || short_name.ends_with("_candle_cake")
}

#[cfg(feature = "png")]
fn candle_fallback_color(short_name: &str) -> Rgba {
    dye_material_for_prefixed_name(short_name)
        .map_or(Rgba::new(198, 158, 82, 255), |material| material.color)
}

#[cfg(feature = "png")]
fn special_chain_or_lantern_color(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    short_name: &str,
) -> Result<Option<Rgba>> {
    let candidates = if short_name == "lantern" {
        vec!["lantern"]
    } else if short_name == "chain" {
        vec!["chain"]
    } else {
        Vec::new()
    };
    average_special_texture(pack_path, texture_paths, &candidates)
}

#[cfg(feature = "png")]
fn set_resource_pack_special_color(entry: &mut Map<String, Value>, color: Rgba) {
    entry.insert("default".to_string(), color_value(color));
    entry.insert("resource_pack_special".to_string(), color_value(color));
    entry.remove("clean_room_fallback");
    entry.remove("fallback_reason");
}

#[cfg(feature = "png")]
fn insert_variant_color(entry: &mut Map<String, Value>, variant: &str, color: Rgba) {
    let variants = entry
        .entry("variant_colors".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(variants) = variants.as_object_mut() else {
        return;
    };
    variants.insert(variant.to_string(), color_value(color));
}

#[cfg(feature = "png")]
const BANNER_BASE_TEXTURES: &[(&str, &str)] = &[
    ("banner_base_white", "wool_colored_white"),
    ("banner_base_orange", "wool_colored_orange"),
    ("banner_base_magenta", "wool_colored_magenta"),
    ("banner_base_light_blue", "wool_colored_light_blue"),
    ("banner_base_yellow", "wool_colored_yellow"),
    ("banner_base_lime", "wool_colored_lime"),
    ("banner_base_pink", "wool_colored_pink"),
    ("banner_base_gray", "wool_colored_gray"),
    ("banner_base_light_gray", "wool_colored_silver"),
    ("banner_base_cyan", "wool_colored_cyan"),
    ("banner_base_purple", "wool_colored_purple"),
    ("banner_base_blue", "wool_colored_blue"),
    ("banner_base_brown", "wool_colored_brown"),
    ("banner_base_green", "wool_colored_green"),
    ("banner_base_red", "wool_colored_red"),
    ("banner_base_black", "wool_colored_black"),
];

#[cfg(feature = "png")]
fn alias_side_texture_names(target: &str) -> Vec<String> {
    let Some(without_top) = target.strip_suffix("_top") else {
        return Vec::new();
    };
    if target.contains('/') {
        return vec![without_top.to_string()];
    }
    vec![
        format!("{without_top}_side"),
        without_top.to_string(),
        format!("textures/blocks/{without_top}"),
    ]
}

#[cfg(feature = "png")]
fn insert_pillar_axis_alias_state_colors(
    block_name: &str,
    top: Rgba,
    side: Rgba,
    entry: &mut Map<String, Value>,
) {
    let short_name = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    if !is_pillar_resource_block(short_name) {
        return;
    }
    entry.insert(
        "state_colors".to_string(),
        json!({
            "pillar_axis": [
                {"values": ["x", "z"], "color": color_value(side)},
                {"values": ["y"], "color": color_value(top)}
            ]
        }),
    );
}

fn alias_texture_names(
    block_textures: &BTreeMap<String, BlockTextureSpec>,
    target: &str,
) -> Vec<String> {
    let normalized = normalize_block_name(target);
    if let Some(spec) = block_textures.get(&normalized) {
        return spec.top.clone();
    }
    vec![target.to_string()]
}

fn first_existing_texture_path(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    texture_names: &[String],
    choice: TextureChoice,
) -> Option<PathBuf> {
    for texture_name in texture_names {
        for candidate in direct_texture_candidates(texture_name) {
            let path = pack_path.join(format!("{candidate}.png"));
            if path.exists() {
                return Some(path);
            }
        }
        let Some(paths) = texture_paths.get(texture_name) else {
            continue;
        };
        for texture_path in select_texture_paths(paths, choice) {
            let path = pack_path.join(format!("{texture_path}.png"));
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(feature = "png")]
fn unresolved_alias_reason(aliases: &AliasTable, block_name: &str) -> Option<String> {
    let targets = aliases.targets_for_block(block_name);
    if targets.is_empty() {
        None
    } else {
        Some(format!(
            "resource-pack-alias-unresolved:{}",
            targets
                .iter()
                .map(|target| format!("{}={}", target.rule, target.target))
                .collect::<Vec<_>>()
                .join(",")
        ))
    }
}

#[cfg(feature = "png")]
fn average_direct_texture(pack_path: &Path, texture_name: &str) -> Result<Option<Rgba>> {
    for candidate in direct_texture_candidates(texture_name) {
        let path = pack_path.join(format!("{candidate}.png"));
        if path.exists() {
            return average_texture(&path).map(Some);
        }
    }
    Ok(None)
}

fn direct_texture_candidates(texture_name: &str) -> Vec<String> {
    if texture_name.contains('/') {
        return vec![texture_name.to_string()];
    }
    vec![
        texture_name.to_string(),
        format!("textures/blocks/{texture_name}"),
    ]
}

#[derive(Clone, Copy)]
enum TextureChoice {
    First,
    Last,
}

fn select_texture_paths(paths: &[String], choice: TextureChoice) -> Vec<&String> {
    match choice {
        TextureChoice::First => paths.first().into_iter().collect(),
        TextureChoice::Last => paths.last().into_iter().collect(),
    }
}

fn special_texture_choice(block_name: &str) -> TextureChoice {
    let name = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    if matches!(name, "wheat" | "carrots" | "potatoes" | "beetroot") {
        return TextureChoice::Last;
    }
    TextureChoice::First
}

#[cfg(feature = "png")]
fn insert_side_color(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    texture_spec: &BlockTextureSpec,
    entry: &mut Map<String, Value>,
) -> Result<Option<Rgba>> {
    if texture_spec.side == texture_spec.top {
        return Ok(None);
    }
    let Some(side) = average_first_existing_texture(
        pack_path,
        texture_paths,
        &texture_spec.side,
        TextureChoice::First,
    )?
    else {
        return Ok(None);
    };
    entry.insert("resource_pack_side".to_string(), color_value(side));
    Ok(Some(side))
}

#[cfg(feature = "png")]
fn insert_state_colors(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    block_name: &str,
    texture_spec: &BlockTextureSpec,
    entry: &mut Map<String, Value>,
) -> Result<()> {
    let mut state_colors = Map::new();
    insert_growth_state_colors(
        pack_path,
        texture_paths,
        block_name,
        entry,
        &mut state_colors,
    )?;
    insert_pillar_axis_state_colors(block_name, texture_spec, entry, &mut state_colors)?;
    if !state_colors.is_empty() {
        entry.insert("state_colors".to_string(), Value::Object(state_colors));
    }
    Ok(())
}

#[cfg(feature = "png")]
fn insert_growth_state_colors(
    pack_path: &Path,
    texture_paths: &BTreeMap<String, Vec<String>>,
    block_name: &str,
    entry: &mut Map<String, Value>,
    state_colors: &mut Map<String, Value>,
) -> Result<()> {
    let Some(stage_count) = crop_stage_count(block_name) else {
        return Ok(());
    };
    let short_name = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    let texture_name = match short_name {
        "wheat" => "wheat",
        "carrots" => "carrots",
        "potatoes" => "potatoes",
        "beetroot" => "beetroot",
        "farmland" => "farmland",
        _ => return Ok(()),
    };
    let Some(paths) = texture_paths.get(texture_name) else {
        return Ok(());
    };
    let mut growth_colors = Vec::new();
    for index in 0..stage_count {
        let Some(texture_path) = paths.get(index).or_else(|| paths.last()) else {
            continue;
        };
        let path = pack_path.join(format!("{texture_path}.png"));
        if path.exists() {
            let color = average_texture(&path)?;
            entry.insert(format!("resource_pack_stage_{index}"), color_value(color));
            growth_colors.push(color);
        }
    }
    match short_name {
        "wheat" if growth_colors.len() >= 8 => {
            let growth_rules = growth_colors
                .iter()
                .enumerate()
                .map(|(index, color)| {
                    let stage = i32::try_from(index).map_err(|_| {
                        validation(format!("growth stage index {index} is too large"))
                    })?;
                    Ok((stage, stage, *color))
                })
                .collect::<Result<Vec<_>>>()?;
            state_colors.insert("growth".to_string(), state_range_rules(growth_rules));
        }
        "carrots" | "potatoes" | "beetroot" if growth_colors.len() >= 4 => {
            state_colors.insert(
                "growth".to_string(),
                state_range_rules(vec![
                    (0, 1, growth_colors[0]),
                    (2, 3, growth_colors[1]),
                    (4, 6, growth_colors[2]),
                    (7, 7, growth_colors[3]),
                ]),
            );
        }
        "farmland" if growth_colors.len() >= 2 => {
            let wet = growth_colors[0];
            let dry = growth_colors[1];
            state_colors.insert(
                "moisturized_amount".to_string(),
                state_range_rules(vec![(0, 0, dry), (1, 7, wet)]),
            );
        }
        _ => {}
    }
    Ok(())
}

#[cfg(feature = "png")]
#[allow(clippy::unnecessary_wraps)]
fn insert_pillar_axis_state_colors(
    block_name: &str,
    texture_spec: &BlockTextureSpec,
    entry: &Map<String, Value>,
    state_colors: &mut Map<String, Value>,
) -> Result<()> {
    let short_name = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    if !is_pillar_resource_block(short_name) {
        return Ok(());
    }
    let Some(side) = entry
        .get("resource_pack_side")
        .and_then(parse_rgba_value_option)
    else {
        return Ok(());
    };
    let top = entry
        .get("resource_pack_top")
        .or_else(|| entry.get("default"))
        .and_then(parse_rgba_value_option)
        .unwrap_or(side);
    if texture_spec.side == texture_spec.top {
        return Ok(());
    }
    state_colors.insert(
        "pillar_axis".to_string(),
        Value::Array(vec![
            json!({"values": ["x", "z"], "color": color_value(side)}),
            json!({"values": ["y"], "color": color_value(top)}),
        ]),
    );
    Ok(())
}

#[cfg(feature = "png")]
fn state_range_rules(rules: Vec<(i32, i32, Rgba)>) -> Value {
    Value::Array(
        rules
            .into_iter()
            .map(|(min, max, color)| json!({"min": min, "max": max, "color": color_value(color)}))
            .collect(),
    )
}

#[cfg(feature = "png")]
fn parse_rgba_value_option(value: &Value) -> Option<Rgba> {
    parse_rgba_value(value, "state-color").ok()
}

#[cfg(feature = "png")]
fn is_pillar_resource_block(name: &str) -> bool {
    name.contains("log")
        || name.ends_with("_wood")
        || name.contains("stem")
        || name.contains("hyphae")
        || name == "bamboo_block"
}

#[cfg(feature = "png")]
fn crop_stage_count(block_name: &str) -> Option<usize> {
    match block_name.strip_prefix("minecraft:").unwrap_or(block_name) {
        "wheat" => Some(8),
        "carrots" | "potatoes" | "beetroot" => Some(4),
        "farmland" => Some(2),
        _ => None,
    }
}

#[cfg(feature = "png")]
fn average_texture(path: &Path) -> Result<Rgba> {
    let image = image::ImageReader::open(path)
        .map_err(|error| {
            BedrockRenderError::io(format!("failed to open {}", path.display()), error)
        })?
        .with_guessed_format()
        .map_err(|error| {
            BedrockRenderError::io(format!("failed to inspect {}", path.display()), error)
        })?
        .decode()
        .map_err(|error| {
            BedrockRenderError::image(format!("failed to decode {}", path.display()), error)
        })?
        .to_rgba8();
    let mut red = 0_u64;
    let mut green = 0_u64;
    let mut blue = 0_u64;
    let mut alpha = 0_u64;
    let mut weight = 0_u64;
    for pixel in image.pixels() {
        let [r, g, b, a] = pixel.0;
        if a < 16 {
            continue;
        }
        let a = u64::from(a);
        red += u64::from(r) * a;
        green += u64::from(g) * a;
        blue += u64::from(b) * a;
        alpha += a;
        weight += a;
    }
    if weight == 0 {
        return Ok(Rgba::new(0, 0, 0, 0));
    }
    Ok(Rgba::new(
        u8_from_u64(red / weight),
        u8_from_u64(green / weight),
        u8_from_u64(blue / weight),
        u8_from_u64(
            alpha
                / u64::from(image.width())
                    .saturating_mul(u64::from(image.height()))
                    .max(1),
        ),
    ))
}

#[cfg(feature = "png")]
fn u8_from_u64(value: u64) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

#[cfg(feature = "png")]
fn u8_from_u32(value: u32) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn dedup_preserve_order(values: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

fn normalize_block_document(value: &Value) -> Result<Value> {
    let blocks = value.get("blocks").unwrap_or(value);
    let Some(blocks) = blocks.as_object() else {
        return Err(validation(
            "block palette JSON must contain an object `blocks` map",
        ));
    };
    let mut normalized_blocks = Map::new();
    let mut block_entries = blocks.iter().collect::<Vec<_>>();
    block_entries.sort_by_key(|(left, _)| *left);
    for (block_name, entry) in block_entries {
        normalized_blocks.insert(
            block_name.clone(),
            normalize_block_entry(block_name, entry)?,
        );
    }

    Ok(json!({
        "schema_version": PALETTE_SCHEMA_VERSION,
        "minecraft_bedrock_version": minecraft_version(value),
        "sources": block_sources(),
        "blocks": normalized_blocks,
    }))
}

fn normalize_block_entry(block_name: &str, entry: &Value) -> Result<Value> {
    let Some(textures) = entry.as_object() else {
        return Err(validation(format!(
            "block `{block_name}` entry must be an object"
        )));
    };
    let mut normalized = Map::new();
    let mut texture_entries = textures.iter().collect::<Vec<_>>();
    texture_entries.sort_by_key(|(left, _)| *left);
    for (key, value) in texture_entries {
        let normalized_value = match key.as_str() {
            "fallback_reason"
            | "resource_pack_alias_rule"
            | "resource_pack_alias_target"
            | "resource_pack_tint_reason"
            | "resource_pack_tint_source" => Value::String(
                value
                    .as_str()
                    .ok_or_else(|| validation(format!("{block_name}.{key} must be a string")))?
                    .to_string(),
            ),
            "state_colors" => normalize_state_colors(block_name, value)?,
            "variant_colors" => normalize_variant_colors(block_name, value)?,
            _ => color_array(value, &format!("{block_name}.{key}"))?,
        };
        normalized.insert(key.clone(), normalized_value);
    }
    Ok(Value::Object(normalized))
}

fn normalize_variant_colors(block_name: &str, value: &Value) -> Result<Value> {
    let Some(map) = value.as_object() else {
        return Err(validation(format!(
            "{block_name}.variant_colors must be an object"
        )));
    };
    let mut normalized = Map::new();
    let mut variants = map.iter().collect::<Vec<_>>();
    variants.sort_by_key(|(left, _)| *left);
    for (variant, color) in variants {
        normalized.insert(
            variant.clone(),
            color_array(color, &format!("{block_name}.variant_colors.{variant}"))?,
        );
    }
    Ok(Value::Object(normalized))
}

fn normalize_state_colors(block_name: &str, value: &Value) -> Result<Value> {
    let Some(map) = value.as_object() else {
        return Err(validation(format!(
            "{block_name}.state_colors must be an object"
        )));
    };
    let mut normalized = Map::new();
    let mut states = map.iter().collect::<Vec<_>>();
    states.sort_by_key(|(left, _)| *left);
    for (state_name, rules) in states {
        let Some(rules) = rules.as_array() else {
            return Err(validation(format!(
                "{block_name}.state_colors.{state_name} must be an array"
            )));
        };
        let mut normalized_rules = Vec::with_capacity(rules.len());
        for rule in rules {
            let Some(rule_map) = rule.as_object() else {
                return Err(validation(format!(
                    "{block_name}.state_colors.{state_name} rule must be an object"
                )));
            };
            let mut normalized_rule = Map::new();
            if let Some(min) = rule_map.get("min") {
                normalized_rule.insert("min".to_string(), int_value(min, "min")?);
            }
            if let Some(max) = rule_map.get("max") {
                normalized_rule.insert("max".to_string(), int_value(max, "max")?);
            }
            if let Some(values) = rule_map.get("values") {
                normalized_rule.insert(
                    "values".to_string(),
                    string_values(values, &format!("{block_name}.{state_name}.values"))?,
                );
            }
            let color = rule_map.get("color").ok_or_else(|| {
                validation(format!(
                    "{block_name}.state_colors.{state_name} rule is missing color"
                ))
            })?;
            normalized_rule.insert(
                "color".to_string(),
                color_array(color, &format!("{block_name}.{state_name}.color"))?,
            );
            normalized_rules.push(Value::Object(normalized_rule));
        }
        normalized.insert(state_name.clone(), Value::Array(normalized_rules));
    }
    Ok(Value::Object(normalized))
}

fn int_value(value: &Value, label: &str) -> Result<Value> {
    value
        .as_i64()
        .map(Value::from)
        .ok_or_else(|| validation(format!("{label} must be an integer")))
}

fn string_values(value: &Value, label: &str) -> Result<Value> {
    let Some(values) = value.as_array() else {
        return Err(validation(format!("{label} must be an array")));
    };
    Ok(Value::Array(
        values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(|value| Value::String(value.to_string()))
                    .ok_or_else(|| validation(format!("{label} entries must be strings")))
            })
            .collect::<Result<Vec<_>>>()?,
    ))
}

fn normalize_biome_document(value: &Value) -> Result<Value> {
    let biomes = value.get("biomes").unwrap_or(value);
    let Some(biomes) = biomes.as_object() else {
        return Err(validation(
            "biome palette JSON must contain an object `biomes` map",
        ));
    };
    let defaults_value = value
        .get("defaults")
        .or_else(|| biomes.get("default"))
        .ok_or_else(|| validation("biome palette JSON must contain `defaults`"))?;
    let defaults = normalize_biome_defaults(defaults_value)?;
    let mut normalized_biomes = Map::new();
    let mut ids = BTreeSet::new();
    let mut entries = biomes
        .iter()
        .filter(|(name, _)| name.as_str() != "default")
        .collect::<Vec<_>>();
    entries.sort_by(|(left_name, left), (right_name, right)| {
        let left_id = left.get("id").and_then(Value::as_u64).unwrap_or(u64::MAX);
        let right_id = right.get("id").and_then(Value::as_u64).unwrap_or(u64::MAX);
        left_id
            .cmp(&right_id)
            .then_with(|| left_name.cmp(right_name))
    });
    for (name, entry) in entries {
        let Some(map) = entry.as_object() else {
            return Err(validation(format!(
                "biome `{name}` entry must be an object"
            )));
        };
        let id = map
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| validation(format!("biome `{name}` is missing integer id")))?;
        let id = u32::try_from(id)
            .map_err(|_| validation(format!("biome `{name}` id is outside u32 range")))?;
        if !ids.insert(id) {
            return Err(validation(format!("duplicate biome id {id} at `{name}`")));
        }
        let mut biome = Map::new();
        biome.insert("id".to_string(), Value::from(id));
        biome.insert(
            "rgb".to_string(),
            color_array(
                map.get("rgb")
                    .ok_or_else(|| validation(format!("biome `{name}` is missing rgb")))?,
                &format!("{name}.rgb"),
            )?,
        );
        for key in ["grass", "leaves", "water"] {
            if let Some(color) = map.get(key) {
                biome.insert(
                    key.to_string(),
                    color_array(color, &format!("{name}.{key}"))?,
                );
            }
        }
        normalized_biomes.insert(name.clone(), Value::Object(biome));
    }

    Ok(json!({
        "schema_version": PALETTE_SCHEMA_VERSION,
        "minecraft_bedrock_version": minecraft_version(value),
        "sources": biome_sources(),
        "defaults": defaults,
        "biomes": normalized_biomes,
    }))
}

fn normalize_biome_defaults(value: &Value) -> Result<Value> {
    let Some(map) = value.as_object() else {
        return Err(validation("biome defaults must be an object"));
    };
    let mut defaults = Map::new();
    for key in ["rgb", "grass", "leaves", "water"] {
        let Some(color) = map.get(key) else {
            return Err(validation(format!("biome defaults are missing `{key}`")));
        };
        defaults.insert(
            key.to_string(),
            color_array(color, &format!("defaults.{key}"))?,
        );
    }
    Ok(Value::Object(defaults))
}

fn color_array(value: &Value, label: &str) -> Result<Value> {
    let Some(channels) = value.as_array() else {
        return Err(validation(format!("{label} color must be an array")));
    };
    if !(channels.len() == 3 || channels.len() == 4) {
        return Err(validation(format!(
            "{label} color must have 3 or 4 channels"
        )));
    }
    let mut output = Vec::with_capacity(4);
    for channel in channels {
        let Some(channel) = channel.as_u64() else {
            return Err(validation(format!(
                "{label} color channel must be an integer"
            )));
        };
        let channel = u8::try_from(channel)
            .map_err(|_| validation(format!("{label} color channel is outside 0..=255")))?;
        output.push(Value::from(channel));
    }
    if output.len() == 3 {
        output.push(Value::from(255_u8));
    }
    Ok(Value::Array(output))
}

fn canonical_json(value: &Value) -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(value).expect("palette JSON should serialize")
    )
}

fn minecraft_version(value: &Value) -> String {
    value
        .get("minecraft_bedrock_version")
        .and_then(Value::as_str)
        .unwrap_or(PALETTE_VERSION)
        .to_string()
}

fn block_sources() -> Value {
    json!([
        {
            "id": "bedrock-render-vanilla-resource-pack-derived-v1",
            "kind": "resource-pack-derived",
            "name": "Minecraft Bedrock vanilla resource pack",
            "description": "Default block colors are generated from local vanilla blocks.json, terrain_texture.json, and block PNG averages with top-face priority.",
            "license": "Minecraft Bedrock vanilla resource pack terms apply to the source assets; this repository stores only derived aggregate color values.",
            "retrieved_at": "2026-05-01",
            "usage": "default-color-values"
        },
        {
            "id": "bedrock-render-clean-room-v1",
            "kind": "project-authored-clean-room",
            "description": "Project-authored fallback colors generated from block identifiers, material categories, and deterministic color rules for missing resource-pack mappings.",
            "license": "MIT OR Apache-2.0 as part of this repository",
            "retrieved_at": "2026-05-01",
            "usage": "fallback-color-values"
        },
        {
            "id": "minecraft-wiki-block-id-reference",
            "kind": "public-reference",
            "name": "Minecraft Wiki Bedrock Edition block ID reference",
            "url": "https://zh.minecraft.wiki/w/%E5%9F%BA%E5%B2%A9%E7%89%88%E6%95%B0%E6%8D%AE%E5%80%BC/%E6%96%B9%E5%9D%97ID",
            "license": "Minecraft Wiki content license applies to the reference page; used for identifiers only, not copied color values or icons.",
            "retrieved_at": "2026-05-01",
            "usage": "id-reference"
        },
        {
            "id": "microsoft-learn-blocks-json",
            "kind": "public-reference",
            "name": "Microsoft Learn blocks.json file structure reference",
            "url": "https://learn.microsoft.com/en-us/minecraft/creator/reference/content/blockreference/examples/blocksjsonfilestructure?view=minecraft-bedrock-stable",
            "license": "Microsoft Learn documentation terms",
            "retrieved_at": "2026-05-01",
            "usage": "blocks-json-structure-reference"
        },
        {
            "id": "microsoft-learn-map-color",
            "kind": "public-reference",
            "name": "Microsoft Learn minecraft:map_color component reference",
            "url": "https://learn.microsoft.com/minecraft/creator/reference/content/blockreference/examples/blockcomponents/minecraftblock_map_color?view=minecraft-bedrock-experimental",
            "license": "Microsoft Learn documentation terms",
            "retrieved_at": "2026-05-01",
            "usage": "semantics-reference"
        },
        {
            "id": "microsoft-minecraft-samples",
            "kind": "public-reference",
            "name": "Microsoft Minecraft samples repository",
            "url": "https://github.com/microsoft/minecraft-samples",
            "license": "Repository license and notices apply",
            "retrieved_at": "2026-05-01",
            "usage": "schema-and-pack-reference"
        }
    ])
}

fn biome_sources() -> Value {
    json!([
        {
            "id": "bedrock-render-vanilla-client-biome-derived-v1",
            "kind": "resource-pack-derived",
            "name": "Minecraft Bedrock vanilla client biome and colormap resources",
            "description": "Default biome tint colors are generated from local vanilla biomes_client.json and textures/colormap with project-authored biome category fallbacks.",
            "license": "Minecraft Bedrock vanilla resource pack terms apply to the source assets; this repository stores only derived aggregate color values.",
            "retrieved_at": "2026-05-01",
            "usage": "default-biome-tint-values"
        },
        {
            "id": "bedrock-render-clean-room-v1",
            "kind": "project-authored-clean-room",
            "description": "Project-authored biome category fallbacks used when client resources do not provide an explicit value.",
            "license": "MIT OR Apache-2.0 as part of this repository",
            "retrieved_at": "2026-05-01",
            "usage": "fallback-biome-values"
        },
        {
            "id": "minecraft-wiki-biome-id-reference",
            "kind": "public-reference",
            "name": "Minecraft Wiki biome reference",
            "url": "https://zh.minecraft.wiki/w/%E7%94%9F%E7%89%A9%E7%BE%A4%E7%B3%BB?variant=zh-cn",
            "license": "Minecraft Wiki content license applies to the reference page; used for biome identifiers and semantics only, not copied color values.",
            "retrieved_at": "2026-05-01",
            "usage": "id-reference-and-semantics"
        },
        {
            "id": "microsoft-learn-client-biomes",
            "kind": "public-reference",
            "name": "Microsoft Learn client biome definition reference",
            "url": "https://learn.microsoft.com/en-us/minecraft/creator/reference/content/clientbiomesreference/examples/components/client_biome_definition?view=minecraft-bedrock-stable",
            "license": "Microsoft Learn documentation terms",
            "retrieved_at": "2026-05-01",
            "usage": "client-biome-schema-reference"
        },
        {
            "id": "mojang-bedrock-samples-client-biomes",
            "kind": "public-reference",
            "name": "Mojang Bedrock samples client biome reference",
            "url": "https://mojang.github.io/bedrock-samples/Client%20Biomes.html",
            "license": "Documentation/sample terms apply",
            "retrieved_at": "2026-05-01",
            "usage": "biome-schema-reference"
        },
        {
            "id": "microsoft-minecraft-samples",
            "kind": "public-reference",
            "name": "Microsoft Minecraft samples repository",
            "url": "https://github.com/microsoft/minecraft-samples",
            "license": "Repository license and notices apply",
            "retrieved_at": "2026-05-01",
            "usage": "schema-and-pack-reference"
        }
    ])
}

fn validation(message: impl Into<String>) -> BedrockRenderError {
    BedrockRenderError::Validation(message.into())
}
