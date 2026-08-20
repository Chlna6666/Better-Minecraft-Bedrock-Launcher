use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum BlockFace {
    Up,
    Down,
    North,
    South,
    East,
    West,
    Side,
    All,
    Default,
}

impl BlockFace {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "up" | "top" => Self::Up,
            "down" | "bottom" => Self::Down,
            "north" => Self::North,
            "south" => Self::South,
            "east" => Self::East,
            "west" => Self::West,
            "side" => Self::Side,
            "all" => Self::All,
            _ => Self::Default,
        }
    }

    #[must_use]
    pub fn material_slot(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::North => "north",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
            Self::Side => "side",
            Self::All | Self::Default => "*",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TextureSet {
    pub faces: BTreeMap<BlockFace, TextureReference>,
}

impl TextureSet {
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::String(texture) => Some(Self::single(texture)),
            Value::Array(items) => items.iter().find_map(Self::from_value),
            Value::Object(object) => {
                let mut textures = Self::default();
                for (key, face_value) in object {
                    let Some(texture_reference) = TextureReference::from_value(face_value) else {
                        continue;
                    };
                    textures
                        .faces
                        .insert(BlockFace::parse(key), texture_reference);
                }

                (!textures.faces.is_empty()).then_some(textures)
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => None,
        }
    }

    #[must_use]
    pub fn single(texture: &str) -> Self {
        let mut faces = BTreeMap::new();
        faces.insert(BlockFace::All, TextureReference::new(texture));
        Self { faces }
    }

    #[must_use]
    pub fn texture_for_face(&self, face: BlockFace) -> Option<&TextureReference> {
        self.faces
            .get(&face)
            .or_else(|| {
                matches!(
                    face,
                    BlockFace::North | BlockFace::South | BlockFace::East | BlockFace::West
                )
                .then(|| self.faces.get(&BlockFace::Side))
                .flatten()
            })
            .or_else(|| self.faces.get(&BlockFace::All))
            .or_else(|| self.faces.get(&BlockFace::Default))
    }

    pub fn merge_from(&mut self, other: Self) {
        for (face, texture) in other.faces {
            self.faces.insert(face, texture);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextureReference {
    pub key: String,
}

impl TextureReference {
    #[must_use]
    pub fn new(key: &str) -> Self {
        Self {
            key: key.to_owned(),
        }
    }

    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::String(texture) => Some(Self::new(texture)),
            Value::Array(items) => items.iter().find_map(Self::from_value),
            Value::Object(object) => object
                .get("texture")
                .or_else(|| object.get("path"))
                .or_else(|| object.get("textures"))
                .and_then(Self::from_value),
            Value::Null | Value::Bool(_) | Value::Number(_) => None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MaterialInstance {
    pub texture: Option<TextureReference>,
    pub render_method: Option<String>,
    pub tint_method: Option<String>,
    pub ambient_occlusion: Option<bool>,
    pub face_dimming: Option<bool>,
}

impl MaterialInstance {
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::String(texture) => Some(Self {
                texture: Some(TextureReference::new(texture)),
                ..Self::default()
            }),
            Value::Object(object) => Some(Self {
                texture: object.get("texture").and_then(TextureReference::from_value),
                render_method: object
                    .get("render_method")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                tint_method: object
                    .get("tint_method")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                ambient_occlusion: object.get("ambient_occlusion").and_then(Value::as_bool),
                face_dimming: object.get("face_dimming").and_then(Value::as_bool),
            }),
            Value::Array(items) => items.iter().find_map(Self::from_value),
            Value::Null | Value::Bool(_) | Value::Number(_) => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockComponents {
    pub geometry: Option<String>,
    pub material_instances: BTreeMap<String, MaterialInstance>,
    pub textures: Option<TextureSet>,
    pub transformation: Option<BlockTransformation>,
}

impl BlockComponents {
    #[must_use]
    pub fn from_legacy_block(value: &Value) -> Self {
        let mut components = Self::default();
        components.merge_legacy_fields(value);
        components
    }

    #[must_use]
    pub fn from_components(value: &Value) -> Self {
        let mut components = Self::default();
        let Some(object) = value.as_object() else {
            return components;
        };

        if let Some(geometry) = object
            .get("minecraft:geometry")
            .or_else(|| object.get("geometry"))
        {
            components.geometry = geometry_identifier(geometry);
        }

        if let Some(material_instances) = object
            .get("minecraft:material_instances")
            .or_else(|| object.get("material_instances"))
        {
            components.material_instances = material_instances_from_value(material_instances);
        }

        if let Some(textures) = object
            .get("minecraft:textures")
            .or_else(|| object.get("textures"))
            .or_else(|| object.get("texture"))
        {
            components.textures = TextureSet::from_value(textures);
        }

        if let Some(transformation) = object
            .get("minecraft:transformation")
            .or_else(|| object.get("transformation"))
        {
            components.transformation = BlockTransformation::from_value(transformation);
        }

        components.merge_legacy_fields(value);
        components
    }

    pub fn merge_from(&mut self, other: Self) {
        if other.geometry.is_some() {
            self.geometry = other.geometry;
        }
        for (slot, material) in other.material_instances {
            self.material_instances.insert(slot, material);
        }
        if let Some(other_textures) = other.textures {
            if let Some(textures) = &mut self.textures {
                textures.merge_from(other_textures);
            } else {
                self.textures = Some(other_textures);
            }
        }
        if other.transformation.is_some() {
            self.transformation = other.transformation;
        }
    }

    fn merge_legacy_fields(&mut self, value: &Value) {
        let Some(object) = value.as_object() else {
            return;
        };

        if self.geometry.is_none() {
            self.geometry = object.get("geometry").and_then(geometry_identifier);
        }
        if self.textures.is_none() {
            self.textures = object
                .get("textures")
                .or_else(|| object.get("texture"))
                .or_else(|| object.get("carried_textures"))
                .and_then(TextureSet::from_value);
        }
        if self.material_instances.is_empty()
            && let Some(material_instances) = object.get("material_instances")
        {
            self.material_instances = material_instances_from_value(material_instances);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockTransformation {
    pub rotation: Option<[f32; 3]>,
    pub scale: Option<[f32; 3]>,
    pub translation: Option<[f32; 3]>,
}

impl BlockTransformation {
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        Some(Self {
            rotation: object.get("rotation").and_then(vector3_from_value),
            scale: object.get("scale").and_then(vector3_from_value),
            translation: object.get("translation").and_then(vector3_from_value),
        })
    }
}

fn geometry_identifier(value: &Value) -> Option<String> {
    match value {
        Value::String(identifier) => Some(identifier.clone()),
        Value::Object(object) => object
            .get("identifier")
            .or_else(|| object.get("geometry"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        Value::Array(_) | Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

fn material_instances_from_value(value: &Value) -> BTreeMap<String, MaterialInstance> {
    let mut material_instances = BTreeMap::new();
    let Some(object) = value.as_object() else {
        return material_instances;
    };

    for (slot, material_value) in object {
        if let Some(material) = MaterialInstance::from_value(material_value) {
            material_instances.insert(slot.clone(), material);
        }
    }

    material_instances
}

fn vector3_from_value(value: &Value) -> Option<[f32; 3]> {
    let items = value.as_array()?;
    let x = items.first().and_then(number_to_f32)?;
    let y = items.get(1).and_then(number_to_f32)?;
    let z = items.get(2).and_then(number_to_f32)?;
    Some([x, y, z])
}

fn number_to_f32(value: &Value) -> Option<f32> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Bedrock transformation coordinates are authored as JSON numbers and consumed as renderer f32 values."
    )]
    value.as_f64().map(|number| number as f32)
}
