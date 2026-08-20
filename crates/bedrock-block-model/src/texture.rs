use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::Result;
use crate::json::read_json_file;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TerrainTextureAtlas {
    pub textures: BTreeMap<String, TerrainTexture>,
}

impl TerrainTextureAtlas {
    /// Merges terrain texture data from one `terrain_texture.json` file.
    ///
    /// Missing files are ignored so callers can probe standard resource-pack locations.
    ///
    /// # Errors
    ///
    /// Returns an error when an existing file cannot be read or parsed.
    pub fn merge_file(&mut self, path: &Path) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }

        let value = read_json_file(path)?;
        self.merge_value(&value);
        Ok(())
    }

    pub fn merge_value(&mut self, value: &Value) {
        let Some(texture_data) = value.get("texture_data").and_then(Value::as_object) else {
            return;
        };

        for (texture_key, texture_value) in texture_data {
            let mut paths = Vec::new();
            collect_texture_paths(
                texture_value.get("textures").unwrap_or(texture_value),
                &mut paths,
            );
            if paths.is_empty() {
                paths.push(normalize_texture_path(texture_key));
            }

            self.textures.insert(
                texture_key.clone(),
                TerrainTexture {
                    key: texture_key.clone(),
                    paths,
                },
            );
        }
    }

    #[must_use]
    pub fn resolve(&self, texture_key: &str) -> TerrainTexture {
        let normalized_key = normalize_texture_key(texture_key);
        self.textures
            .get(&normalized_key)
            .cloned()
            .or_else(|| self.textures.get(texture_key).cloned())
            .unwrap_or_else(|| TerrainTexture {
                key: normalized_key.clone(),
                paths: vec![normalize_texture_path(&normalized_key)],
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerrainTexture {
    pub key: String,
    pub paths: Vec<String>,
}

impl TerrainTexture {
    #[must_use]
    pub fn primary_path(&self) -> Option<&str> {
        self.paths.first().map(String::as_str)
    }
}

#[must_use]
pub fn normalize_texture_key(texture_key: &str) -> String {
    let normalized = normalize_texture_path(texture_key);
    let without_textures = normalized.strip_prefix("textures/").unwrap_or(&normalized);
    without_textures
        .strip_prefix("blocks/")
        .unwrap_or(without_textures)
        .to_owned()
}

#[must_use]
pub fn normalize_texture_path(texture_path: &str) -> String {
    let path = texture_path.replace('\\', "/");
    let trimmed = path
        .strip_suffix(".png")
        .or_else(|| path.strip_suffix(".tga"))
        .or_else(|| path.strip_suffix(".jpg"))
        .unwrap_or(&path);
    trimmed.trim_matches('/').to_owned()
}

#[must_use]
pub fn terrain_texture_paths(pack_root: &Path) -> Vec<PathBuf> {
    [
        pack_root.join("textures").join("terrain_texture.json"),
        pack_root.join("terrain_texture.json"),
    ]
    .into_iter()
    .collect()
}

fn collect_texture_paths(value: &Value, paths: &mut Vec<String>) {
    match value {
        Value::String(texture_path) => paths.push(normalize_texture_path(texture_path)),
        Value::Array(items) => {
            for item in items {
                collect_texture_paths(item, paths);
            }
        }
        Value::Object(object) => {
            if let Some(path) = object
                .get("path")
                .or_else(|| object.get("texture"))
                .or_else(|| object.get("textures"))
            {
                collect_texture_paths(path, paths);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{TerrainTextureAtlas, normalize_texture_key};

    #[test]
    fn normalize_texture_key_should_strip_terrain_prefixes() {
        assert_eq!(
            normalize_texture_key("textures/blocks/amethyst_cluster"),
            "amethyst_cluster"
        );
    }

    #[test]
    fn resolve_should_not_require_item_texture_data() {
        let mut atlas = TerrainTextureAtlas::default();
        atlas.merge_value(&serde_json::json!({
            "texture_data": {
                "grass_top": { "textures": "textures/blocks/grass_top" }
            }
        }));

        let texture = atlas.resolve("grass_top");

        assert_eq!(texture.primary_path(), Some("textures/blocks/grass_top"));
    }
}
