from __future__ import annotations

from pathlib import Path
import re

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


def replace_all_checked(text: str, old: str, new: str, expected: int, label: str) -> str:
    count = text.count(old)
    if count != expected:
        raise RuntimeError(f"{label}: expected {expected} matches, got {count}")
    return text.replace(old, new)


# ---- bedrock-block-model exports -------------------------------------------------
rel = "crates/bedrock-block-model/src/model_family.rs"
text = read(rel)
text = replace_once(
    text,
    "pub use shape::{ModelCuboid, ModelPlane, ModelShape};",
    "pub use shape::{ModelCuboid, ModelPlane, ModelPlaneSidedness, ModelShape};",
    "export ModelPlaneSidedness from model_family",
)
write(rel, text)

rel = "crates/bedrock-block-model/src/bedrock_block_model.rs"
text = read(rel)
text = replace_once(
    text,
    "    ModelCuboid, ModelFamily, ModelPlane, ModelShape, canonical_block_name_for_state,",
    "    ModelCuboid, ModelFamily, ModelPlane, ModelPlaneSidedness, ModelShape, canonical_block_name_for_state,",
    "export ModelPlaneSidedness from crate root",
)
write(rel, text)

# Java packed faces are physical model faces, not billboard/decal planes.
rel = "crates/bedrock-block-model/src/java_runtime.rs"
text = read(rel)
text = replace_once(
    text,
    "            ModelPlane::new(corners, nearest_axis_normal(normal))\n                .with_material_slot(face.material_slot)",
    "            ModelPlane::new(corners, nearest_axis_normal(normal))\n                .front_only()\n                .with_material_slot(face.material_slot)",
    "mark Java packed model faces front-only",
)
write(rel, text)

# ---- modern Bedrock state mapping ------------------------------------------------
rel = "crates/bedrock-block-model/src/java.rs"
text = read(rel)
text = replace_all_checked(
    text,
    "            let top = state_bool(state, \"upside_down_bit\").unwrap_or(false);",
    "            let top = state_top_half(state).unwrap_or(false);",
    2,
    "prefer modern half state for trapdoors and stairs",
)
text = replace_once(
    text,
    "            if let Some(half) = state_string(state, \"vertical_half\") {",
    "            if let Some(half) = state_string(state, \"vertical_half\")\n                .or_else(|| state_string(state, \"half\"))\n            {",
    "support modern slab half alias",
)
text = replace_once(
    text,
    "fn bedrock_cardinal_direction(state: &BlockStateQuery) -> Option<&'static str> {\n    state_string(state, \"cardinal_direction\")\n        .and_then(cardinal_direction_string)\n        .or_else(|| state_i64(state, \"direction\").and_then(cardinal_direction_0_3))\n        .or_else(|| state_i64(state, \"weirdo_direction\").and_then(cardinal_direction_0_3))\n}",
    "fn bedrock_cardinal_direction(state: &BlockStateQuery) -> Option<&'static str> {\n    state_string(state, \"cardinal_direction\")\n        .and_then(cardinal_direction_string)\n        .or_else(|| state_string(state, \"facing\").and_then(cardinal_direction_string))\n        .or_else(|| state_string(state, \"direction\").and_then(cardinal_direction_string))\n        .or_else(|| state_i64(state, \"direction\").and_then(cardinal_direction_0_3))\n        .or_else(|| state_i64(state, \"weirdo_direction\").and_then(cardinal_direction_0_3))\n}",
    "support modern string facing in Java mapping",
)
text = replace_once(
    text,
    "fn state_i64(state: &BlockStateQuery, key: &str) -> Option<i64> {",
    "fn state_top_half(state: &BlockStateQuery) -> Option<bool> {\n    state_string(state, \"vertical_half\")\n        .or_else(|| state_string(state, \"half\"))\n        .and_then(|value| {\n            let value = value.trim().strip_prefix(\"minecraft:\").unwrap_or(value.trim());\n            match value {\n                \"top\" | \"upper\" => Some(true),\n                \"bottom\" | \"lower\" => Some(false),\n                _ => None,\n            }\n        })\n        .or_else(|| state_bool(state, \"upside_down_bit\"))\n}\n\nfn state_i64(state: &BlockStateQuery, key: &str) -> Option<i64> {",
    "add shared modern half parser",
)
text = replace_once(
    text,
    "    #[test]\n    fn java_variant_selector_matches_property_sets() {",
    "    #[test]\n    fn modern_stair_half_and_facing_override_legacy_fallbacks() {\n        let state = BlockStateQuery::new(\"minecraft:oak_stairs\")\n            .with_state(\"facing\", \"east\")\n            .with_state(\"vertical_half\", \"top\")\n            .with_state(\"upside_down_bit\", false)\n            .with_state(\"weirdo_direction\", 2);\n        let properties = java_properties_for_bedrock_state(&state);\n        assert_eq!(properties.get(\"facing\").map(String::as_str), Some(\"east\"));\n        assert_eq!(properties.get(\"half\").map(String::as_str), Some(\"top\"));\n    }\n\n    #[test]\n    fn modern_trapdoor_half_overrides_legacy_bit() {\n        let state = BlockStateQuery::new(\"minecraft:oak_trapdoor\")\n            .with_state(\"direction\", \"north\")\n            .with_state(\"vertical_half\", \"top\")\n            .with_state(\"upside_down_bit\", false);\n        let properties = java_properties_for_bedrock_state(&state);\n        assert_eq!(properties.get(\"half\").map(String::as_str), Some(\"top\"));\n    }\n\n    #[test]\n    fn java_variant_selector_matches_property_sets() {",
    "add modern Java state mapping regression tests",
)
write(rel, text)

