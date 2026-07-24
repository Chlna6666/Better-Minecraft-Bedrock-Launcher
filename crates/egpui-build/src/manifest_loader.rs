use std::{
    fs,
    path::{Path, PathBuf},
};

use egpui_manifest::{AppManifest, ManifestValidationError};
use schemars::schema_for;
use serde_json::Value as JsonValue;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ManifestLoadError {
    #[error("failed to read manifest `{path}`: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse manifest TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("manifest validation failed: {0}")]
    Validation(#[from] ManifestValidationError),
    #[error("failed to serialize manifest schema: {0}")]
    Schema(#[from] serde_json::Error),
}

/// Loads an `App.toml`, applying optional platform overlays in order.
///
/// Tables are merged recursively. Arrays and scalar values from a later
/// overlay replace the earlier value, making the result deterministic.
pub fn load_manifest(
    manifest_path: impl AsRef<Path>,
    overlay_paths: &[PathBuf],
) -> Result<AppManifest, ManifestLoadError> {
    let manifest_path = manifest_path.as_ref();
    let base = fs::read_to_string(manifest_path).map_err(|source| ManifestLoadError::Read {
        path: manifest_path.to_owned(),
        source,
    })?;
    let mut document = toml::from_str::<toml::Value>(&base)?;
    for overlay_path in overlay_paths {
        let overlay =
            fs::read_to_string(overlay_path).map_err(|source| ManifestLoadError::Read {
                path: overlay_path.clone(),
                source,
            })?;
        let overlay = toml::from_str::<toml::Value>(&overlay)?;
        merge_toml(&mut document, overlay);
    }
    let manifest = document.try_into::<AppManifest>()?;
    manifest.validate()?;
    Ok(manifest)
}

/// Parses and validates a manifest held by a caller.
pub fn load_manifest_from_str(source: &str) -> Result<AppManifest, ManifestLoadError> {
    let manifest = toml::from_str::<toml::Value>(source)?.try_into::<AppManifest>()?;
    manifest.validate()?;
    Ok(manifest)
}

/// Returns the JSON Schema for the supported `App.toml` model.
pub fn manifest_schema_json() -> Result<String, ManifestLoadError> {
    let schema = schema_for!(AppManifest);
    let value = serde_json::to_value(schema)?;
    let mut document = serde_json::Map::new();
    document.insert("schema".to_owned(), value);
    document.insert("schema_version".to_owned(), JsonValue::from(1));
    Ok(format!("{}\n", serde_json::to_string_pretty(&document)?))
}

fn merge_toml(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, value) in overlay_table {
                match base_table.get_mut(&key) {
                    Some(existing) => merge_toml(existing, value),
                    None => {
                        base_table.insert(key, value);
                    }
                }
            }
        }
        (base_value, overlay_value) => *base_value = overlay_value,
    }
}

#[cfg(test)]
mod tests {
    use super::{load_manifest_from_str, manifest_schema_json};

    const VALID: &str = r#"
schema_version = 1

[application]
id = "com.example.egpui"
name = "Example"
version = "1.0.0"
publisher = "Example"
default_locale = "en-US"
binary_name = "example"

[runtime]
provider = "tokio"
shutdown_timeout_seconds = 10
ui_queue_capacity = 16

[resources]
embedded = ["assets/**/*"]
bundled = ["bin/**/*"]
development_overlays = []

[i18n]
source_locale = "en-US"
locales = ["en-US", "zh-CN"]
catalog_pattern = "locales/{locale}/main.ftl"

[bundle]
targets = ["windows-portable"]
"#;

    #[test]
    fn loads_and_validates_manifest() {
        load_manifest_from_str(VALID).expect("valid manifest");
    }

    #[test]
    fn emits_versioned_schema() {
        let schema = manifest_schema_json().expect("schema");
        assert!(schema.contains("\"schema_version\": 1"));
        assert!(schema.contains("\"AppManifest\""));
    }
}
