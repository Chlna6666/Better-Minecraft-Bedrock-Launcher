use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::material::BlockFace;
use crate::model_family::shape::{
    ModelCuboid, ModelPlane, ModelShape, detail_cuboid_with_local_uv,
};
use crate::{
    BlockModelError, BlockStateQuery, BlockStateValue, ModelFamily, Result,
    canonical_block_name_for_state, model_family_for_block_name,
};

/// Java Edition resource-pack model repository used as an offline conversion source.
///
/// This type deliberately does not make Java assets a runtime dependency of BMCBL. Callers point
/// it at an extracted Java client/resource-pack `assets` tree, resolve the desired Bedrock state,
/// then serialize or otherwise retain the baked result they need.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JavaModelRepository {
    assets_root: PathBuf,
}

/// A Java model baked for one Bedrock block state.
#[derive(Clone, Debug, PartialEq)]
pub struct JavaBakedModel {
    pub java_block_id: String,
    pub properties: BTreeMap<String, String>,
    pub shape: ModelShape,
    pub source_models: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ResolvedJavaModel {
    textures: BTreeMap<String, String>,
    elements: Vec<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct JavaVariantApply {
    model: String,
    x: i32,
    y: i32,
    uvlock: bool,
}

impl JavaModelRepository {
    /// Creates a repository from an extracted Java resource root.
    ///
    /// Accepted paths are the directory containing `assets/`, `assets/` itself, or
    /// `assets/minecraft/` itself.
    ///
    /// # Errors
    ///
    /// Returns an error when no `assets` tree can be identified.
    pub fn from_root(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let assets_root = if root.join("assets").is_dir() {
            root.join("assets")
        } else if root.file_name().is_some_and(|name| name == "assets") && root.is_dir() {
            root.to_path_buf()
        } else if root.file_name().is_some_and(|name| name == "minecraft")
            && root
                .parent()
                .is_some_and(|parent| parent.file_name().is_some_and(|name| name == "assets"))
        {
            root.parent().expect("checked parent").to_path_buf()
        } else {
            return Err(BlockModelError::Message(format!(
                "Java model root must contain an assets directory: {}",
                root.display()
            )));
        };

        Ok(Self { assets_root })
    }

    #[must_use]
    pub fn assets_root(&self) -> &Path {
        &self.assets_root
    }

    /// Resolves one Bedrock block state through Java blockstate/model JSON and bakes it into the
    /// crate's renderer-neutral `ModelShape` representation.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON, missing referenced model files, parent cycles, or
    /// unsupported non-quarter-turn blockstate rotations.
    pub fn resolve_bedrock_state(&self, state: &BlockStateQuery) -> Result<Option<JavaBakedModel>> {
        let java_block_id = java_block_id_for_bedrock_state(state);
        let properties = java_properties_for_bedrock_state(state);
        let blockstate_path = self.blockstate_path(&java_block_id);
        if !blockstate_path.is_file() {
            return Ok(None);
        }

        let blockstate = read_json(&blockstate_path)?;
        let applies = matching_applies(&blockstate, &properties);
        if applies.is_empty() {
            return Ok(None);
        }

        let mut shape = ModelShape::default();
        let mut source_models = Vec::with_capacity(applies.len());
        let mut warnings = Vec::new();

        for apply in applies {
            let model = self.resolve_model(&apply.model, 0)?;
            let mut model_shape = model_shape_from_java(&model, &mut warnings);
            rotate_shape_quarter_turns(&mut model_shape, apply.x, apply.y)?;
            if apply.uvlock && (apply.x != 0 || apply.y != 0) {
                warnings.push(format!(
                    "{} requests uvlock; geometry rotation is baked but exact Java uvlock semantics are not yet preserved",
                    apply.model
                ));
            }
            shape.cuboids.extend(model_shape.cuboids);
            shape.planes.extend(model_shape.planes);
            source_models.push(apply.model);
        }

        Ok(Some(JavaBakedModel {
            java_block_id,
            properties,
            shape,
            source_models,
            warnings,
        }))
    }

    fn blockstate_path(&self, id: &str) -> PathBuf {
        let (namespace, path) = split_resource_id(id);
        self.assets_root
            .join(namespace)
            .join("blockstates")
            .join(format!("{path}.json"))
    }

    fn model_path(&self, id: &str) -> PathBuf {
        let (namespace, path) = split_resource_id(id);
        self.assets_root
            .join(namespace)
            .join("models")
            .join(format!("{path}.json"))
    }

    fn resolve_model(&self, id: &str, depth: usize) -> Result<ResolvedJavaModel> {
        if depth > 32 {
            return Err(BlockModelError::Message(format!(
                "Java model parent chain is too deep or cyclic at {id}"
            )));
        }

        let path = self.model_path(id);
        let value = read_json(&path)?;
        let object = value.as_object().ok_or_else(|| {
            BlockModelError::Message(format!(
                "Java model is not a JSON object: {}",
                path.display()
            ))
        })?;

        let mut resolved = if let Some(parent) = object.get("parent").and_then(Value::as_str) {
            self.resolve_model(parent, depth + 1)?
        } else {
            ResolvedJavaModel::default()
        };

        if let Some(textures) = object.get("textures").and_then(Value::as_object) {
            for (name, value) in textures {
                if let Some(texture) = value.as_str() {
                    resolved.textures.insert(name.clone(), texture.to_owned());
                }
            }
        }
        if let Some(elements) = object.get("elements").and_then(Value::as_array) {
            resolved.elements.clone_from(elements);
        }
        Ok(resolved)
    }
}

/// Maps a canonical Bedrock block id to the Java id used as the geometry template source.
#[must_use]
pub fn java_block_id_for_bedrock_state(state: &BlockStateQuery) -> String {
    let canonical = canonical_block_name_for_state(state);
    let name = canonical.strip_prefix("minecraft:").unwrap_or(&canonical);
    let java_name = match name {
        // Bedrock kept `grass` as the historical full grass block id. Java uses `grass_block`.
        "grass" => "grass_block",
        "grass_path" => "dirt_path",
        "wooden_door" => "oak_door",
        "trapdoor" => "oak_trapdoor",
        "fence" => "oak_fence",
        "fence_gate" => "oak_fence_gate",
        "wooden_button" => "oak_button",
        "wooden_pressure_plate" => "oak_pressure_plate",
        other => other,
    };
    format!("minecraft:{java_name}")
}

/// Converts persisted Bedrock state properties into the Java blockstate vocabulary used by model
/// selectors. Family-specific direction encodings are intentionally handled separately.
#[must_use]
pub fn java_properties_for_bedrock_state(state: &BlockStateQuery) -> BTreeMap<String, String> {
    let canonical = canonical_block_name_for_state(state);
    let family = model_family_for_block_name(&canonical);
    let mut properties = BTreeMap::new();

    for (key, value) in &state.states {
        let key = key.strip_prefix("minecraft:").unwrap_or(key);
        properties.insert(key.to_owned(), state_value_string(value));
    }

    alias_bool(state, &mut properties, "open_bit", "open");
    alias_bool(state, &mut properties, "powered_bit", "powered");
    alias_bool(state, &mut properties, "attached_bit", "attached");
    alias_bool(state, &mut properties, "in_wall_bit", "in_wall");

    if let Some(axis) = state_string(state, "pillar_axis").or_else(|| state_string(state, "axis")) {
        properties.insert("axis".to_owned(), axis.to_owned());
    }

    match family {
        ModelFamily::Trapdoor => {
            if let Some(direction) = state_i64(state, "direction").and_then(trapdoor_direction) {
                properties.insert("facing".to_owned(), direction.to_owned());
            } else if let Some(direction) = state_string(state, "direction") {
                properties.insert("facing".to_owned(), direction.to_owned());
            }
            let top = state_bool(state, "upside_down_bit").unwrap_or(false);
            properties.insert(
                "half".to_owned(),
                if top { "top" } else { "bottom" }.to_owned(),
            );
        }
        ModelFamily::Door => {
            if let Some(direction) = bedrock_cardinal_direction(state) {
                properties.insert("facing".to_owned(), direction.to_owned());
            }
            let upper = state_bool(state, "upper_block_bit").unwrap_or(false);
            properties.insert(
                "half".to_owned(),
                if upper { "upper" } else { "lower" }.to_owned(),
            );
            let hinge_right = state_bool(state, "door_hinge_bit").unwrap_or(false);
            properties.insert(
                "hinge".to_owned(),
                if hinge_right { "right" } else { "left" }.to_owned(),
            );
        }
        ModelFamily::Stairs => {
            if let Some(direction) = bedrock_stair_direction(state) {
                properties.insert("facing".to_owned(), direction.to_owned());
            }
            let top = state_bool(state, "upside_down_bit").unwrap_or(false);
            properties.insert(
                "half".to_owned(),
                if top { "top" } else { "bottom" }.to_owned(),
            );
        }
        ModelFamily::Slab => {
            if let Some(half) = state_string(state, "vertical_half") {
                properties.insert(
                    "type".to_owned(),
                    if half == "top" { "top" } else { "bottom" }.to_owned(),
                );
            }
        }
        _ => {
            if let Some(direction) = bedrock_cardinal_direction(state) {
                properties.insert("facing".to_owned(), direction.to_owned());
            }
        }
    }

    if let Some(face) = state_i64(state, "facing_direction").and_then(facing_direction_0_5) {
        properties.insert("facing".to_owned(), face.to_owned());
    }

    // Java variants frequently include these properties even when the corresponding Bedrock
    // state is implicit rather than persisted.
    properties
        .entry("powered".to_owned())
        .or_insert_with(|| "false".to_owned());
    properties
        .entry("waterlogged".to_owned())
        .or_insert_with(|| "false".to_owned());
    properties
}

fn matching_applies(
    blockstate: &Value,
    properties: &BTreeMap<String, String>,
) -> Vec<JavaVariantApply> {
    let mut applies = Vec::new();
    if let Some(variants) = blockstate.get("variants").and_then(Value::as_object) {
        for (selector, apply) in variants {
            if variant_selector_matches(selector, properties) {
                if let Some(apply) = parse_apply(apply) {
                    applies.push(apply);
                }
                break;
            }
        }
    }

    if let Some(parts) = blockstate.get("multipart").and_then(Value::as_array) {
        for part in parts {
            let when_matches = part
                .get("when")
                .is_none_or(|when| multipart_when_matches(when, properties));
            if when_matches && let Some(apply) = part.get("apply").and_then(parse_apply) {
                applies.push(apply);
            }
        }
    }
    applies
}

fn parse_apply(value: &Value) -> Option<JavaVariantApply> {
    let value = match value {
        Value::Array(items) => items.first()?,
        _ => value,
    };
    let object = value.as_object()?;
    Some(JavaVariantApply {
        model: object.get("model")?.as_str()?.to_owned(),
        x: object.get("x").and_then(Value::as_i64).unwrap_or(0) as i32,
        y: object.get("y").and_then(Value::as_i64).unwrap_or(0) as i32,
        uvlock: object
            .get("uvlock")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn variant_selector_matches(selector: &str, properties: &BTreeMap<String, String>) -> bool {
    if selector.trim().is_empty() {
        return true;
    }
    selector.split(',').all(|term| {
        let Some((key, values)) = term.split_once('=') else {
            return false;
        };
        properties
            .get(key.trim())
            .is_some_and(|actual| values.split('|').any(|expected| actual == expected.trim()))
    })
}

fn multipart_when_matches(when: &Value, properties: &BTreeMap<String, String>) -> bool {
    let Some(object) = when.as_object() else {
        return false;
    };
    if let Some(or) = object.get("OR").and_then(Value::as_array) {
        return or
            .iter()
            .any(|item| multipart_when_matches(item, properties));
    }
    if let Some(and) = object.get("AND").and_then(Value::as_array) {
        return and
            .iter()
            .all(|item| multipart_when_matches(item, properties));
    }
    object.iter().all(|(key, expected)| {
        let Some(actual) = properties.get(key) else {
            return false;
        };
        match expected {
            Value::String(expected) => expected.split('|').any(|value| value == actual),
            Value::Bool(expected) => actual == if *expected { "true" } else { "false" },
            Value::Number(expected) => expected.to_string() == *actual,
            _ => false,
        }
    })
}

fn model_shape_from_java(model: &ResolvedJavaModel, warnings: &mut Vec<String>) -> ModelShape {
    let mut cuboids = Vec::with_capacity(model.elements.len());
    for element in &model.elements {
        let Some(object) = element.as_object() else {
            continue;
        };
        if object.get("rotation").is_some() {
            warnings.push("Java element rotation is not axis-aligned; element skipped".to_owned());
            continue;
        }
        let Some(from) = object.get("from").and_then(vector3) else {
            continue;
        };
        let Some(to) = object.get("to").and_then(vector3) else {
            continue;
        };
        let mut cuboid = ModelCuboid::new(scale16(from), scale16(to));
        if let Some(faces) = object.get("faces").and_then(Value::as_object) {
            for (face_name, face_value) in faces {
                let face = BlockFace::parse(face_name);
                if matches!(face, BlockFace::Default | BlockFace::All | BlockFace::Side) {
                    continue;
                }
                let texture = face_value
                    .get("texture")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let slot = java_texture_slot(texture, face);
                cuboid = cuboid.with_face_material_slot(face, slot);
            }
        }
        cuboids.push(detail_cuboid_with_local_uv(cuboid));
    }
    ModelShape::from_cuboids(cuboids)
}

fn java_texture_slot(texture: &str, face: BlockFace) -> &'static str {
    let texture = texture.trim_start_matches('#');
    match texture {
        "top" | "up" | "end" => "up",
        "bottom" | "down" => "down",
        "side" => "side",
        "front" => "front",
        _ => face.material_slot(),
    }
}

fn rotate_shape_quarter_turns(shape: &mut ModelShape, x: i32, y: i32) -> Result<()> {
    let x_turns = quarter_turns(x)?;
    let y_turns = quarter_turns(y)?;
    for _ in 0..x_turns {
        rotate_shape_x_90(shape);
    }
    for _ in 0..y_turns {
        rotate_shape_y_90(shape);
    }
    Ok(())
}

fn quarter_turns(degrees: i32) -> Result<u8> {
    if degrees.rem_euclid(90) != 0 {
        return Err(BlockModelError::Message(format!(
            "Java blockstate rotation must be a multiple of 90 degrees, got {degrees}"
        )));
    }
    Ok((degrees.rem_euclid(360) / 90) as u8)
}

fn rotate_shape_x_90(shape: &mut ModelShape) {
    for cuboid in &mut shape.cuboids {
        rotate_cuboid(cuboid, rotate_point_x_90, rotate_face_x_90);
    }
    for plane in &mut shape.planes {
        plane.corners = plane.corners.map(rotate_point_x_90);
        plane.normal = rotate_normal_x_90(plane.normal);
    }
}

fn rotate_shape_y_90(shape: &mut ModelShape) {
    for cuboid in &mut shape.cuboids {
        rotate_cuboid(cuboid, rotate_point_y_90, rotate_face_y_90);
    }
    for plane in &mut shape.planes {
        plane.corners = plane.corners.map(rotate_point_y_90);
        plane.normal = rotate_normal_y_90(plane.normal);
    }
}

fn rotate_cuboid(
    cuboid: &mut ModelCuboid,
    rotate_point: fn([f32; 3]) -> [f32; 3],
    rotate_face: fn(BlockFace) -> BlockFace,
) {
    let [min_x, min_y, min_z] = cuboid.min;
    let [max_x, max_y, max_z] = cuboid.max;
    let corners = [
        [min_x, min_y, min_z],
        [min_x, min_y, max_z],
        [min_x, max_y, min_z],
        [min_x, max_y, max_z],
        [max_x, min_y, min_z],
        [max_x, min_y, max_z],
        [max_x, max_y, min_z],
        [max_x, max_y, max_z],
    ]
    .map(rotate_point);
    cuboid.min = [f32::INFINITY; 3];
    cuboid.max = [f32::NEG_INFINITY; 3];
    for corner in corners {
        for axis in 0..3 {
            cuboid.min[axis] = cuboid.min[axis].min(corner[axis]);
            cuboid.max[axis] = cuboid.max[axis].max(corner[axis]);
        }
    }
    cuboid.face_material_slots = std::mem::take(&mut cuboid.face_material_slots)
        .into_iter()
        .map(|(face, slot)| (rotate_face(face), slot))
        .collect();
    cuboid.face_uvs = std::mem::take(&mut cuboid.face_uvs)
        .into_iter()
        .map(|(face, uv)| (rotate_face(face), uv))
        .collect();
}

fn rotate_point_y_90([x, y, z]: [f32; 3]) -> [f32; 3] {
    [1.0 - z, y, x]
}

fn rotate_point_x_90([x, y, z]: [f32; 3]) -> [f32; 3] {
    [x, 1.0 - z, y]
}

fn rotate_normal_y_90([x, y, z]: [i32; 3]) -> [i32; 3] {
    [-z, y, x]
}

fn rotate_normal_x_90([x, y, z]: [i32; 3]) -> [i32; 3] {
    [x, -z, y]
}

fn rotate_face_y_90(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::North => BlockFace::East,
        BlockFace::East => BlockFace::South,
        BlockFace::South => BlockFace::West,
        BlockFace::West => BlockFace::North,
        other => other,
    }
}

fn rotate_face_x_90(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::North => BlockFace::Up,
        BlockFace::Up => BlockFace::South,
        BlockFace::South => BlockFace::Down,
        BlockFace::Down => BlockFace::North,
        other => other,
    }
}

fn bedrock_cardinal_direction(state: &BlockStateQuery) -> Option<&'static str> {
    state_string(state, "cardinal_direction")
        .and_then(cardinal_direction_string)
        .or_else(|| state_i64(state, "direction").and_then(cardinal_direction_0_3))
        .or_else(|| state_i64(state, "weirdo_direction").and_then(cardinal_direction_0_3))
}

fn bedrock_stair_direction(state: &BlockStateQuery) -> Option<&'static str> {
    state_string(state, "cardinal_direction")
        .and_then(cardinal_direction_string)
        .or_else(|| state_string(state, "facing").and_then(cardinal_direction_string))
        .or_else(|| state_string(state, "direction").and_then(cardinal_direction_string))
        .or_else(|| state_i64(state, "weirdo_direction").and_then(stair_direction_0_3))
        .or_else(|| state_i64(state, "direction").and_then(cardinal_direction_0_3))
}

