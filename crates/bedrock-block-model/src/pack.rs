use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::error::Result;
use crate::geometry::{BlockGeometry, GeometryLibrary};
use crate::json::{read_json_file, read_resource_json};
use crate::material::BlockComponents;
use crate::permutation::BlockPermutation;
use crate::resolver::{ResolvedBlockModel, resolve_block};
use crate::resource::PackResources;
use crate::state::BlockStateQuery;
use crate::texture::TerrainTextureAtlas;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockModelRepository {
    pub blocks: BTreeMap<String, BlockDefinition>,
    pub terrain_textures: TerrainTextureAtlas,
    pub geometries: GeometryLibrary,
}

impl BlockModelRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads and merges resource-pack block model data from the given roots.
    ///
    /// Later roots override earlier roots.
    ///
    /// # Errors
    ///
    /// Returns an error when a pack JSON file cannot be read, parsed, or discovered.
    pub fn load_packs<I, P>(pack_roots: I) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut repository = Self::new();
        for pack_root in pack_roots {
            repository.merge_pack(pack_root.as_ref())?;
        }
        Ok(repository)
    }

    /// Merges one resource-pack root into this repository.
    ///
    /// Reads loose files and mirrored `__brarchive` v1 entries without extracting
    /// the pack. Embedded content wins over a matching loose file; empty archive
    /// registrations fall back to loose files. Only text JSON is interpreted,
    /// preserving unknown fields in the existing raw values without conversion.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid/unsupported archives, compiled MCB resources,
    /// or supported JSON files that cannot be read or parsed.
    pub fn merge_pack(&mut self, pack_root: &Path) -> Result<()> {
        self.merge_resources(&PackResources::open(pack_root)?)
    }

    pub(crate) fn merge_resources(&mut self, resources: &PackResources) -> Result<()> {
        for path in ["textures/terrain_texture.json", "terrain_texture.json"] {
            if let Some(resource) = resources.get(Path::new(path)) {
                self.terrain_textures
                    .merge_value(&read_resource_json(&resource)?);
            }
        }
        if let Some(resource) = resources.get(Path::new("blocks.json")) {
            self.merge_blocks_value(&read_resource_json(&resource)?);
        }
        for folder in ["blocks", "definitions/blocks"] {
            for resource in resources.json_files(Path::new(folder))?.values() {
                self.merge_blocks_value(&read_resource_json(resource)?);
            }
        }
        for resource in resources.json_files(Path::new("models"))?.values() {
            self.geometries.merge_value(&read_resource_json(resource)?);
        }
        Ok(())
    }

    /// Merges block definitions from one `blocks.json` or custom block JSON file.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or parsed.
    pub fn merge_blocks_file(&mut self, path: &Path) -> Result<()> {
        let value = read_json_file(path)?;
        self.merge_blocks_value(&value);
        Ok(())
    }

    pub fn merge_blocks_value(&mut self, value: &Value) {
        for block in block_definitions_from_value(value) {
            let identifier = block.identifier.clone();
            if let Some(alias) = minecraft_identifier_alias(&identifier) {
                self.blocks.insert(alias, block.clone());
            }
            self.blocks.insert(identifier, block);
        }
    }

    /// Merges geometry definitions from one JSON file.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or parsed.
    pub fn merge_geometry_file(&mut self, path: &Path) -> Result<()> {
        self.geometries.merge_file(path)
    }

    #[must_use]
    pub fn resolve_block(&self, state: &BlockStateQuery) -> ResolvedBlockModel {
        resolve_block(self, state)
    }

    #[must_use]
    pub fn geometry(&self, identifier: &str) -> Option<&BlockGeometry> {
        self.geometries.get(identifier)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockDefinition {
    pub identifier: String,
    pub components: BlockComponents,
    pub permutations: Vec<BlockPermutation>,
    pub raw: Value,
}

#[must_use]
pub fn block_definitions_from_value(value: &Value) -> Vec<BlockDefinition> {
    if let Some(block) = custom_block_definition_from_value(value) {
        return vec![block];
    }

    if let Some(blocks) = value.get("blocks").and_then(Value::as_object) {
        return blocks
            .iter()
            .map(|(identifier, block_value)| legacy_block_definition(identifier, block_value))
            .collect();
    }

    let Some(object) = value.as_object() else {
        return Vec::new();
    };

    object
        .iter()
        .filter_map(|(identifier, block_value)| {
            if is_metadata_key(identifier) {
                None
            } else {
                Some(legacy_block_definition(identifier, block_value))
            }
        })
        .collect()
}

fn custom_block_definition_from_value(value: &Value) -> Option<BlockDefinition> {
    let minecraft_block = value.get("minecraft:block")?;
    let description = minecraft_block.get("description")?;
    let identifier = description.get("identifier")?.as_str()?.to_owned();
    let components = minecraft_block
        .get("components")
        .map(BlockComponents::from_components)
        .unwrap_or_default();
    let permutations = minecraft_block
        .get("permutations")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(permutation_from_value).collect())
        .unwrap_or_default();

    Some(BlockDefinition {
        identifier,
        components,
        permutations,
        raw: value.clone(),
    })
}

