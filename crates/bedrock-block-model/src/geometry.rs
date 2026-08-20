use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::Result;
use crate::json::read_json_file;
use crate::material::BlockFace;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeometryLibrary {
    pub geometries: BTreeMap<String, BlockGeometry>,
}

impl GeometryLibrary {
    /// Merges geometry definitions from one JSON file.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or parsed as resource-pack JSON.
    pub fn merge_file(&mut self, path: &Path) -> Result<()> {
        let value = read_json_file(path)?;
        self.merge_value(&value);
        Ok(())
    }

    pub fn merge_value(&mut self, value: &Value) {
        for geometry in geometries_from_value(value) {
            self.geometries
                .insert(geometry.identifier.clone(), geometry);
        }
    }

    #[must_use]
    pub fn get(&self, identifier: &str) -> Option<&BlockGeometry> {
        self.geometries.get(identifier)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockGeometry {
    pub identifier: String,
    pub bones: Vec<GeometryBone>,
    pub raw: Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeometryBone {
    pub name: String,
    pub parent: Option<String>,
    pub pivot: Option<[f32; 3]>,
    pub rotation: Option<[f32; 3]>,
    pub cubes: Vec<GeometryCube>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeometryCube {
    pub origin: Option<[f32; 3]>,
    pub size: Option<[f32; 3]>,
    pub pivot: Option<[f32; 3]>,
    pub rotation: Option<[f32; 3]>,
    pub material_instance: Option<String>,
    pub face_material_instances: BTreeMap<BlockFace, String>,
    pub uv: Option<GeometryUv>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeometryUv {
    pub raw: Value,
}

#[must_use]
pub fn geometries_from_value(value: &Value) -> Vec<BlockGeometry> {
    let mut geometries = Vec::new();

    if let Some(minecraft_geometry) = value.get("minecraft:geometry") {
        match minecraft_geometry {
            Value::Array(items) => {
                for item in items {
                    if let Some(geometry) = geometry_from_modern_value(item) {
                        geometries.push(geometry);
                    }
                }
            }
            Value::Object(_) => {
                if let Some(geometry) = geometry_from_modern_value(minecraft_geometry) {
                    geometries.push(geometry);
                }
            }
            Value::String(_) | Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    if let Some(object) = value.as_object() {
        for (key, legacy_geometry) in object {
            if key.starts_with("geometry.") {
                geometries.push(geometry_from_legacy_value(key, legacy_geometry));
            }
        }
    }

    geometries
}

fn geometry_from_modern_value(value: &Value) -> Option<BlockGeometry> {
    let object = value.as_object()?;
    let identifier = object
        .get("description")
        .and_then(|description| description.get("identifier"))
        .and_then(Value::as_str)
        .or_else(|| object.get("identifier").and_then(Value::as_str))?;

    Some(BlockGeometry {
        identifier: identifier.to_owned(),
        bones: object
            .get("bones")
            .and_then(Value::as_array)
            .map(|bones| bones.iter().filter_map(bone_from_value).collect())
            .unwrap_or_default(),
        raw: value.clone(),
    })
}

fn geometry_from_legacy_value(identifier: &str, value: &Value) -> BlockGeometry {
    let object = value.as_object();
    BlockGeometry {
        identifier: identifier.to_owned(),
        bones: object
            .and_then(|geometry_object| geometry_object.get("bones"))
            .and_then(Value::as_array)
            .map(|bones| bones.iter().filter_map(bone_from_value).collect())
            .unwrap_or_default(),
        raw: value.clone(),
    }
}

fn bone_from_value(value: &Value) -> Option<GeometryBone> {
    let object = value.as_object()?;
    Some(GeometryBone {
        name: object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        parent: object
            .get("parent")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        pivot: object.get("pivot").and_then(vector3_from_value),
        rotation: object.get("rotation").and_then(vector3_from_value),
        cubes: object
            .get("cubes")
            .and_then(Value::as_array)
            .map(|cubes| cubes.iter().filter_map(cube_from_value).collect())
            .unwrap_or_default(),
    })
}

fn cube_from_value(value: &Value) -> Option<GeometryCube> {
    let object = value.as_object()?;
    Some(GeometryCube {
        origin: object.get("origin").and_then(vector3_from_value),
        size: object.get("size").and_then(vector3_from_value),
        pivot: object.get("pivot").and_then(vector3_from_value),
        rotation: object.get("rotation").and_then(vector3_from_value),
        material_instance: object
            .get("material_instance")
            .or_else(|| object.get("material"))
            .and_then(material_instance_from_value)
            .or_else(|| {
                object
                    .get("uv")
                    .and_then(|uv| uv.get("material_instance"))
                    .and_then(material_instance_from_value)
            }),
        face_material_instances: object
            .get("uv")
            .map(face_material_instances_from_uv)
            .unwrap_or_default(),
        uv: object.get("uv").map(|uv| GeometryUv { raw: uv.clone() }),
    })
}

fn face_material_instances_from_uv(value: &Value) -> BTreeMap<BlockFace, String> {
    let mut instances = BTreeMap::new();
    let Some(object) = value.as_object() else {
        return instances;
    };

    for (key, face) in [
        ("up", BlockFace::Up),
        ("down", BlockFace::Down),
        ("north", BlockFace::North),
        ("south", BlockFace::South),
        ("east", BlockFace::East),
        ("west", BlockFace::West),
    ] {
        if let Some(instance) = object
            .get(key)
            .and_then(|value| {
                value
                    .get("material_instance")
                    .or_else(|| value.get("material"))
            })
            .and_then(material_instance_from_value)
        {
            instances.insert(face, instance);
        }
    }

    instances
}

fn material_instance_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Object(object) => object
            .get("name")
            .or_else(|| object.get("slot"))
            .or_else(|| object.get("material_instance"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        Value::Array(items) => items.iter().find_map(material_instance_from_value),
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

fn vector3_from_value(value: &Value) -> Option<[f32; 3]> {
    let items = value.as_array()?;
    Some([
        number_to_f32(items.first()?)?,
        number_to_f32(items.get(1)?)?,
        number_to_f32(items.get(2)?)?,
    ])
}

fn number_to_f32(value: &Value) -> Option<f32> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Bedrock geometry coordinates are authored as JSON numbers and consumed as renderer f32 values."
    )]
    value.as_f64().map(|number| number as f32)
}

#[cfg(test)]
mod tests {
    use super::geometries_from_value;

    #[test]
    fn geometries_from_value_should_parse_modern_geometry_array() {
        let value = serde_json::json!({
            "minecraft:geometry": [{
                "description": { "identifier": "geometry.test.block" },
                "bones": [{
                    "name": "root",
                    "cubes": [{ "origin": [0, 0, 0], "size": [16, 16, 16] }]
                }]
            }]
        });

        let geometries = geometries_from_value(&value);

        assert_eq!(geometries[0].identifier, "geometry.test.block");
        assert_eq!(geometries[0].bones[0].cubes.len(), 1);
    }
}