fn cardinal_direction_string(value: &str) -> Option<&'static str> {
    match value
        .trim()
        .strip_prefix("minecraft:")
        .unwrap_or(value.trim())
    {
        "north" => Some("north"),
        "south" => Some("south"),
        "east" => Some("east"),
        "west" => Some("west"),
        _ => None,
    }
}

fn cardinal_direction_0_3(value: i64) -> Option<&'static str> {
    match value.rem_euclid(4) {
        0 => Some("south"),
        1 => Some("west"),
        2 => Some("north"),
        3 => Some("east"),
        _ => None,
    }
}

fn stair_direction_0_3(value: i64) -> Option<&'static str> {
    match value.rem_euclid(4) {
        0 => Some("east"),
        1 => Some("west"),
        2 => Some("south"),
        3 => Some("north"),
        _ => None,
    }
}

fn trapdoor_direction(value: i64) -> Option<&'static str> {
    match value.rem_euclid(4) {
        0 => Some("west"),
        1 => Some("east"),
        2 => Some("north"),
        3 => Some("south"),
        _ => None,
    }
}

fn facing_direction_0_5(value: i64) -> Option<&'static str> {
    match value {
        0 => Some("down"),
        1 => Some("up"),
        2 => Some("north"),
        3 => Some("south"),
        4 => Some("west"),
        5 => Some("east"),
        _ => None,
    }
}

