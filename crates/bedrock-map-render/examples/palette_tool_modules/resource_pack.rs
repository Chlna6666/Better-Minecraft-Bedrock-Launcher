//! Read-only view of a standalone pack or an installed vanilla version stack.

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

use super::super::{Result, read_jsonc, validation};

pub(crate) struct ResourcePack {
    layers: Vec<PathBuf>,
    version: Option<[u32; 3]>,
}

impl ResourcePack {
    pub(crate) fn open(selected: &Path) -> Result<Self> {
        let root = if selected.join("vanilla").is_dir() {
            Some(selected)
        } else if selected.file_name().is_some_and(|name| name == "vanilla") {
            selected
                .parent()
                .filter(|parent| parent.join("vanilla_base").is_dir())
        } else {
            None
        };
        let Some(root) = root else {
            return Ok(Self {
                layers: vec![selected.to_owned()],
                version: None,
            });
        };
        let mut layers = vec![root.join("vanilla")];
        let mut versions = Vec::new();
        for entry in fs::read_dir(root).map_err(|error| validation(error.to_string()))? {
            let entry = entry.map_err(|error| validation(error.to_string()))?;
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let Some(version) = name.to_str().and_then(version_from_name) else {
                continue;
            };
            versions.push((version, entry.path()));
        }
        versions.sort_by_key(|(version, _)| *version);
        let version = versions.last().map(|(version, _)| *version);
        layers.extend(versions.into_iter().map(|(_, path)| path));
        Ok(Self { layers, version })
    }

    pub(crate) fn version_label(&self) -> Option<String> {
        self.version.map(|[major, minor, patch]| {
            if major == 1 && minor >= 26 {
                format!("{minor}.{patch}")
            } else {
                format!("{major}.{minor}.{patch}")
            }
        })
    }

    /// Later version layers override both texture files and JSON definitions.
    pub(crate) fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        let relative = relative.as_ref();
        self.layers
            .iter()
            .rev()
            .map(|layer| layer.join(relative))
            .find(|path| path.is_file())
            .unwrap_or_else(|| self.layers[0].join(relative))
    }

    pub(crate) fn json(&self, names: &[&str]) -> Result<Value> {
        self.optional_json(names)?
            .ok_or_else(|| validation(format!("resource pack has no {}", names[0])))
    }

    pub(crate) fn optional_json(&self, names: &[&str]) -> Result<Option<Value>> {
        let mut merged = None;
        for layer in &self.layers {
            let Some(path) = names
                .iter()
                .map(|name| layer.join(name))
                .find(|path| path.is_file())
            else {
                continue;
            };
            let value = read_jsonc(&path)?;
            if let Some(base) = merged.as_mut() {
                merge(base, value);
            } else {
                merged = Some(value);
            }
        }
        Ok(merged)
    }
}

fn version_from_name(name: &str) -> Option<[u32; 3]> {
    let mut parts = name.strip_prefix("vanilla_")?.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().map_or(Some(0), |part| part.parse().ok())?;
    if parts.next().is_some() {
        return None;
    }
    Some([major, minor, patch])
}

fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                if let Some(previous) = base.get_mut(&key) {
                    merge(previous, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numeric_version_order_excludes_unrelated_packs() {
        assert!(version_from_name("vanilla_1.21.90") < version_from_name("vanilla_1.21.100"));
        assert_eq!(version_from_name("vanilla_1.14"), Some([1, 14, 0]));
        for name in [
            "beta",
            "chemistry_1.26.40",
            "vanilla_music",
            "vanilla_base",
            "vanilla_1.26.40.extra",
        ] {
            assert_eq!(version_from_name(name), None);
        }
    }

    #[test]
    fn overlays_preserve_older_blocks_and_replace_texture_arrays() {
        let mut base = json!({"stone":{"textures":"stone"},"wood":{"textures":{"up":"old_top","side":"old_side"}},"texture_data":{"wood":{"textures":["old_0","old_1"]}}});
        merge(
            &mut base,
            json!({"wood":{"textures":{"up":"new_top"}},"texture_data":{"wood":{"textures":["new"]}}}),
        );
        assert_eq!(base["stone"]["textures"], "stone");
        assert_eq!(base["wood"]["textures"]["up"], "new_top");
        assert_eq!(base["wood"]["textures"]["side"], "old_side");
        assert_eq!(base["texture_data"]["wood"]["textures"], json!(["new"]));
    }

    #[test]
    fn installed_stack_resolves_json_and_textures_from_the_latest_version() {
        let root =
            std::env::temp_dir().join(format!("bedrock-palette-stack-{}", std::process::id()));
        fs::create_dir_all(&root).expect("fixture root");
        for layer in [
            "vanilla",
            "vanilla_base",
            "vanilla_1.21.90",
            "vanilla_1.21.100",
            "beta",
        ] {
            fs::create_dir_all(root.join(layer)).expect("fixture layer");
        }
        fs::write(
            root.join("vanilla/blocks.json"),
            r#"{"stone":{"textures":"stone"},"wood":{"textures":"old"}}"#,
        )
        .expect("base JSON");
        fs::write(
            root.join("vanilla_1.21.90/blocks.json"),
            r#"{"wood":{"textures":"ninety"}}"#,
        )
        .expect("older JSON");
        fs::write(
            root.join("vanilla_1.21.100/blocks.json"),
            r#"{"wood":{"textures":"hundred"}}"#,
        )
        .expect("newer JSON");
        for layer in ["vanilla", "vanilla_1.21.90", "vanilla_1.21.100", "beta"] {
            fs::write(root.join(layer).join("wood.png"), layer).expect("texture marker");
        }
        let pack = ResourcePack::open(&root.join("vanilla")).expect("open installed stack");
        let blocks = pack.json(&["blocks.json"]).expect("merged JSON");
        assert_eq!(blocks["stone"]["textures"], "stone");
        assert_eq!(blocks["wood"]["textures"], "hundred");
        assert_eq!(
            pack.join("wood.png"),
            root.join("vanilla_1.21.100/wood.png")
        );
        assert_eq!(pack.version_label().as_deref(), Some("1.21.100"));
        fs::remove_dir_all(&root).expect("remove owned fixture");
    }
}