# Hand-written stairs fallback must agree with the Java state mapping.
rel = "crates/bedrock-block-model/src/model_family/building/stairs.rs"
text = read(rel)
text = replace_once(
    text,
    "    let top = state_string(state, \"minecraft:vertical_half\")\n        .map(is_top_half)\n        .or_else(|| state_bool(state, \"upside_down_bit\"))\n        .unwrap_or(false);",
    "    let top = state_string(state, \"vertical_half\")\n        .or_else(|| state_string(state, \"half\"))\n        .map(is_top_half)\n        .or_else(|| state_bool(state, \"upside_down_bit\"))\n        .unwrap_or(false);",
    "prefer modern stairs half state",
)
if "modern_half_overrides_legacy_bit" not in text:
    text += """

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modern_half_overrides_legacy_bit() {
        let state = BlockStateQuery::new("minecraft:oak_stairs")
            .with_state("facing", "north")
            .with_state("shape", "straight")
            .with_state("vertical_half", "top")
            .with_state("upside_down_bit", false);
        let shape = shape(&state);
        assert_eq!(shape.cuboids[0].min[1], 0.5);
        assert_eq!(shape.cuboids[0].max[1], 1.0);
    }
}
"""
write(rel, text)

# ---- BMCBL 3D preview: front-only planes + O(1) stairs adjacency -----------------
rel = "src/ui/window/map_viewer/preview_3d_source.rs"
text = read(rel)
text = replace_once(
    text,
    "    ModelCuboid, ModelPlane, ModelShape, ModelWarning, block_export_material_name_for_block,",
    "    ModelCuboid, ModelPlane, ModelPlaneSidedness, ModelShape, ModelWarning,\n    block_export_material_name_for_block,",
    "import ModelPlaneSidedness",
)
text = replace_once(
    text,
    "struct Preview3dDetailBlock {\n    key: BlockKey,\n    normalized_name: Arc<str>,\n    inferred_connections: bool,\n    shape: Preview3dDetailShape,\n    color: [f32; 4],\n    material: Preview3dMaterialName,\n}\n\n#[derive(Clone, Debug, Default)]\nstruct Preview3dDetailShape {\n    cuboids: Vec<Preview3dCuboid>,\n    planes: Vec<Preview3dPlane>,\n}\n\nimpl Preview3dDetailShape {\n    fn from_cuboids(cuboids: impl Into<Vec<Preview3dCuboid>>) -> Self {\n        Self {\n            cuboids: cuboids.into(),\n            planes: Vec::new(),\n        }\n    }\n\n    fn with_planes(mut self, planes: impl Into<Vec<Preview3dPlane>>) -> Self {\n        self.planes = planes.into();\n        self\n    }\n\n    const fn is_empty(&self) -> bool {\n        self.cuboids.is_empty() && self.planes.is_empty()\n    }\n}",
    "struct Preview3dDetailBlock {\n    key: BlockKey,\n    normalized_name: Arc<str>,\n    inferred_connections: bool,\n    stair_state: Option<Preview3dStairState>,\n    infer_stair_shape: bool,\n    shape: Preview3dDetailShape,\n    color: [f32; 4],\n    material: Preview3dMaterialName,\n}\n\n#[derive(Clone, Debug, Default)]\nstruct Preview3dDetailShape {\n    cuboids: Vec<Preview3dCuboid>,\n    planes: Vec<Preview3dPlane>,\n    front_only_planes: Vec<Preview3dPlane>,\n}\n\nimpl Preview3dDetailShape {\n    fn from_cuboids(cuboids: impl Into<Vec<Preview3dCuboid>>) -> Self {\n        Self {\n            cuboids: cuboids.into(),\n            planes: Vec::new(),\n            front_only_planes: Vec::new(),\n        }\n    }\n\n    fn with_planes(mut self, planes: impl Into<Vec<Preview3dPlane>>) -> Self {\n        self.planes = planes.into();\n        self\n    }\n\n    const fn is_empty(&self) -> bool {\n        self.cuboids.is_empty() && self.planes.is_empty() && self.front_only_planes.is_empty()\n    }\n}",
    "extend detail block and detail shape metadata",
)
text = replace_once(
    text,
    "fn preview_3d_detail_shape_from_model_shape(shape: ModelShape) -> Preview3dDetailShape {\n    Preview3dDetailShape {\n        cuboids: shape\n            .cuboids\n            .into_iter()\n            .map(preview_3d_cuboid_from_model_cuboid)\n            .collect(),\n        planes: shape\n            .planes\n            .into_iter()\n            .map(preview_3d_plane_from_model_plane)\n            .collect(),\n    }\n}",
    "fn preview_3d_detail_shape_from_model_shape(shape: ModelShape) -> Preview3dDetailShape {\n    let (front_only_planes, planes): (Vec<_>, Vec<_>) = shape\n        .planes\n        .into_iter()\n        .partition(|plane| plane.sidedness == ModelPlaneSidedness::FrontOnly);\n    Preview3dDetailShape {\n        cuboids: shape\n            .cuboids\n            .into_iter()\n            .map(preview_3d_cuboid_from_model_cuboid)\n            .collect(),\n        planes: planes\n            .into_iter()\n            .map(preview_3d_plane_from_model_plane)\n            .collect(),\n        front_only_planes: front_only_planes\n            .into_iter()\n            .map(preview_3d_plane_from_model_plane)\n            .collect(),\n    }\n}",
    "partition front-only model planes",
)
text = replace_once(
    text,
    "    let inferred_connections = preview_3d_should_infer_detail_connections(state);\n    let resolved_shape = block_models\n        .and_then(|models| preview_3d_resolved_detail_shape_for_block(models, state, block_class));",
    "    let inferred_connections = preview_3d_should_infer_detail_connections(state);\n    let resolved_shape = block_models\n        .and_then(|models| preview_3d_resolved_detail_shape_for_block(models, state, block_class));\n    let stair_state = preview_3d_stair_state(state);\n    let infer_stair_shape = stair_state.is_some() && resolved_shape.is_none();",
    "capture stair metadata before shape selection",
)
pattern = re.compile(r"(?m)^(?P<indent>\s*)inferred_connections,\n(?P=indent)shape,")
matches = list(pattern.finditer(text))
if len(matches) != 3:
    raise RuntimeError(f"detail block initializers: expected 3 matches, got {len(matches)}")