fn alias_bool(
    state: &BlockStateQuery,
    properties: &mut BTreeMap<String, String>,
    source: &str,
    target: &str,
) {
    if let Some(value) = state_bool(state, source) {
        properties.insert(target.to_owned(), value.to_string());
    }
}

fn state_bool(state: &BlockStateQuery, key: &str) -> Option<bool> {
    match state.state(key)? {
        BlockStateValue::Bool(value) => Some(*value),
        BlockStateValue::Int(value) => Some(*value != 0),
        BlockStateValue::String(value) => match value.as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
    }
}

fn state_i64(state: &BlockStateQuery, key: &str) -> Option<i64> {
    match state.state(key)? {
        BlockStateValue::Int(value) => Some(*value),
        BlockStateValue::Bool(value) => Some(i64::from(*value)),
        BlockStateValue::String(value) => value.parse().ok(),
    }
}

fn state_string<'a>(state: &'a BlockStateQuery, key: &str) -> Option<&'a str> {
    match state.state(key)? {
        BlockStateValue::String(value) => Some(value),
        BlockStateValue::Bool(_) | BlockStateValue::Int(_) => None,
    }
}

fn state_value_string(value: &BlockStateValue) -> String {
    match value {
        BlockStateValue::Bool(value) => value.to_string(),
        BlockStateValue::Int(value) => value.to_string(),
        BlockStateValue::String(value) => value.clone(),
    }
}

