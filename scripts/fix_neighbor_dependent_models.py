from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(rel: str) -> str:
    return (ROOT / rel).read_text(encoding="utf-8")


def write(rel: str, text: str) -> None:
    (ROOT / rel).write_text(text, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, got {count}")
    return text.replace(old, new, 1)


def replace_all(text: str, old: str, new: str, expected: int, label: str) -> str:
    count = text.count(old)
    if count != expected:
        raise RuntimeError(f"{label}: expected {expected} matches, got {count}")
    return text.replace(old, new)


# -----------------------------------------------------------------------------
# bedrock-block-model: normalize persisted Bedrock connection vocabulary before
# the JE-first model resolver sees it.
# -----------------------------------------------------------------------------
rel = "crates/bedrock-block-model/src/java.rs"
text = read(rel)
text = replace_once(
    text,
    '    alias_bool(state, &mut properties, "in_wall_bit", "in_wall");\n\n',
    '    alias_bool(state, &mut properties, "in_wall_bit", "in_wall");\n    alias_neighbor_properties(state, family, &mut properties);\n\n',
    "call neighbor property normalization",
)
text = replace_all(
    text,
    '            let top = state_bool(state, "upside_down_bit").unwrap_or(false);',
    '            let top = state_top_half(state).unwrap_or(false);',
    2,
    "prefer modern half state for trapdoor and stairs",
)
text = replace_once(
    text,
    '            if let Some(half) = state_string(state, "vertical_half") {',
    '            if let Some(half) = state_string(state, "vertical_half")\n                .or_else(|| state_string(state, "half"))\n            {',
    "support modern slab half",
)
text = replace_once(
    text,
    '''fn bedrock_cardinal_direction(state: &BlockStateQuery) -> Option<&'static str> {
    state_string(state, "cardinal_direction")
        .and_then(cardinal_direction_string)
        .or_else(|| state_i64(state, "direction").and_then(cardinal_direction_0_3))
        .or_else(|| state_i64(state, "weirdo_direction").and_then(cardinal_direction_0_3))
}
''',
    '''fn bedrock_cardinal_direction(state: &BlockStateQuery) -> Option<&'static str> {
    state_string(state, "cardinal_direction")
        .and_then(cardinal_direction_string)
        .or_else(|| state_string(state, "facing").and_then(cardinal_direction_string))
        .or_else(|| state_string(state, "direction").and_then(cardinal_direction_string))
        .or_else(|| state_i64(state, "direction").and_then(cardinal_direction_0_3))
        .or_else(|| state_i64(state, "weirdo_direction").and_then(cardinal_direction_0_3))
}
''',
    "support modern string cardinal direction",
)
text = replace_once(
    text,
    '''fn alias_bool(
    state: &BlockStateQuery,
    properties: &mut BTreeMap<String, String>,
    source: &str,
    target: &str,
) {
''',
    '''fn alias_neighbor_properties(
    state: &BlockStateQuery,
    family: ModelFamily,
    properties: &mut BTreeMap<String, String>,
) {
    const DIRECTIONS: [&str; 4] = ["north", "south", "east", "west"];
    match family {
        ModelFamily::Fence | ModelFamily::Pane => {
            for direction in DIRECTIONS {
                let source = format!("connection_{direction}");
                if let Some(connected) = state_bool(state, &source) {
                    properties.insert(direction.to_owned(), connected.to_string());
                }
            }
        }
        ModelFamily::Wall => {
            for direction in DIRECTIONS {
                let source = format!("wall_connection_type_{direction}");
                if let Some(connection) = state
                    .state(&source)
                    .and_then(java_wall_connection_literal)
                {
                    properties.insert(direction.to_owned(), connection.to_owned());
                }
            }
            if let Some(up) = state_bool(state, "wall_post_bit") {
                properties.insert("up".to_owned(), up.to_string());
            }
        }
        ModelFamily::RedstoneWire => {
            if let Some(power) = state_i64(state, "redstone_signal")
                .or_else(|| state_i64(state, "power"))
            {
                properties.insert("power".to_owned(), power.clamp(0, 15).to_string());
            }
        }
        _ => {}
    }
}

fn java_wall_connection_literal(value: &BlockStateValue) -> Option<&'static str> {
    match value {
        BlockStateValue::Bool(value) => Some(if *value { "low" } else { "none" }),
        BlockStateValue::Int(value) => Some(match *value {
            0 => "none",
            2 => "tall",
            _ => "low",
        }),
        BlockStateValue::String(value) => match value
            .trim()
            .strip_prefix("minecraft:")
            .unwrap_or(value.trim())
        {
            "none" | "false" | "0" => Some("none"),
            "tall" | "high" | "2" => Some("tall"),
            "short" | "low" | "true" | "1" => Some("low"),
            _ => None,
        },
    }
}

fn state_top_half(state: &BlockStateQuery) -> Option<bool> {
    state_string(state, "vertical_half")
        .or_else(|| state_string(state, "half"))
        .and_then(|value| {
            match value
                .trim()
                .strip_prefix("minecraft:")
                .unwrap_or(value.trim())
            {
                "top" | "upper" => Some(true),
                "bottom" | "lower" => Some(false),
                _ => None,
            }
        })
        .or_else(|| state_bool(state, "upside_down_bit"))
}

fn alias_bool(
    state: &BlockStateQuery,
    properties: &mut BTreeMap<String, String>,
    source: &str,
    target: &str,
) {
''',
    "add neighbor normalization helpers",
)
text = replace_once(
    text,
    '''    #[test]
    fn java_variant_selector_matches_property_sets() {
''',
    '''    #[test]
    fn fence_connections_are_normalized_for_java_multipart() {
        let state = BlockStateQuery::new("minecraft:oak_fence")
            .with_state("minecraft:connection_north", true)
            .with_state("minecraft:connection_south", false)
            .with_state("minecraft:connection_east", true)
            .with_state("minecraft:connection_west", false);
        let properties = java_properties_for_bedrock_state(&state);
        assert_eq!(properties.get("north").map(String::as_str), Some("true"));
        assert_eq!(properties.get("south").map(String::as_str), Some("false"));
        assert_eq!(properties.get("east").map(String::as_str), Some("true"));
        assert_eq!(properties.get("west").map(String::as_str), Some("false"));
    }

    #[test]
    fn wall_connections_are_normalized_to_java_low_tall_and_up() {
        let state = BlockStateQuery::new("minecraft:cobblestone_wall")
            .with_state("wall_connection_type_north", "short")
            .with_state("wall_connection_type_south", "tall")
            .with_state("wall_connection_type_east", "none")
            .with_state("wall_connection_type_west", "short")
            .with_state("wall_post_bit", false);
        let properties = java_properties_for_bedrock_state(&state);
        assert_eq!(properties.get("north").map(String::as_str), Some("low"));
        assert_eq!(properties.get("south").map(String::as_str), Some("tall"));
        assert_eq!(properties.get("east").map(String::as_str), Some("none"));
        assert_eq!(properties.get("west").map(String::as_str), Some("low"));
        assert_eq!(properties.get("up").map(String::as_str), Some("false"));
    }

    #[test]
    fn redstone_signal_is_normalized_to_java_power() {
        let state = BlockStateQuery::new("minecraft:redstone_wire")
            .with_state("redstone_signal", 11)
            .with_state("north", "side")
            .with_state("south", "none")
            .with_state("east", "up")
            .with_state("west", "none");
        let properties = java_properties_for_bedrock_state(&state);
        assert_eq!(properties.get("power").map(String::as_str), Some("11"));
        assert_eq!(properties.get("north").map(String::as_str), Some("side"));
        assert_eq!(properties.get("east").map(String::as_str), Some("up"));
    }

    #[test]
    fn modern_half_state_overrides_legacy_bit() {
        let state = BlockStateQuery::new("minecraft:oak_stairs")
            .with_state("facing", "east")
            .with_state("vertical_half", "top")
            .with_state("upside_down_bit", false);
        let properties = java_properties_for_bedrock_state(&state);
        assert_eq!(properties.get("facing").map(String::as_str), Some("east"));
        assert_eq!(properties.get("half").map(String::as_str), Some("top"));
    }

    #[test]
    fn java_variant_selector_matches_property_sets() {
''',
    "add connection normalization tests",
)
write(rel, text)


# -----------------------------------------------------------------------------
# Hand-written chest fallback: use BlockEntity-derived pair direction when the
# preview injects it. Each half reaches the shared seam instead of rendering two
# isolated single chests.
# -----------------------------------------------------------------------------
rel = "crates/bedrock-block-model/src/model_family/utility/containers.rs"
text = read(rel)
text = replace_once(
    text,
    'use crate::model_family::direction::{CardinalDirection, cardinal_direction, state_i64};',
    'use crate::model_family::direction::{\n    CardinalDirection, cardinal_direction, state_i64, state_string,\n};',
    "import chest pair state parser",
)
text = replace_once(
    text,
    '''fn chest_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let front_face = block_face_from_direction(direction);
    let back_face = opposite_block_face(front_face);
    let (left_face, right_face) = left_right_faces(direction);
    let mut cuboids = vec![
        chest_box_uv(ModelCuboid::new(
            [0.0625, 0.0, 0.0625],
            [0.9375, 0.625, 0.9375],
        ))
''',
    '''fn chest_shape(state: &BlockStateQuery) -> ModelShape {
    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);
    let front_face = block_face_from_direction(direction);
    let back_face = opposite_block_face(front_face);
    let (left_face, right_face) = left_right_faces(direction);
    let pair_direction = state_string(state, "pair_direction").and_then(cardinal_from_string);
    let (body_min, body_max) = chest_pair_bounds(pair_direction);
    let mut cuboids = vec![
        chest_box_uv(ModelCuboid::new(
            body_min,
            [body_max[0], 0.625, body_max[2]],
        ))
''',
    "apply double chest body bounds",
)
text = replace_once(
    text,
    '''        chest_lid_uv(ModelCuboid::new(
            [0.0625, 0.625, 0.0625],
            [0.9375, 0.875, 0.9375],
        ))
''',
    '''        chest_lid_uv(ModelCuboid::new(
            [body_min[0], 0.625, body_min[2]],
            [body_max[0], 0.875, body_max[2]],
        ))
''',
    "apply double chest lid bounds",
)
text = replace_once(
    text,
    '''fn block_face_from_direction(direction: CardinalDirection) -> BlockFace {
''',
    '''fn chest_pair_bounds(pair_direction: Option<CardinalDirection>) -> ([f32; 3], [f32; 3]) {
    let mut min = [0.0625, 0.0, 0.0625];
    let mut max = [0.9375, 1.0, 0.9375];
    match pair_direction {
        Some(CardinalDirection::North) => min[2] = 0.0,
        Some(CardinalDirection::South) => max[2] = 1.0,
        Some(CardinalDirection::East) => max[0] = 1.0,
        Some(CardinalDirection::West) => min[0] = 0.0,
        None => {}
    }
    (min, max)
}

fn cardinal_from_string(value: &str) -> Option<CardinalDirection> {
    match value.trim().strip_prefix("minecraft:").unwrap_or(value.trim()) {
        "north" => Some(CardinalDirection::North),
        "south" => Some(CardinalDirection::South),
        "east" => Some(CardinalDirection::East),
        "west" => Some(CardinalDirection::West),
        _ => None,
    }
}

fn block_face_from_direction(direction: CardinalDirection) -> BlockFace {
''',
    "add chest pair direction helpers",
)
write(rel, text)


# -----------------------------------------------------------------------------
# 3D preview: sparse neighbor indices, dynamic redstone/fence/wall/pane/stair
# state inference, and block-entity state enrichment in the same chunk load.
# -----------------------------------------------------------------------------
rel = "src/ui/window/map_viewer/preview_3d_source.rs"
text = read(rel)
text = replace_once(
    text,
    '    ModelCuboid, ModelPlane, ModelShape, ModelWarning, block_export_material_name_for_block,',
    '    ModelCuboid, ModelFamily, ModelPlane, ModelShape, ModelWarning,\n    block_export_material_name_for_block,',
    "import ModelFamily",
)
text = replace_once(
    text,
    '    model_family_has_detail_shape, model_shape_for_block_state,',
    '    model_family_for_block_name, model_family_has_detail_shape, model_shape_for_block_state,',
    "import model family resolver",
)
text = replace_once(
    text,
    '''#[derive(Clone, Debug)]
struct Preview3dDetailBlock {
    key: BlockKey,
    normalized_name: Arc<str>,
    inferred_connections: bool,
    shape: Preview3dDetailShape,
    color: [f32; 4],
    material: Preview3dMaterialName,
}
''',
    '''#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Preview3dNeighborModel {
    None,
    Pane,
    Fence,
    Wall,
    RedstoneWire { power: u8 },
    Stairs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Preview3dConnectionKind {
    Pane,
    Fence,
    FenceGate,
    Wall,
    RedstoneWire,
    RedstoneAny,
    RedstoneAxisX,
    RedstoneAxisZ,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Preview3dStairState {
    facing: Preview3dCardinalDirection,
    top: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Preview3dBlockEntityModelState {
    chest_pair_direction: Option<Preview3dCardinalDirection>,
    chest_pair_lead: bool,
}

#[derive(Clone, Debug)]
struct Preview3dDetailBlock {
    key: BlockKey,
    normalized_name: Arc<str>,
    neighbor_model: Preview3dNeighborModel,
    stair_state: Option<Preview3dStairState>,
    shape: Preview3dDetailShape,
    color: [f32; 4],
    material: Preview3dMaterialName,
}
''',
    "add neighbor model metadata",
)
text = replace_once(
    text,
    '''    detail_connectors: HashSet<BlockKey>,
    opaque_blocks: Vec<Preview3dBlockRecord>,
''',
    '''    connection_kinds: HashMap<BlockKey, Preview3dConnectionKind>,
    stair_states: HashMap<BlockKey, Preview3dStairState>,
    opaque_blocks: Vec<Preview3dBlockRecord>,
''',
    "replace pane-only connector index",
)
text = replace_once(
    text,
    '''            detail_connectors: HashSet::default(),
            opaque_blocks: Vec::new(),
''',
    '''            connection_kinds: HashMap::default(),
            stair_states: HashMap::default(),
            opaque_blocks: Vec::new(),
''',
    "initialize neighbor indexes",
)
text = replace_once(
    text,
    '''impl Preview3dChunkBlocks {
    fn rebuild_detail_connectors(&mut self) {
        self.detail_connectors.clear();
        for block in self
            .detail_blocks
            .iter()
            .chain(self.glass_detail_blocks.iter())
        {
            if preview_3d_detail_block_connects_to_panes(block.normalized_name.as_ref()) {
                self.detail_connectors.insert(block.key);
            }
        }
    }
''',
    '''impl Preview3dChunkBlocks {
    fn rebuild_neighbor_indexes(&mut self) {
        self.stair_states.clear();
        for block in self
            .detail_blocks
            .iter()
            .chain(self.glass_detail_blocks.iter())
        {
            if let Some(state) = block.stair_state {
                self.stair_states.insert(block.key, state);
            }
        }
    }
''',
    "generalize detail neighbor indexes",
)
text = replace_once(
    text,
    '''    fn detail_connector_at(&self, block: BlockKey) -> bool {
        self.detail_connectors.contains(&block)
    }
''',
    '''    fn connection_kind_at(&self, block: BlockKey) -> Option<Preview3dConnectionKind> {
        self.connection_kinds.get(&block).copied()
    }

    fn stair_state_at(&self, block: BlockKey) -> Option<Preview3dStairState> {
        self.stair_states.get(&block).copied()
    }
''',
    "replace pane connector accessor",
)
text = text.replace("rebuild_detail_connectors", "rebuild_neighbor_indexes")
text = replace_once(
    text,
    '''        data_request: ChunkDataRequest::new()
            .full_3d_indices()
            .biome(BiomeDataRequirement::All),
''',
    '''        data_request: ChunkDataRequest::new()
            .full_3d_indices()
            .biome(BiomeDataRequirement::All)
            .block_entities(),
''',
    "load block entities with 3d chunks",
)
text = replace_once(
    text,
    '''    let mut opaque_blocks = Vec::<Preview3dBlockRecord>::with_capacity(initial_block_capacity);
''',
    '''    let mut connection_kinds = HashMap::<BlockKey, Preview3dConnectionKind>::default();
    let block_entity_states = preview_3d_block_entity_model_states(chunk);
    let mut opaque_blocks = Vec::<Preview3dBlockRecord>::with_capacity(initial_block_capacity);
''',
    "allocate sparse neighbor and block entity indexes",
)
# Three world-state calls share the same argument sequence.
text = replace_all(
    text,
    '''                                biome,
                                block_models,
                                &palette,
                                &mut block_budget,
                                &mut occupied,
''',
    '''                                biome,
                                block_entity_states.get(&key).copied(),
                                block_models,
                                &palette,
                                &mut block_budget,
                                &mut connection_kinds,
                                &mut occupied,
''',
    3,
    "pass world neighbor metadata into collector",
)
text = replace_once(
    text,
    '''        detail_connectors: HashSet::default(),
        opaque_blocks,
''',
    '''        connection_kinds,
        stair_states: HashMap::default(),
        opaque_blocks,
''',
    "store sparse neighbor indexes",
)
text = replace_once(
    text,
    '''        biome: Option<Preview3dBiomeSample>,
    block_models: Option<&BlockModelRepository>,
    palette: &RenderPalette,
    block_budget: &mut Preview3dBlockBudget,
    occupied: &mut HashSet<BlockKey>,
''',
    '''    biome: Option<Preview3dBiomeSample>,
    block_entity_state: Option<Preview3dBlockEntityModelState>,
    block_models: Option<&BlockModelRepository>,
    palette: &RenderPalette,
    block_budget: &mut Preview3dBlockBudget,
    connection_kinds: &mut HashMap<BlockKey, Preview3dConnectionKind>,
    occupied: &mut HashSet<BlockKey>,
''',
    "extend collected block parameters",
)
text = replace_once(
    text,
    '''    let normalized_name = Arc::<str>::from(preview_3d_normalized_block_name(&state.name));
    let inferred_connections = preview_3d_should_infer_detail_connections(state);
    let resolved_shape = block_models
''',
    '''    let normalized_name = Arc::<str>::from(preview_3d_normalized_block_name(&state.name));
    let neighbor_model = preview_3d_neighbor_model(state);
    let stair_state = preview_3d_stair_state(state);
    if let Some(kind) = preview_3d_connection_kind(state) {
        connection_kinds.insert(key, kind);
    }
    let resolved_shape = block_models
''',
    "capture neighbor model metadata",
)
text = replace_all(
    text,
    'resolved_shape.or_else(|| preview_3d_detail_shape_for_block(state))',
    'resolved_shape.or_else(|| preview_3d_detail_shape_for_block(state, block_entity_state))',
    3,
    "enrich fallback model with block entity state",
)
text = replace_all(
    text,
    '''                        inferred_connections,
                        shape,
''',
    '''                        neighbor_model,
                        stair_state,
                        shape,
''',
    2,
    "add neighbor metadata to opaque/glass detail blocks",
)
text = replace_once(
    text,
    '''                inferred_connections,
                shape,
''',
    '''                neighbor_model,
                stair_state,
                shape,
''',
    "add neighbor metadata to generic detail block",
)
# Structure preview has no BlockEntity payload but uses the same neighbor indexes.
text = replace_once(
    text,
    '''        None,
        None,
        render_palette,
        &mut builder.block_budget,
        &mut builder.blocks.occupied,
''',
    '''        None,
        None,
        None,
        render_palette,
        &mut builder.block_budget,
        &mut builder.blocks.connection_kinds,
        &mut builder.blocks.occupied,
''',
    "pass structure neighbor index",
)
# Replace the pane-only model inference block with all neighbor-derived families.
text = replace_once(
    text,
    '''    fn preview_3d_inferred_detail_shape(
        &self,
        block: &Preview3dDetailBlock,
    ) -> Option<Preview3dDetailShape> {
        if !block.inferred_connections
            || !preview_3d_is_pane_like_block(block.normalized_name.as_ref())
        {
            return None;
        }
        let block_name = if block.normalized_name.starts_with("minecraft:") {
            block.normalized_name.to_string()
        } else {
            format!("minecraft:{}", block.normalized_name)
        };
        let mut query = BlockStateQuery::new(block_name);
        for direction in Preview3dCardinalDirection::ALL {
            if self.preview_3d_pane_neighbor_connects(block.key, direction) {
                query = query.with_state(direction.state_key(), true);
            }
        }
        model_shape_for_block_state(&query).map(preview_3d_detail_shape_from_model_shape)
    }

    fn preview_3d_pane_neighbor_connects(
        &self,
        block: BlockKey,
        direction: Preview3dCardinalDirection,
    ) -> bool {
        let neighbor = block.cardinal_neighbor(direction);
        if matches!(
            self.block_class_at(neighbor),
            Some(Preview3dBlockClass::Opaque | Preview3dBlockClass::TransparentGlass)
        ) {
            return true;
        }
        self.block_chunks
            .get(&ChunkKey::from_block(neighbor))
            .is_some_and(|chunk| chunk.detail_connector_at(neighbor))
    }
''',
    '''    fn preview_3d_inferred_detail_shape(
        &self,
        block: &Preview3dDetailBlock,
    ) -> Option<Preview3dDetailShape> {
        if block.neighbor_model == Preview3dNeighborModel::None {
            return None;
        }
        let block_name = if block.normalized_name.starts_with("minecraft:") {
            block.normalized_name.to_string()
        } else {
            format!("minecraft:{}", block.normalized_name)
        };
        let mut query = BlockStateQuery::new(block_name);
        match block.neighbor_model {
            Preview3dNeighborModel::None => return None,
            Preview3dNeighborModel::Pane => {
                for direction in Preview3dCardinalDirection::ALL {
                    query = query.with_state(
                        direction.state_key(),
                        self.preview_3d_pane_neighbor_connects(block.key, direction),
                    );
                }
            }
            Preview3dNeighborModel::Fence => {
                for direction in Preview3dCardinalDirection::ALL {
                    query = query.with_state(
                        direction.state_key(),
                        self.preview_3d_fence_neighbor_connects(block.key, direction),
                    );
                }
            }
            Preview3dNeighborModel::Wall => {
                let mut connected = Vec::with_capacity(4);
                for direction in Preview3dCardinalDirection::ALL {
                    let connects = self.preview_3d_wall_neighbor_connects(block.key, direction);
                    if connects {
                        connected.push(direction);
                    }
                    query = query.with_state(
                        direction.state_key(),
                        if connects { "low" } else { "none" },
                    );
                }
                let straight = connected.len() == 2
                    && connected[0].opposite() == connected[1]
                    && self.block_class_at(block.above()) != Some(Preview3dBlockClass::Opaque);
                query = query.with_state("up", !straight);
            }
            Preview3dNeighborModel::RedstoneWire { power } => {
                query = query.with_state("redstone_signal", i32::from(power));
                query = query.with_state("power", i32::from(power));
                for direction in Preview3dCardinalDirection::ALL {
                    query = query.with_state(
                        direction.state_key(),
                        self.preview_3d_redstone_connection(block.key, direction),
                    );
                }
            }
            Preview3dNeighborModel::Stairs => {
                let current = block.stair_state?;
                query = query
                    .with_state("facing", current.facing.as_str())
                    .with_state("vertical_half", if current.top { "top" } else { "bottom" })
                    .with_state("half", if current.top { "top" } else { "bottom" })
                    .with_state("shape", self.preview_3d_stair_shape(block.key, current));
            }
        }
        model_shape_for_block_state(&query).map(preview_3d_detail_shape_from_model_shape)
    }

    fn preview_3d_connection_kind_at(&self, block: BlockKey) -> Option<Preview3dConnectionKind> {
        self.block_chunks
            .get(&ChunkKey::from_block(block))
            .and_then(|chunk| chunk.connection_kind_at(block))
    }

    fn preview_3d_pane_neighbor_connects(
        &self,
        block: BlockKey,
        direction: Preview3dCardinalDirection,
    ) -> bool {
        let neighbor = block.cardinal_neighbor(direction);
        matches!(
            self.block_class_at(neighbor),
            Some(Preview3dBlockClass::Opaque | Preview3dBlockClass::TransparentGlass)
        ) || self.preview_3d_connection_kind_at(neighbor) == Some(Preview3dConnectionKind::Pane)
    }

    fn preview_3d_fence_neighbor_connects(
        &self,
        block: BlockKey,
        direction: Preview3dCardinalDirection,
    ) -> bool {
        let neighbor = block.cardinal_neighbor(direction);
        self.block_class_at(neighbor) == Some(Preview3dBlockClass::Opaque)
            || matches!(
                self.preview_3d_connection_kind_at(neighbor),
                Some(
                    Preview3dConnectionKind::Fence
                        | Preview3dConnectionKind::FenceGate
                        | Preview3dConnectionKind::Wall
                )
            )
    }

    fn preview_3d_wall_neighbor_connects(
        &self,
        block: BlockKey,
        direction: Preview3dCardinalDirection,
    ) -> bool {
        let neighbor = block.cardinal_neighbor(direction);
        self.block_class_at(neighbor) == Some(Preview3dBlockClass::Opaque)
            || matches!(
                self.preview_3d_connection_kind_at(neighbor),
                Some(Preview3dConnectionKind::Wall | Preview3dConnectionKind::FenceGate)
            )
    }

    fn preview_3d_redstone_connection(
        &self,
        block: BlockKey,
        direction: Preview3dCardinalDirection,
    ) -> &'static str {
        let neighbor = block.cardinal_neighbor(direction);
        if self.preview_3d_redstone_component_connects(neighbor, direction) {
            return "side";
        }
        if self.block_class_at(neighbor) == Some(Preview3dBlockClass::Opaque)
            && self.block_class_at(block.above()) != Some(Preview3dBlockClass::Opaque)
            && self.preview_3d_connection_kind_at(neighbor.above())
                == Some(Preview3dConnectionKind::RedstoneWire)
        {
            return "up";
        }
        if self.block_class_at(neighbor) != Some(Preview3dBlockClass::Opaque)
            && self.preview_3d_connection_kind_at(neighbor.below())
                == Some(Preview3dConnectionKind::RedstoneWire)
        {
            return "side";
        }
        "none"
    }

    fn preview_3d_redstone_component_connects(
        &self,
        block: BlockKey,
        direction: Preview3dCardinalDirection,
    ) -> bool {
        match self.preview_3d_connection_kind_at(block) {
            Some(Preview3dConnectionKind::RedstoneWire | Preview3dConnectionKind::RedstoneAny) => {
                true
            }
            Some(Preview3dConnectionKind::RedstoneAxisX) => {
                matches!(direction, Preview3dCardinalDirection::East | Preview3dCardinalDirection::West)
            }
            Some(Preview3dConnectionKind::RedstoneAxisZ) => {
                matches!(direction, Preview3dCardinalDirection::North | Preview3dCardinalDirection::South)
            }
            _ => false,
        }
    }

    fn preview_3d_stair_shape(
        &self,
        block: BlockKey,
        current: Preview3dStairState,
    ) -> &'static str {
        let front = block.cardinal_neighbor(current.facing);
        if let Some(neighbor) = self.preview_3d_stair_state_at(front)
            && neighbor.top == current.top
            && current.facing.is_perpendicular(neighbor.facing)
            && self.preview_3d_stair_can_take_shape(block, current, neighbor.facing.opposite())
        {
            return if neighbor.facing == current.facing.counter_clockwise() {
                "outer_left"
            } else {
                "outer_right"
            };
        }
        let back = block.cardinal_neighbor(current.facing.opposite());
        if let Some(neighbor) = self.preview_3d_stair_state_at(back)
            && neighbor.top == current.top
            && current.facing.is_perpendicular(neighbor.facing)
            && self.preview_3d_stair_can_take_shape(block, current, neighbor.facing)
        {
            return if neighbor.facing == current.facing.counter_clockwise() {
                "inner_left"
            } else {
                "inner_right"
            };
        }
        "straight"
    }

    fn preview_3d_stair_can_take_shape(
        &self,
        block: BlockKey,
        current: Preview3dStairState,
        direction: Preview3dCardinalDirection,
    ) -> bool {
        self.preview_3d_stair_state_at(block.cardinal_neighbor(direction))
            .is_none_or(|neighbor| neighbor.facing != current.facing || neighbor.top != current.top)
    }

    fn preview_3d_stair_state_at(&self, block: BlockKey) -> Option<Preview3dStairState> {
        self.block_chunks
            .get(&ChunkKey::from_block(block))
            .and_then(|chunk| chunk.stair_state_at(block))
    }
''',
    "replace pane-only inference with neighbor-dependent families",
)
text = replace_once(
    text,
    '''    const fn cardinal_neighbor(self, direction: Preview3dCardinalDirection) -> Self {
        let [x, y, z] = direction.normal();
        Self {
            x: self.x + x,
            y: self.y + y,
            z: self.z + z,
        }
    }
''',
    '''    const fn cardinal_neighbor(self, direction: Preview3dCardinalDirection) -> Self {
        let [x, y, z] = direction.normal();
        Self {
            x: self.x + x,
            y: self.y + y,
            z: self.z + z,
        }
    }

    const fn above(self) -> Self {
        Self {
            x: self.x,
            y: self.y + 1,
            z: self.z,
        }
    }

    const fn below(self) -> Self {
        Self {
            x: self.x,
            y: self.y - 1,
            z: self.z,
        }
    }
''',
    "add vertical neighbor helpers",
)
text = replace_once(
    text,
    '''    const fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::East => Self::West,
            Self::West => Self::East,
        }
    }

    const fn normal(self) -> [i32; 3] {
''',
    '''    const fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::East => Self::West,
            Self::West => Self::East,
        }
    }

    const fn counter_clockwise(self) -> Self {
        match self {
            Self::North => Self::West,
            Self::West => Self::South,
            Self::South => Self::East,
            Self::East => Self::North,
        }
    }

    const fn is_perpendicular(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::North | Self::South, Self::East | Self::West)
                | (Self::East | Self::West, Self::North | Self::South)
        )
    }

    const fn as_str(self) -> &'static str {
        self.state_key()
    }

    const fn normal(self) -> [i32; 3] {
''',
    "add cardinal operations",
)
# Replace pane-only inference classifier and the now-unused connector helper.
text = replace_once(
    text,
    '''fn preview_3d_should_infer_detail_connections(state: &BlockState) -> bool {
    let normalized = preview_3d_normalized_block_name(&state.name);
    preview_3d_is_pane_like_block(&normalized) && !preview_3d_has_direction_connection_state(state)
}
''',
    '''fn preview_3d_neighbor_model(state: &BlockState) -> Preview3dNeighborModel {
    let family = model_family_for_block_name(&state.name);
    match family {
        ModelFamily::Pane if !preview_3d_has_direction_connection_state(state) => {
            Preview3dNeighborModel::Pane
        }
        ModelFamily::Fence if !preview_3d_has_direction_connection_state(state) => {
            Preview3dNeighborModel::Fence
        }
        ModelFamily::Wall if !preview_3d_has_direction_connection_state(state) => {
            Preview3dNeighborModel::Wall
        }
        ModelFamily::RedstoneWire => Preview3dNeighborModel::RedstoneWire {
            power: preview_3d_state_i32(state, "redstone_signal")
                .or_else(|| preview_3d_state_i32(state, "power"))
                .unwrap_or(0)
                .clamp(0, 15) as u8,
        },
        ModelFamily::Stairs => Preview3dNeighborModel::Stairs,
        _ => Preview3dNeighborModel::None,
    }
}

fn preview_3d_connection_kind(state: &BlockState) -> Option<Preview3dConnectionKind> {
    let family = model_family_for_block_name(&state.name);
    match family {
        ModelFamily::Pane => Some(Preview3dConnectionKind::Pane),
        ModelFamily::Fence => Some(Preview3dConnectionKind::Fence),
        ModelFamily::FenceGate => Some(Preview3dConnectionKind::FenceGate),
        ModelFamily::Wall => Some(Preview3dConnectionKind::Wall),
        ModelFamily::RedstoneWire => Some(Preview3dConnectionKind::RedstoneWire),
        ModelFamily::Button | ModelFamily::PressurePlate => Some(Preview3dConnectionKind::RedstoneAny),
        ModelFamily::RedstoneDevice => preview_3d_redstone_device_connection_kind(state),
        _ => {
            let normalized = preview_3d_normalized_block_name(&state.name);
            matches!(
                normalized.as_str(),
                "redstone_block"
                    | "target"
                    | "lever"
                    | "redstone_torch"
                    | "unlit_redstone_torch"
                    | "daylight_detector"
                    | "daylight_detector_inverted"
            )
            .then_some(Preview3dConnectionKind::RedstoneAny)
        }
    }
}

fn preview_3d_redstone_device_connection_kind(
    state: &BlockState,
) -> Option<Preview3dConnectionKind> {
    let normalized = preview_3d_normalized_block_name(&state.name);
    if normalized.contains("repeater") || normalized.contains("comparator") {
        return preview_3d_cardinal_direction(state).map(|direction| match direction {
            Preview3dCardinalDirection::East | Preview3dCardinalDirection::West => {
                Preview3dConnectionKind::RedstoneAxisX
            }
            Preview3dCardinalDirection::North | Preview3dCardinalDirection::South => {
                Preview3dConnectionKind::RedstoneAxisZ
            }
        });
    }
    Some(Preview3dConnectionKind::RedstoneAny)
}

fn preview_3d_stair_state(state: &BlockState) -> Option<Preview3dStairState> {
    if model_family_for_block_name(&state.name) != ModelFamily::Stairs {
        return None;
    }
    let facing = preview_3d_stair_direction(state)?;
    let top = preview_3d_state_string(state, "vertical_half")
        .or_else(|| preview_3d_state_string(state, "half"))
        .and_then(|value| match value.trim().strip_prefix("minecraft:").unwrap_or(value.trim()) {
            "top" | "upper" => Some(true),
            "bottom" | "lower" => Some(false),
            _ => None,
        })
        .or_else(|| preview_3d_state_bool(state, "upside_down_bit"))
        .unwrap_or(false);
    Some(Preview3dStairState { facing, top })
}

fn preview_3d_stair_direction(state: &BlockState) -> Option<Preview3dCardinalDirection> {
    preview_3d_state_string(state, "minecraft:cardinal_direction")
        .and_then(preview_3d_cardinal_direction_from_string)
        .or_else(|| preview_3d_state_string(state, "facing").and_then(preview_3d_cardinal_direction_from_string))
        .or_else(|| preview_3d_state_string(state, "direction").and_then(preview_3d_cardinal_direction_from_string))
        .or_else(|| preview_3d_state_i32(state, "weirdo_direction").and_then(preview_3d_stair_direction_from_int))
        .or_else(|| preview_3d_state_i32(state, "direction").and_then(preview_3d_cardinal_direction_from_int))
}
''',
    "generalize neighbor model classifier",
)
text = replace_once(
    text,
    '''fn preview_3d_detail_block_connects_to_panes(normalized: &str) -> bool {
    preview_3d_is_pane_like_block(normalized) || preview_3d_is_glass_block(normalized)
}

''',
    '',
    "remove obsolete pane connector helper",
)
text = replace_once(
    text,
    '''fn preview_3d_cardinal_direction_from_int(value: i32) -> Option<Preview3dCardinalDirection> {
    match value.rem_euclid(4) {
        0 => Some(Preview3dCardinalDirection::South),
        1 => Some(Preview3dCardinalDirection::West),
        2 => Some(Preview3dCardinalDirection::North),
        3 => Some(Preview3dCardinalDirection::East),
        _ => None,
    }
}
''',
    '''fn preview_3d_cardinal_direction_from_int(value: i32) -> Option<Preview3dCardinalDirection> {
    match value.rem_euclid(4) {
        0 => Some(Preview3dCardinalDirection::South),
        1 => Some(Preview3dCardinalDirection::West),
        2 => Some(Preview3dCardinalDirection::North),
        3 => Some(Preview3dCardinalDirection::East),
        _ => None,
    }
}

fn preview_3d_stair_direction_from_int(value: i32) -> Option<Preview3dCardinalDirection> {
    match value.rem_euclid(4) {
        0 => Some(Preview3dCardinalDirection::East),
        1 => Some(Preview3dCardinalDirection::West),
        2 => Some(Preview3dCardinalDirection::South),
        3 => Some(Preview3dCardinalDirection::North),
        _ => None,
    }
}
''',
    "add preview stairs integer mapping",
)
# Enrich the handwritten fallback query with chest pairing from BlockEntity NBT.
text = replace_once(
    text,
    '''fn preview_3d_detail_shape_for_block(state: &BlockState) -> Option<Preview3dDetailShape> {
    let mut shape = model_shape_for_block_state(&preview_3d_block_state_query(state))
''',
    '''fn preview_3d_detail_shape_for_block(
    state: &BlockState,
    block_entity_state: Option<Preview3dBlockEntityModelState>,
) -> Option<Preview3dDetailShape> {
    let mut query = preview_3d_block_state_query(state);
    if let Some(block_entity_state) = block_entity_state
        && let Some(pair_direction) = block_entity_state.chest_pair_direction
    {
        query = query
            .with_state("pair_direction", pair_direction.as_str())
            .with_state("pair_lead", block_entity_state.chest_pair_lead);
    }
    let mut shape = model_shape_for_block_state(&query)
''',
    "inject block entity model state",
)
# Add block entity parsing helpers immediately before the detail fallback function.
text = replace_once(
    text,
    '''fn preview_3d_detail_shape_for_block(
    state: &BlockState,
''',
    '''fn preview_3d_block_entity_model_states(
    chunk: &ChunkData,
) -> HashMap<BlockKey, Preview3dBlockEntityModelState> {
    let mut states = HashMap::default();
    for entity in &chunk.block_entities {
        let Some([x, y, z]) = entity.position else {
            continue;
        };
        let NbtTag::Compound(nbt) = &entity.nbt else {
            continue;
        };
        let Some(pair_x) = preview_3d_nbt_i32(nbt.get("pairx")) else {
            continue;
        };
        let Some(pair_z) = preview_3d_nbt_i32(nbt.get("pairz")) else {
            continue;
        };
        let dx = pair_x.saturating_sub(x);
        let dz = pair_z.saturating_sub(z);
        let pair_direction = match (dx, dz) {
            (1, 0) => Some(Preview3dCardinalDirection::East),
            (-1, 0) => Some(Preview3dCardinalDirection::West),
            (0, 1) => Some(Preview3dCardinalDirection::South),
            (0, -1) => Some(Preview3dCardinalDirection::North),
            _ => None,
        };
        if let Some(chest_pair_direction) = pair_direction {
            states.insert(
                BlockKey { x, y, z },
                Preview3dBlockEntityModelState {
                    chest_pair_direction: Some(chest_pair_direction),
                    chest_pair_lead: preview_3d_nbt_bool(nbt.get("pairlead")).unwrap_or(false),
                },
            );
        }
    }
    states
}

fn preview_3d_nbt_i32(value: Option<&NbtTag>) -> Option<i32> {
    match value? {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn preview_3d_nbt_bool(value: Option<&NbtTag>) -> Option<bool> {
    preview_3d_nbt_i32(value).map(|value| value != 0)
}

fn preview_3d_detail_shape_for_block(
    state: &BlockState,
''',
    "add block entity model state parser",
)
write(rel, text)

print("neighbor-dependent model patch applied")
