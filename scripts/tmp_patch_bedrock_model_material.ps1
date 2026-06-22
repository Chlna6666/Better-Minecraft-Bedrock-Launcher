$modelFamilyPath = 'C:\Users\Administrator\Desktop\BE-Community-Dev\bedrock-block-model\src\model_family.rs'
$bedrockModelPath = 'C:\Users\Administrator\Desktop\BE-Community-Dev\bedrock-block-model\src\bedrock_block_model.rs'

$modelFamilyText = Get-Content -Raw -Path $modelFamilyPath

if (-not $modelFamilyText.Contains('use std::borrow::Cow;')) {
    $modelFamilyText = $modelFamilyText.Replace(
        "use crate::state::BlockStateQuery;",
        "use std::borrow::Cow;`r`n`r`nuse crate::state::{BlockStateQuery, BlockStateValue};"
    )
} elseif ($modelFamilyText.Contains("use crate::state::BlockStateQuery;")) {
    $modelFamilyText = $modelFamilyText.Replace(
        "use crate::state::BlockStateQuery;",
        "use crate::state::{BlockStateQuery, BlockStateValue};"
    )
}

$materialHelper = @"
#[must_use]
pub fn detail_material_block_name_for_state(
    state: &BlockStateQuery,
) -> Option<Cow<'static, str>> {
    let canonical_name = canonical_state_block_name(state);
    let name = canonical_name
        .strip_prefix("minecraft:")
        .unwrap_or(canonical_name.as_str());
    if name == "portal" || name == "nether_portal" {
        return Some(Cow::Borrowed("minecraft:portal"));
    }
    if name == "end_portal" {
        return Some(Cow::Borrowed("minecraft:end_portal"));
    }
    if name == "redstone_wire" {
        return Some(Cow::Borrowed("minecraft:redstone_wire"));
    }
    if name == "cobweb" || name == "web" {
        return Some(Cow::Borrowed("minecraft:web"));
    }
    if name == "snow_layer" {
        return Some(Cow::Borrowed("minecraft:snow"));
    }
    if name == "carpet" {
        return Some(Cow::Borrowed("minecraft:wool"));
    }
    if name == "iron_bars" {
        return Some(Cow::Borrowed("minecraft:iron_bars"));
    }
    if name == "chain" {
        return Some(Cow::Borrowed("minecraft:chain"));
    }
    if name == "lantern" || name == "soul_lantern" {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if name == "candle" || name.ends_with("_candle") || name.contains("_candle_") {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if matches!(name, "anvil" | "chipped_anvil" | "damaged_anvil" | "decorated_pot") {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if name == "stonecutter" || name == "stonecutter_block" {
        return Some(Cow::Borrowed("minecraft:stonecutter_block"));
    }
    if name == "flower_pot" || name.starts_with("potted_") {
        return Some(Cow::Borrowed("minecraft:flower_pot"));
    }
    if name == "shulker_box" || name.ends_with("_shulker_box") {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if name == "hopper" {
        return Some(Cow::Borrowed("minecraft:hopper"));
    }
    if name == "chest" || name.ends_with("_chest") {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if let Some(color) = name.strip_suffix("_carpet") {
        return Some(Cow::Owned(format!("minecraft:{color}_wool")));
    }
    if let Some(color) = name.strip_suffix("_stained_glass") {
        return Some(Cow::Owned(format!("minecraft:{color}_stained_glass")));
    }
    if let Some(color) = name.strip_suffix("_stained_glass_pane") {
        return Some(Cow::Owned(format!("minecraft:{color}_stained_glass_pane")));
    }
    if matches!(
        model_family_for_block_name(name),
        ModelFamily::CrossPlant | ModelFamily::Vine
    ) {
        return Some(Cow::Owned(format!("minecraft:{name}")));
    }
    if let Some(base) = name.strip_suffix("_stairs")
        && let Some(material) = canonical_stairs_material_name(base)
    {
        return Some(material);
    }
    if let Some(base) = name.strip_suffix("_fence_gate") {
        return wood_detail_material_name(base);
    }
    if let Some(base) = name.strip_suffix("_fence") {
        if name == "nether_brick_fence" {
            return Some(Cow::Borrowed("minecraft:nether_bricks"));
        }
        return wood_detail_material_name(base);
    }
    if let Some(base) = name.strip_suffix("_trapdoor") {
        return Some(Cow::Owned(format!("minecraft:{base}_trapdoor")));
    }
    if let Some(base) = name.strip_suffix("_wall") {
        return Some(canonical_wall_material_name(base));
    }
    None
}

fn canonical_state_block_name(state: &BlockStateQuery) -> String {
    let name = normalized_block_name(&state.name);
    if name == "wool" {
        let color = state_color(state).unwrap_or("white");
        return format!("minecraft:{color}_wool");
    }
    if name == "carpet" {
        let color = state_color(state).unwrap_or("white");
        return format!("minecraft:{color}_carpet");
    }
    if name == "stained_glass" {
        let color = state_color(state).unwrap_or("white");
        return format!("minecraft:{color}_stained_glass");
    }
    if name == "stained_glass_pane" {
        let color = state_color(state).unwrap_or("white");
        return format!("minecraft:{color}_stained_glass_pane");
    }
    if name == "shulker_box"
        && let Some(color) = state_color(state)
    {
        return format!("minecraft:{color}_shulker_box");
    }
    state.name.clone()
}

fn state_color(state: &BlockStateQuery) -> Option<&'static str> {
    let value = state
        .state("color")
        .and_then(block_state_value_as_string)
        .or_else(|| state.state("old_log_type").and_then(block_state_value_as_string))
        .or_else(|| state.state("wood_type").and_then(block_state_value_as_string))
        .or_else(|| state.state("stone_slab_type").and_then(block_state_value_as_string))
        .or_else(|| state.state("stone_slab_type_2").and_then(block_state_value_as_string))?;
    Some(match value {
        "silver" => "light_gray",
        "light_blue" => "light_blue",
        "cyan" => "cyan",
        "purple" => "purple",
        "blue" => "blue",
        "brown" => "brown",
        "green" => "green",
        "red" => "red",
        "black" => "black",
        "gray" => "gray",
        "pink" => "pink",
        "lime" => "lime",
        "yellow" => "yellow",
        "magenta" => "magenta",
        "orange" => "orange",
        "white" => "white",
        _ => return None,
    })
}

fn block_state_value_as_string(value: &BlockStateValue) -> Option<&str> {
    match value {
        BlockStateValue::String(value) => Some(value),
        BlockStateValue::Bool(_) | BlockStateValue::Int(_) => None,
    }
}

fn canonical_wall_material_name(base: &str) -> Cow<'static, str> {
    match base {
        "mossy_cobblestone" => Cow::Borrowed("minecraft:mossy_cobblestone"),
        "brick" => Cow::Borrowed("minecraft:bricks"),
        "stone_brick" => Cow::Borrowed("minecraft:stone_bricks"),
        "mossy_stone_brick" => Cow::Borrowed("minecraft:mossy_stone_bricks"),
        "end_brick" | "end_stone_brick" => Cow::Borrowed("minecraft:end_bricks"),
        "mud_brick" => Cow::Borrowed("minecraft:mud_bricks"),
        "nether_brick" => Cow::Borrowed("minecraft:nether_bricks"),
        "red_nether_brick" => Cow::Borrowed("minecraft:red_nether_bricks"),
        "deepslate_brick" => Cow::Borrowed("minecraft:deepslate_bricks"),
        "deepslate_tile" => Cow::Borrowed("minecraft:deepslate_tiles"),
        "polished_blackstone_brick" => {
            Cow::Borrowed("minecraft:polished_blackstone_bricks")
        }
        "prismarine_brick" => Cow::Borrowed("minecraft:prismarine_bricks"),
        "tuff_brick" => Cow::Borrowed("minecraft:tuff_bricks"),
        _ => Cow::Owned(format!("minecraft:{base}")),
    }
}

fn canonical_stairs_material_name(base: &str) -> Option<Cow<'static, str>> {
    if let Some(material) = wood_detail_material_name(base) {
        return Some(material);
    }
    Some(match base {
        "brick" => Cow::Borrowed("minecraft:bricks"),
        "stone_brick" => Cow::Borrowed("minecraft:stone_bricks"),
        "mossy_stone_brick" => Cow::Borrowed("minecraft:mossy_stone_bricks"),
        "nether_brick" => Cow::Borrowed("minecraft:nether_bricks"),
        "red_nether_brick" => Cow::Borrowed("minecraft:red_nether_bricks"),
        "end_brick" | "end_stone_brick" => Cow::Borrowed("minecraft:end_bricks"),
        "purpur" => Cow::Borrowed("minecraft:purpur_block"),
        "quartz" => Cow::Borrowed("minecraft:quartz_block"),
        "smooth_quartz" => Cow::Borrowed("minecraft:smooth_quartz"),
        _ => return None,
    })
}

fn wood_detail_material_name(base: &str) -> Option<Cow<'static, str>> {
    let material = match base {
        "oak" | "spruce" | "birch" | "jungle" | "acacia" | "dark_oak" | "mangrove"
        | "cherry" | "bamboo" | "crimson" | "warped" | "pale_oak" => {
            format!("minecraft:{base}_planks")
        }
        _ => return None,
    };
    Some(Cow::Owned(material))
}
"@

if (-not $modelFamilyText.Contains('pub fn detail_material_block_name_for_state')) {
    $modelFamilyText = $modelFamilyText.Replace(
        "fn normalized_block_name(name: &str) -> &str {",
        "$materialHelper`r`nfn normalized_block_name(name: &str) -> &str {"
    )
}

Set-Content -Path $modelFamilyPath -Value $modelFamilyText

$bedrockModelText = Get-Content -Raw -Path $bedrockModelPath
if (-not $bedrockModelText.Contains('detail_material_block_name_for_state')) {
    $bedrockModelText = $bedrockModelText.Replace(
        "    ModelCuboid, ModelFamily, ModelPlane, ModelShape, model_family_for_block_name,`r`n    model_family_has_detail_shape, model_shape_for_block_state,",
        "    ModelCuboid, ModelFamily, ModelPlane, ModelShape, detail_material_block_name_for_state,`r`n    model_family_for_block_name, model_family_has_detail_shape, model_shape_for_block_state,"
    )
}

Set-Content -Path $bedrockModelPath -Value $bedrockModelText