fn vector3(value: &Value) -> Option<[f32; 3]> {
    let values = value.as_array()?;
    Some([
        number_f32(values.first()?)?,
        number_f32(values.get(1)?)?,
        number_f32(values.get(2)?)?,
    ])
}

fn number_f32(value: &Value) -> Option<f32> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Java model coordinates are renderer f32 values"
    )]
    value.as_f64().map(|value| value as f32)
}

fn scale16([x, y, z]: [f32; 3]) -> [f32; 3] {
    [x / 16.0, y / 16.0, z / 16.0]
}

fn split_resource_id(id: &str) -> (&str, &str) {
    id.split_once(':').unwrap_or(("minecraft", id))
}

fn read_json(path: &Path) -> Result<Value> {
    let bytes = std::fs::read(path).map_err(|source| BlockModelError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| BlockModelError::Json {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_bedrock_grass_maps_to_java_grass_block() {
        let state = BlockStateQuery::new("minecraft:grass");
        assert_eq!(
            java_block_id_for_bedrock_state(&state),
            "minecraft:grass_block"
        );
    }

    #[test]
    fn trapdoor_direction_is_not_conflated_with_door_direction() {
        let trapdoor = BlockStateQuery::new("minecraft:oak_trapdoor")
            .with_state("direction", 0)
            .with_state("open_bit", true)
            .with_state("upside_down_bit", false);
        let door = BlockStateQuery::new("minecraft:oak_door")
            .with_state("direction", 0)
            .with_state("open_bit", true)
            .with_state("upper_block_bit", false)
            .with_state("door_hinge_bit", false);

        assert_eq!(
            java_properties_for_bedrock_state(&trapdoor)
                .get("facing")
                .map(String::as_str),
            Some("west")
        );
        assert_eq!(
            java_properties_for_bedrock_state(&door)
                .get("facing")
                .map(String::as_str),
            Some("south")
        );
    }

    #[test]
    fn stairs_weirdo_direction_matches_bedrock_encoding() {
        let cases = [(0, "east"), (1, "west"), (2, "south"), (3, "north")];
        for (direction, expected) in cases {
            let stairs = BlockStateQuery::new("minecraft:oak_stairs")
                .with_state("weirdo_direction", direction)
                .with_state("upside_down_bit", false);
            assert_eq!(
                java_properties_for_bedrock_state(&stairs)
                    .get("facing")
                    .map(String::as_str),
                Some(expected),
                "weirdo_direction={direction}",
            );
        }
    }

    #[test]
    fn java_variant_selector_matches_property_sets() {
        let properties = BTreeMap::from([
            ("facing".to_owned(), "east".to_owned()),
            ("half".to_owned(), "top".to_owned()),
            ("open".to_owned(), "true".to_owned()),
        ]);
        assert!(variant_selector_matches(
            "facing=east,half=top,open=true",
            &properties
        ));
        assert!(!variant_selector_matches(
            "facing=north,half=top,open=true",
            &properties
        ));
    }

    #[test]
    fn positive_java_y_rotation_moves_north_model_to_east() {
        let mut shape =
            ModelShape::from_cuboids([ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 0.25])]);
        rotate_shape_quarter_turns(&mut shape, 0, 90).unwrap();
        assert_eq!(shape.cuboids[0].min, [0.75, 0.0, 0.0]);
        assert_eq!(shape.cuboids[0].max, [1.0, 1.0, 1.0]);
    }
}