text = pattern.sub(
    lambda m: f"{m.group('indent')}inferred_connections,\n{m.group('indent')}stair_state,\n{m.group('indent')}infer_stair_shape,\n{m.group('indent')}shape,",
    text,
)
text = replace_once(
    text,
    "    detail_connectors: HashSet<BlockKey>,\n    opaque_blocks: Vec<Preview3dBlockRecord>,",
    "    detail_connectors: HashSet<BlockKey>,\n    stair_states: HashMap<BlockKey, Preview3dStairState>,\n    opaque_blocks: Vec<Preview3dBlockRecord>,",
    "add per-chunk stair state index",
)
text = replace_all_checked(
    text,
    "            detail_connectors: HashSet::default(),\n            opaque_blocks:",
    "            detail_connectors: HashSet::default(),\n            stair_states: HashMap::default(),\n            opaque_blocks:",
    1,
    "initialize stair index in Default",
)
text = replace_once(
    text,
    "        detail_connectors: HashSet::default(),\n        opaque_blocks,",
    "        detail_connectors: HashSet::default(),\n        stair_states: HashMap::default(),\n        opaque_blocks,",
    "initialize stair index in collected chunk",
)
text = text.replace("rebuild_detail_connectors", "rebuild_detail_indexes")
text = replace_once(
    text,
    "    fn rebuild_detail_indexes(&mut self) {\n        self.detail_connectors.clear();\n        for block in self\n            .detail_blocks\n            .iter()\n            .chain(self.glass_detail_blocks.iter())\n        {\n            if preview_3d_detail_block_connects_to_panes(block.normalized_name.as_ref()) {\n                self.detail_connectors.insert(block.key);\n            }\n        }\n    }\n\n    fn class_at(&self, block: BlockKey) -> Option<Preview3dBlockClass> {",
    "    fn rebuild_detail_indexes(&mut self) {\n        self.detail_connectors.clear();\n        self.stair_states.clear();\n        for block in self\n            .detail_blocks\n            .iter()\n            .chain(self.glass_detail_blocks.iter())\n        {\n            if preview_3d_detail_block_connects_to_panes(block.normalized_name.as_ref()) {\n                self.detail_connectors.insert(block.key);\n            }\n            if let Some(stair_state) = block.stair_state {\n                self.stair_states.insert(block.key, stair_state);\n            }\n        }\n    }\n\n    fn class_at(&self, block: BlockKey) -> Option<Preview3dBlockClass> {",
    "rebuild detail connection and stair indexes",
)
text = replace_once(
    text,
    "    fn detail_connector_at(&self, block: BlockKey) -> bool {\n        self.detail_connectors.contains(&block)\n    }\n}",
    "    fn detail_connector_at(&self, block: BlockKey) -> bool {\n        self.detail_connectors.contains(&block)\n    }\n\n    fn stair_state_at(&self, block: BlockKey) -> Option<Preview3dStairState> {\n        self.stair_states.get(&block).copied()\n    }\n}",
    "expose O(1) stair lookup inside a chunk",
)
old_inferred = """    fn preview_3d_inferred_detail_shape(
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

"""
new_inferred = """    fn preview_3d_inferred_detail_shape(
        &self,
        block: &Preview3dDetailBlock,
    ) -> Option<Preview3dDetailShape> {
        let block_name = if block.normalized_name.starts_with("minecraft:") {
            block.normalized_name.to_string()
        } else {
            format!("minecraft:{}", block.normalized_name)
        };
        if block.infer_stair_shape {
            let stair_state = block.stair_state?;
            let stair_shape = self.preview_3d_inferred_stair_shape(block.key, stair_state);
            let query = BlockStateQuery::new(block_name)
                .with_state("facing", stair_state.facing.state_key())
                .with_state("half", if stair_state.top { "top" } else { "bottom" })
                .with_state("shape", stair_shape);
            return model_shape_for_block_state(&query)
                .map(preview_3d_detail_shape_from_model_shape);
        }
        if !block.inferred_connections
            || !preview_3d_is_pane_like_block(block.normalized_name.as_ref())
        {
            return None;
        }
        let mut query = BlockStateQuery::new(block_name);
        for direction in Preview3dCardinalDirection::ALL {
            if self.preview_3d_pane_neighbor_connects(block.key, direction) {
                query = query.with_state(direction.state_key(), true);
            }
        }
        model_shape_for_block_state(&query).map(preview_3d_detail_shape_from_model_shape)
    }

    fn preview_3d_inferred_stair_shape(
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

"""
text = replace_once(text, old_inferred, new_inferred, "add stairs adjacency inference")
text = replace_once(
    text,
    "    for plane in &shape.planes {\n        preview_3d_push_plane_face(",
    "    for plane in &shape.front_only_planes {\n        preview_3d_push_plane_face(\n            block.key,\n            plane.clone(),\n            block.color,\n            block.material.clone(),\n            faces,\n        );\n    }\n    for plane in &shape.planes {\n        preview_3d_push_plane_face(",
    "emit front-only detail planes once",
)
text = replace_once(
    text,
    "    for plane in &mut shape.planes {\n        plane.uv = Some(uv);\n    }",
    "    for plane in shape\n        .planes\n        .iter_mut()\n        .chain(shape.front_only_planes.iter_mut())\n    {\n        plane.uv = Some(uv);\n    }",
    "apply inventory UV to both plane classes",
)
text = replace_once(
    text,
    "    shape.planes.clear();\n}",
    "    shape.planes.clear();\n    shape.front_only_planes.clear();\n}",
    "clear shulker plane classes",
)
text = replace_once(
    text,
    "    shape.planes.extend(preview_3d_rotated_cuboid_planes(",
    "    shape\n        .front_only_planes\n        .extend(preview_3d_rotated_cuboid_planes(",
    "route Bedrock rotated cube faces to front-only planes",
)
text = replace_once(
    text,
    "    shape.planes.is_empty()\n        && shape.cuboids.len() == 1",
    "    shape.planes.is_empty()\n        && shape.front_only_planes.is_empty()\n        && shape.cuboids.len() == 1",
    "full cube detection accounts for front-only planes",
)
text = replace_once(
    text,
    "    const fn opposite(self) -> Self {\n        match self {\n            Self::North => Self::South,\n            Self::South => Self::North,\n            Self::East => Self::West,\n            Self::West => Self::East,\n        }\n    }\n\n    const fn normal(self) -> [i32; 3] {",
    "    const fn opposite(self) -> Self {\n        match self {\n            Self::North => Self::South,\n            Self::South => Self::North,\n            Self::East => Self::West,\n            Self::West => Self::East,\n        }\n    }\n\n    const fn counter_clockwise(self) -> Self {\n        match self {\n            Self::North => Self::West,\n            Self::West => Self::South,\n            Self::South => Self::East,\n            Self::East => Self::North,\n        }\n    }\n\n    const fn is_perpendicular(self, other: Self) -> bool {\n        matches!(\n            (self, other),\n            (Self::North | Self::South, Self::East | Self::West)\n                | (Self::East | Self::West, Self::North | Self::South)\n        )\n    }\n\n    const fn normal(self) -> [i32; 3] {",
    "add stairs direction operations",
)
text = replace_once(
    text,
    "fn preview_3d_cardinal_direction(state: &BlockState) -> Option<Preview3dCardinalDirection> {",
    "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\nstruct Preview3dStairState {\n    facing: Preview3dCardinalDirection,\n    top: bool,\n}\n\nfn preview_3d_stair_state(state: &BlockState) -> Option<Preview3dStairState> {\n    let normalized = preview_3d_normalized_block_name(&state.name);\n    if !preview_3d_is_stair_block(&normalized) {\n        return None;\n    }\n    let facing = preview_3d_cardinal_direction(state)?;\n    let top = preview_3d_state_string(state, \"vertical_half\")\n        .or_else(|| preview_3d_state_string(state, \"half\"))\n        .and_then(|value| match value.trim().strip_prefix(\"minecraft:\").unwrap_or(value.trim()) {\n            \"top\" | \"upper\" => Some(true),\n            \"bottom\" | \"lower\" => Some(false),\n            _ => None,\n        })\n        .or_else(|| preview_3d_state_bool(state, \"upside_down_bit\"))\n        .unwrap_or(false);\n    Some(Preview3dStairState { facing, top })\n}\n\nfn preview_3d_is_stair_block(normalized: &str) -> bool {\n    normalized.ends_with(\"_stairs\")\n        || matches!(normalized, \"stairs\" | \"stone_stairs\" | \"normal_stone_stairs\")\n}\n\nfn preview_3d_cardinal_direction(state: &BlockState) -> Option<Preview3dCardinalDirection> {",
    "add compact stair state extraction",
)
text = replace_once(
    text,
    "        .or_else(|| {\n            preview_3d_state_string(state, \"facing_direction\")\n                .and_then(preview_3d_cardinal_direction_from_string)\n        })\n        .or_else(|| {\n            preview_3d_block_face(state).and_then(preview_3d_cardinal_direction_from_string)\n        })",
    "        .or_else(|| {\n            preview_3d_state_string(state, \"facing_direction\")\n                .and_then(preview_3d_cardinal_direction_from_string)\n        })\n        .or_else(|| {\n            preview_3d_state_string(state, \"direction\")\n                .and_then(preview_3d_cardinal_direction_from_string)\n        })\n        .or_else(|| {\n            preview_3d_block_face(state).and_then(preview_3d_cardinal_direction_from_string)\n        })",
    "support string direction in preview state extraction",
)
write(rel, text)

print("front-only planes + stairs adjacency patch applied")
