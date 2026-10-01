use std::{collections::HashSet, env, error::Error, fs, path::PathBuf};

const PALETTE_PATH: &str = "data/bedrock-block-palette-26.40.json";
const ID_MANIFEST_PATH: &str = "data/bedrock-block-ids-1.26.40.5.txt";
const BEDROCK_VERSION: &str = "26.40";
const MANIFEST_COMMIT: &str = "7844835b6baad4c0010f46901a4accf87413a022";

const SOLID_COLOR_BLOCKS: &[&str] = &[
    "minecraft:amethyst_block",
    "minecraft:andesite",
    "minecraft:bamboo_mosaic",
    "minecraft:basalt",
    "minecraft:blackstone",
    "minecraft:blue_ice",
    "minecraft:bone_block",
    "minecraft:bricks",
    "minecraft:calcite",
    "minecraft:clay",
    "minecraft:coal_block",
    "minecraft:coarse_dirt",
    "minecraft:cobbled_deepslate",
    "minecraft:cobblestone",
    "minecraft:crimson_nylium",
    "minecraft:crying_obsidian",
    "minecraft:dark_prismarine",
    "minecraft:deepslate",
    "minecraft:diamond_block",
    "minecraft:diorite",
    "minecraft:dirt",
    "minecraft:dripstone_block",
    "minecraft:emerald_block",
    "minecraft:end_stone",
    "minecraft:end_stone_bricks",
    "minecraft:glowstone",
    "minecraft:gold_block",
    "minecraft:granite",
    "minecraft:honeycomb_block",
    "minecraft:iron_block",
    "minecraft:lapis_block",
    "minecraft:moss_block",
    "minecraft:mud",
    "minecraft:nether_bricks",
    "minecraft:netherite_block",
    "minecraft:netherrack",
    "minecraft:obsidian",
    "minecraft:ochre_froglight",
    "minecraft:packed_ice",
    "minecraft:packed_mud",
    "minecraft:pale_moss_block",
    "minecraft:pearlescent_froglight",
    "minecraft:polished_andesite",
    "minecraft:polished_blackstone",
    "minecraft:polished_blackstone_bricks",
    "minecraft:polished_deepslate",
    "minecraft:polished_diorite",
    "minecraft:polished_granite",
    "minecraft:polished_tuff",
    "minecraft:prismarine",
    "minecraft:prismarine_bricks",
    "minecraft:purpur_block",
    "minecraft:quartz_block",
    "minecraft:quartz_bricks",
    "minecraft:raw_copper_block",
    "minecraft:raw_gold_block",
    "minecraft:raw_iron_block",
    "minecraft:red_nether_bricks",
    "minecraft:red_sandstone",
    "minecraft:redstone_block",
    "minecraft:sandstone",
    "minecraft:sea_lantern",
    "minecraft:shroomlight",
    "minecraft:smooth_basalt",
    "minecraft:smooth_quartz",
    "minecraft:snow",
    "minecraft:soul_sand",
    "minecraft:soul_soil",
    "minecraft:stone",
    "minecraft:stone_bricks",
    "minecraft:tuff",
    "minecraft:verdant_froglight",
    "minecraft:waxed_copper",
    "minecraft:waxed_cut_copper",
    "minecraft:waxed_exposed_copper",
    "minecraft:waxed_exposed_cut_copper",
    "minecraft:waxed_oxidized_copper",
    "minecraft:waxed_oxidized_cut_copper",
    "minecraft:waxed_weathered_copper",
    "minecraft:waxed_weathered_cut_copper",
];

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed={PALETTE_PATH}");
    println!("cargo:rerun-if-changed={ID_MANIFEST_PATH}");

    let palette: serde_json::Value =
        serde_json::from_slice(&fs::read(PALETTE_PATH)?).map_err(invalid_data)?;
    let manifest = fs::read_to_string(ID_MANIFEST_PATH)?;
    let official_ids = official_ids(&manifest)?;
    let bytes = encode_palette(&palette, &official_ids).map_err(invalid_data)?;

    let output = PathBuf::from(
        env::var_os("OUT_DIR").ok_or_else(|| std::io::Error::other("OUT_DIR is missing"))?,
    )
    .join("bedrock-block-palette.bin");
    fs::write(output, bytes)?;
    Ok(())
}

fn official_ids(manifest: &str) -> Result<HashSet<String>, std::io::Error> {
    let commit_header = format!("# Source commit: {MANIFEST_COMMIT}");
    if !manifest.starts_with("# Bedrock 1.26.40.5 namespaced vanilla block identifiers.")
        || !manifest.lines().any(|line| line == commit_header.as_str())
    {
        return Err(invalid_data(
            "Bedrock block identifier manifest is not pinned to 1.26.40.5",
        ));
    }

    let ids = manifest
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    if ids.len() != 1_415 {
        return Err(invalid_data(
            "Bedrock block identifier manifest has an unexpected entry count",
        ));
    }
    Ok(ids)
}

