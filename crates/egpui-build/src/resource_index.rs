use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use egpui_manifest::ResourceManifest;
use sha2::{Digest, Sha256};
use thiserror::Error;
use walkdir::WalkDir;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ResourceSource {
    Embedded,
    Bundled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedResource {
    pub logical_path: String,
    pub source: ResourceSource,
    pub filesystem_path: PathBuf,
    pub byte_length: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceIndex {
    entries: Vec<IndexedResource>,
}

impl ResourceIndex {
    #[must_use]
    pub fn entries(&self) -> &[IndexedResource] {
        &self.entries
    }

    #[must_use]
    pub fn by_logical_path(&self) -> BTreeMap<&str, &IndexedResource> {
        self.entries
            .iter()
            .map(|entry| (entry.logical_path.as_str(), entry))
            .collect()
    }

    /// Returns only resources declared for compile-time embedding.
    pub fn embedded_entries(&self) -> impl Iterator<Item = &IndexedResource> {
        self.entries
            .iter()
            .filter(|entry| entry.source == ResourceSource::Embedded)
    }
}

#[derive(Debug, Error)]
pub enum ResourceIndexError {
    #[error("resource root `{0}` does not exist or is not a directory")]
    InvalidRoot(PathBuf),
    #[error("failed to walk resource root: {0}")]
    Walk(#[from] walkdir::Error),
    #[error("failed to read resource `{path}`: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("resource logical path collision: `{0}`")]
    Collision(String),
    #[error("resource pattern `{pattern}` matched no files")]
    EmptyPattern { pattern: String },
}

/// Builds a stable index from the declared embedded and bundled patterns.
pub fn build_resource_index(
    project_root: impl AsRef<Path>,
    manifest: &ResourceManifest,
) -> Result<ResourceIndex, ResourceIndexError> {
    let project_root = project_root.as_ref();
    if !project_root.is_dir() {
        return Err(ResourceIndexError::InvalidRoot(project_root.to_owned()));
    }

    let mut entries = BTreeMap::<String, IndexedResource>::new();
    let mut case_folded_paths = BTreeMap::<String, String>::new();
    for (source, patterns) in [
        (ResourceSource::Embedded, &manifest.embedded),
        (ResourceSource::Bundled, &manifest.bundled),
    ] {
        for pattern in patterns {
            let matched = collect_pattern(
                project_root,
                pattern,
                source,
                &mut entries,
                &mut case_folded_paths,
            )?;
            if !matched {
                return Err(ResourceIndexError::EmptyPattern {
                    pattern: pattern.clone(),
                });
            }
        }
    }
    Ok(ResourceIndex {
        entries: entries.into_values().collect(),
    })
}

/// Renders a deterministic Rust module containing `include_bytes!` entries.
///
/// The generated paths are relative to `CARGO_MANIFEST_DIR`, so build output
/// does not embed a developer-specific absolute workspace path.
#[must_use]
pub fn render_embedded_resource_module(index: &ResourceIndex) -> String {
    let mut output = String::from("pub static EGPUI_EMBEDDED_RESOURCES: &[(&str, &[u8])] = &[\n");
    for entry in index.embedded_entries() {
        let logical_path = format!("{:?}", entry.logical_path);
        output.push_str("    (");
        output.push_str(&logical_path);
        output.push_str(", include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/");
        output.push_str(&escape_rust_path(&entry.logical_path));
        output.push_str("\"))),\n");
    }
    output.push_str("];\n");
    output
}

fn collect_pattern(
    project_root: &Path,
    pattern: &str,
    source: ResourceSource,
    entries: &mut BTreeMap<String, IndexedResource>,
    case_folded_paths: &mut BTreeMap<String, String>,
) -> Result<bool, ResourceIndexError> {
    let mut matched = false;
    for item in WalkDir::new(project_root).follow_links(false) {
        let item = item?;
        if !item.file_type().is_file() {
            continue;
        }
        let relative =
            item.path()
                .strip_prefix(project_root)
                .map_err(|_| ResourceIndexError::Read {
                    path: item.path().to_owned(),
                    source: io::Error::new(io::ErrorKind::InvalidData, "path escaped project root"),
                })?;
        let logical_path = normalize_path(relative);
        if !matches_pattern(&logical_path, pattern) {
            continue;
        }
        matched = true;
        let path = item.path().to_owned();
        let metadata = std::fs::metadata(&path).map_err(|source| ResourceIndexError::Read {
            path: path.clone(),
            source,
        })?;
        let sha256 = hash_file(&path)?;
        let entry = IndexedResource {
            logical_path: logical_path.clone(),
            source,
            filesystem_path: path,
            byte_length: metadata.len(),
            sha256,
        };
        if let Some(existing) = entries.get(&logical_path) {
            if existing.filesystem_path == entry.filesystem_path && existing.source == entry.source
            {
                continue;
            }
            return Err(ResourceIndexError::Collision(logical_path));
        }
        let case_folded = logical_path.to_lowercase();
        if let Some(existing) = case_folded_paths.get(&case_folded) {
            if existing != &logical_path {
                return Err(ResourceIndexError::Collision(format!(
                    "{existing} conflicts with {logical_path} on case-insensitive filesystems"
                )));
            }
        }
        case_folded_paths.insert(case_folded, logical_path.clone());
        entries.insert(logical_path, entry);
    }
    Ok(matched)
}

fn hash_file(path: &Path) -> Result<String, ResourceIndexError> {
    let mut file = File::open(path).map_err(|source| ResourceIndexError::Read {
        path: path.to_owned(),
        source,
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| ResourceIndexError::Read {
                path: path.to_owned(),
                source,
            })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn normalize_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn escape_rust_path(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"")
}

fn matches_pattern(path: &str, pattern: &str) -> bool {
    let path_parts = path.split('/').collect::<Vec<_>>();
    let pattern_parts = pattern.replace('\\', "/");
    let pattern_parts = pattern_parts.split('/').collect::<Vec<_>>();
    matches_parts(&path_parts, &pattern_parts)
}

fn matches_parts(path: &[&str], pattern: &[&str]) -> bool {
    match (path.first(), pattern.first()) {
        (None, None) => true,
        (_, Some(&"**")) => {
            matches_parts(path, &pattern[1..])
                || path
                    .first()
                    .is_some_and(|_| matches_parts(&path[1..], pattern))
        }
        (Some(path_part), Some(pattern_part)) => {
            matches_segment(path_part, pattern_part) && matches_parts(&path[1..], &pattern[1..])
        }
        _ => false,
    }
}

fn matches_segment(value: &str, pattern: &str) -> bool {
    let value = value.chars().collect::<Vec<_>>();
    let pattern = pattern.chars().collect::<Vec<_>>();
    let mut previous = vec![false; pattern.len().saturating_add(1)];
    previous[0] = true;
    for (index, pattern_character) in pattern.iter().enumerate() {
        if *pattern_character == '*' {
            previous[index.saturating_add(1)] = previous[index];
        }
    }
    for value_character in value {
        let mut current = vec![false; pattern.len().saturating_add(1)];
        for (index, pattern_character) in pattern.iter().enumerate() {
            current[index.saturating_add(1)] = if *pattern_character == '*' {
                current[index] || previous[index.saturating_add(1)]
            } else {
                (*pattern_character == '?' || *pattern_character == value_character)
                    && previous[index]
            };
        }
        previous = current;
    }
    previous[pattern.len()]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        IndexedResource, ResourceIndex, ResourceSource, matches_pattern, matches_segment,
        render_embedded_resource_module,
    };

    #[test]
    fn wildcard_matching_is_unicode_safe() {
        assert!(matches_segment("图标-save.svg", "*-*.svg"));
        assert!(matches_segment("save.svg", "*.svg"));
        assert!(matches_segment("save.svg", "s?ve.*"));
        assert!(!matches_segment("save.png", "*.svg"));
    }

    #[test]
    fn recursive_patterns_match_nested_files() {
        assert!(matches_pattern(
            "assets/icons/actions/save.svg",
            "assets/**/*.svg"
        ));
        assert!(matches_pattern("assets/save.svg", "assets/**/*.svg"));
        assert!(!matches_pattern("other/save.svg", "assets/**/*.svg"));
    }

    #[test]
    fn generated_module_uses_manifest_relative_paths() {
        let index = ResourceIndex {
            entries: vec![IndexedResource {
                logical_path: "assets/icons/save.svg".to_owned(),
                source: ResourceSource::Embedded,
                filesystem_path: PathBuf::from("C:/workspace/assets/icons/save.svg"),
                byte_length: 10,
                sha256: "digest".to_owned(),
            }],
        };
        let generated = render_embedded_resource_module(&index);
        assert!(generated.contains("env!(\"CARGO_MANIFEST_DIR\")"));
        assert!(generated.contains("/assets/icons/save.svg"));
        assert!(!generated.contains("C:/workspace"));
    }
}
