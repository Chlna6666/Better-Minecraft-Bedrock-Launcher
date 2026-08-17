//! Discovery helpers for locating Bedrock world folders on disk.

use crate::error::Result;
use crate::level_dat::read_level_dat_document;
use crate::nbt::NbtTag;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

const SIZE_SCAN_FILE_LIMIT: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Configuration for filesystem world discovery.
pub struct WorldDiscovery {
    /// Root directories to search.
    pub roots: Vec<PathBuf>,
    /// Maximum number of files considered when estimating world size.
    pub size_scan_file_limit: usize,
}

impl WorldDiscovery {
    #[must_use]
    /// Creates discovery options for the given root directories.
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            size_scan_file_limit: SIZE_SCAN_FILE_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
/// Summary metadata for a discovered Bedrock world folder.
pub struct WorldSummary {
    /// Folder name containing the world.
    pub folder_name: String,
    /// Path to the world folder.
    pub folder_path: PathBuf,
    /// Display name from `levelname.txt` or `level.dat`.
    pub level_name: Option<String>,
    /// Optional `world_icon.*` path.
    pub icon_path: Option<PathBuf>,
    /// Last modified timestamp for the folder.
    pub modified: Option<SystemTime>,
    /// Approximate folder size when the scan stays within the file limit.
    pub size_bytes: Option<u64>,
    /// Parsed `world_behavior_packs.json` content.
    pub behavior_packs: Option<Value>,
    /// Parsed `world_resource_packs.json` content.
    pub resource_packs: Option<Value>,
    /// Number of behavior pack entries.
    pub behavior_packs_count: Option<usize>,
    /// Number of resource pack entries.
    pub resource_packs_count: Option<usize>,
    /// Root directory that yielded this world.
    pub source_root: PathBuf,
}

/// Discovers Bedrock worlds directly under the configured roots.
pub fn discover_worlds(options: &WorldDiscovery) -> Result<Vec<WorldSummary>> {
    let mut folders = Vec::new();
    for root in &options.roots {
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("level.dat").exists() {
                    folders.push((path, root.clone()));
                }
            }
        }
    }

    let mut worlds = Vec::with_capacity(folders.len());
    for (folder_path, source_root) in folders {
        let Some(folder_name) = folder_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
        else {
            continue;
        };
        if folder_name.starts_with('.') {
            continue;
        }

        let level_name = read_level_name(&folder_path);
        let icon_path = find_world_icon(&folder_path);
        let modified = fs::metadata(&folder_path)
            .and_then(|metadata| metadata.modified())
            .ok();
        let size_bytes = dir_size_limited(&folder_path, options.size_scan_file_limit);
        let behavior_packs = read_json_file(&folder_path.join("world_behavior_packs.json"));
        let resource_packs = read_json_file(&folder_path.join("world_resource_packs.json"));
        let behavior_packs_count = behavior_packs.as_ref().map(count_packs);
        let resource_packs_count = resource_packs.as_ref().map(count_packs);

        worlds.push(WorldSummary {
            folder_name,
            folder_path,
            level_name,
            icon_path,
            modified,
            size_bytes,
            behavior_packs,
            resource_packs,
            behavior_packs_count,
            resource_packs_count,
            source_root,
        });
    }
    worlds.sort_by_key(|world| std::cmp::Reverse(world.modified));
    Ok(worlds)
}

fn read_level_name(folder_path: &Path) -> Option<String> {
    let text_name = fs::read_to_string(folder_path.join("levelname.txt"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if text_name.is_some() {
        return text_name;
    }
    let document = read_level_dat_document(&folder_path.join("level.dat")).ok()?;
    let NbtTag::Compound(root) = document.root else {
        return None;
    };
    match root.get("LevelName") {
        Some(NbtTag::String(name)) if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}

fn find_world_icon(folder_path: &Path) -> Option<PathBuf> {
    ["world_icon.jpeg", "world_icon.jpg", "world_icon.png"]
        .iter()
        .map(|file_name| folder_path.join(file_name))
        .find(|path| path.exists())
}

fn read_json_file(path: &Path) -> Option<Value> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn dir_size_limited(path: &Path, file_limit: usize) -> Option<u64> {
    let mut total = 0_u64;
    let mut file_count = 0_usize;
    for entry in WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        let metadata = entry.metadata().ok()?;
        if metadata.is_file() {
            file_count = file_count.saturating_add(1);
            if file_count > file_limit {
                return None;
            }
            total = total.saturating_add(metadata.len());
        }
    }
    Some(total)
}

fn count_packs(value: &Value) -> usize {
    match value {
        Value::Array(values) => values.len(),
        Value::Object(values) => values
            .get("entries")
            .or_else(|| values.get("packs"))
            .and_then(Value::as_array)
            .map_or(values.len(), Vec::len),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level_dat::{LevelDatDocument, write_level_dat_document};
    use indexmap::IndexMap;
    use std::io::Write as _;

    #[test]
    fn discovers_level_dat_worlds() {
        let root = std::env::temp_dir().join(format!(
            "bedrock-world-discover-{}",
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let world = root.join("world");
        fs::create_dir_all(&world).expect("create world");
        fs::File::create(world.join("levelname.txt"))
            .expect("create levelname")
            .write_all(b"Test World")
            .expect("write levelname");
        let mut level = IndexMap::new();
        level.insert(
            "LevelName".to_string(),
            NbtTag::String("Fallback".to_string()),
        );
        write_level_dat_document(
            &world.join("level.dat"),
            &LevelDatDocument::new(10, NbtTag::Compound(level)),
        )
        .expect("write level.dat");

        let worlds = discover_worlds(&WorldDiscovery::new(vec![root.clone()])).expect("discover");
        assert_eq!(worlds.len(), 1);
        assert_eq!(worlds[0].level_name.as_deref(), Some("Test World"));

        fs::remove_dir_all(root).expect("cleanup");
    }
}