fn legacy_block_definition(identifier: &str, value: &Value) -> BlockDefinition {
    BlockDefinition {
        identifier: identifier.to_owned(),
        components: BlockComponents::from_legacy_block(value),
        permutations: Vec::new(),
        raw: value.clone(),
    }
}

fn permutation_from_value(value: &Value) -> Option<BlockPermutation> {
    let object = value.as_object()?;
    Some(BlockPermutation {
        condition: object
            .get("condition")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        components: object
            .get("components")
            .map(BlockComponents::from_components)
            .unwrap_or_default(),
    })
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
            | "minecraft:block"
    )
}

fn minecraft_identifier_alias(identifier: &str) -> Option<String> {
    if let Some(stripped) = identifier.strip_prefix("minecraft:") {
        return Some(stripped.to_owned());
    }
    (!identifier.contains(':')).then(|| format!("minecraft:{identifier}"))
}

#[cfg(test)]
mod tests {
    use super::block_definitions_from_value;

    #[test]
    fn block_definitions_from_value_should_parse_legacy_blocks_json() {
        let value = serde_json::json!({
            "format_version": "1.20.0",
            "minecraft:grass_block": {
                "textures": {
                    "up": "grass_top",
                    "down": "dirt",
                    "side": "grass_side"
                }
            }
        });

        let blocks = block_definitions_from_value(&value);

        assert_eq!(blocks[0].identifier, "minecraft:grass_block");
        assert!(blocks[0].components.textures.is_some());
    }

    #[test]
    fn repository_should_resolve_legacy_blocks_with_minecraft_namespace() {
        let mut repository = super::BlockModelRepository::new();
        repository.merge_blocks_value(&serde_json::json!({
            "grass": {
                "textures": {
                    "up": "grass_top",
                    "down": "grass_bottom",
                    "side": "grass_side"
                }
            }
        }));
        repository.terrain_textures.merge_value(&serde_json::json!({
            "texture_data": {
                "grass_top": { "textures": "textures/blocks/grass_top" }
            }
        }));

        let resolved =
            repository.resolve_block(&crate::state::BlockStateQuery::new("minecraft:grass"));

        assert_eq!(
            resolved
                .face_textures
                .get(&crate::material::BlockFace::Up)
                .map(|texture| texture.key.as_str()),
            Some("grass_top")
        );
    }

    #[test]
    fn block_definitions_from_value_should_parse_custom_block_permutations() {
        let value = serde_json::json!({
            "minecraft:block": {
                "description": { "identifier": "example:test" },
                "components": { "minecraft:geometry": "geometry.example.default" },
                "permutations": [{
                    "condition": "q.block_state('open_bit')",
                    "components": { "minecraft:geometry": "geometry.example.open" }
                }]
            }
        });

        let blocks = block_definitions_from_value(&value);

        assert_eq!(blocks[0].permutations.len(), 1);
    }
}