fn encode_palette(
    document: &serde_json::Value,
    official_ids: &HashSet<String>,
) -> Result<Vec<u8>, String> {
    if document
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
        || document
            .get("minecraft_bedrock_version")
            .and_then(serde_json::Value::as_str)
            != Some(BEDROCK_VERSION)
    {
        return Err("palette schema or Bedrock version is not supported".to_owned());
    }

    let blocks = document
        .get("blocks")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "palette has no block table".to_owned())?;
    let count = u16::try_from(blocks.len())
        .map_err(|_| "palette has too many blocks for the binary format".to_owned())?;
    if blocks.len() < 90 {
        return Err("palette is unexpectedly small".to_owned());
    }

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"BVP1");
    bytes.extend_from_slice(&count.to_le_bytes());
    let mut seen = HashSet::with_capacity(blocks.len());
    for block in blocks {
        encode_block(block, official_ids, &mut seen, &mut bytes)?;
    }
    Ok(bytes)
}

fn encode_block(
    block: &serde_json::Value,
    official_ids: &HashSet<String>,
    seen: &mut HashSet<String>,
    bytes: &mut Vec<u8>,
) -> Result<(), String> {
    let id = block
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "palette entry has no block ID".to_owned())?;
    validate_block_id(id, official_ids, seen)?;

    for key in ["top_source", "side_source"] {
        let source = block
            .get(key)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("{id} has no {key}"))?;
        if !(source == "texture-top"
            || source == "texture-side"
            || source.starts_with("texture-alias:"))
        {
            return Err(format!("{id} has a non-vanilla texture color source"));
        }
    }

    let top = rgba(block.get("top"), id, "top")?;
    let side = rgba(block.get("side"), id, "side")?;
    let translucent_id = id == "minecraft:glass" || id.ends_with("_stained_glass");
    if !translucent_id && (top[3] != u8::MAX || side[3] != u8::MAX) {
        return Err(format!(
            "opaque voxel candidate {id} has transparent texture data"
        ));
    }
    let id_length = u8::try_from(id.len()).map_err(|_| format!("{id} is too long"))?;
    bytes.push(id_length);
    bytes.extend_from_slice(id.as_bytes());
    bytes.extend_from_slice(&top);
    bytes.extend_from_slice(&side);
    Ok(())
}

fn validate_block_id(
    id: &str,
    official_ids: &HashSet<String>,
    seen: &mut HashSet<String>,
) -> Result<(), String> {
    if !official_ids.contains(id) {
        return Err(format!(
            "palette contains an ID absent from the pinned Bedrock manifest: {id}"
        ));
    }
    if !is_supported_candidate(id) || is_unsupported_block(id) {
        return Err(format!(
            "palette contains an unsupported voxel candidate: {id}"
        ));
    }
    if !seen.insert(id.to_owned()) {
        return Err(format!("palette contains a duplicate block ID: {id}"));
    }
    Ok(())
}

fn is_supported_candidate(id: &str) -> bool {
    id == "minecraft:glass"
        || id.ends_with("_concrete")
        || id.ends_with("_wool")
        || (id.ends_with("_terracotta") && !id.ends_with("_glazed_terracotta"))
        || id.ends_with("_planks")
        || id.ends_with("_stained_glass")
        || SOLID_COLOR_BLOCKS.contains(&id)
}

fn is_unsupported_block(id: &str) -> bool {
    matches!(
        id,
        "minecraft:concrete"
            | "minecraft:wool"
            | "minecraft:terracotta"
            | "minecraft:planks"
            | "minecraft:stained_glass"
            | "minecraft:sand"
            | "minecraft:red_sand"
            | "minecraft:gravel"
            | "minecraft:suspicious_sand"
            | "minecraft:suspicious_gravel"
            | "minecraft:anvil"
            | "minecraft:chipped_anvil"
            | "minecraft:damaged_anvil"
            | "minecraft:dragon_egg"
    ) || id.ends_with("_concrete_powder")
        || id.ends_with("_glass_pane")
}

fn rgba(value: Option<&serde_json::Value>, id: &str, face: &str) -> Result<[u8; 4], String> {
    let channels = value
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("{id} has no {face} RGBA color"))?;
    let channels: [u8; 4] = channels
        .iter()
        .map(|channel| {
            channel
                .as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .ok_or_else(|| format!("{id} has an invalid {face} RGBA channel"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| format!("{id} has an invalid {face} RGBA channel count"))?;
    Ok(channels)
}

fn invalid_data(message: impl ToString) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.to_string())
}
