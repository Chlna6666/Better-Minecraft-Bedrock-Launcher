use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter};
use std::path::{Component, Path, PathBuf};

use zip::ZipArchive;

use super::lip::{AssetPlacement, PlacementKind};

pub(super) fn install_zip_asset(
    archive_path: &Path,
    game_directory: &Path,
    placements: &[AssetPlacement],
    strip_archive_root: bool,
) -> Result<Vec<PathBuf>, String> {
    let archive_file = File::open(archive_path)
        .map_err(|error| format!("打开 Lip 资产失败 {}: {error}", archive_path.display()))?;
    let mut archive = ZipArchive::new(BufReader::new(archive_file))
        .map_err(|error| format!("解析 Lip ZIP 失败 {}: {error}", archive_path.display()))?;
    let mut installed_files = Vec::new();

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("读取 Lip ZIP 条目失败: {error}"))?;
        if entry.is_dir() {
            continue;
        }
        let enclosed = entry.enclosed_name().ok_or_else(|| {
            format!(
                "Lip ZIP 包含不安全路径: {}",
                String::from_utf8_lossy(entry.name_raw())
            )
        })?;
        let package_path = normalized_archive_path(&enclosed, strip_archive_root)?;
        for destination in placement_destinations(&package_path, placements)? {
            let destination = safe_destination(game_directory, &destination)?;
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!("创建 Lip 安装目录失败 {}: {error}", parent.display())
                })?;
            }
            let target = File::create(&destination).map_err(|error| {
                format!("创建 Lip 安装文件失败 {}: {error}", destination.display())
            })?;
            io::copy(&mut entry, &mut BufWriter::new(target)).map_err(|error| {
                format!("写入 Lip 安装文件失败 {}: {error}", destination.display())
            })?;
            installed_files.push(destination);
        }
    }

    installed_files.sort();
    installed_files.dedup();
    Ok(installed_files)
}

pub(super) fn install_uncompressed_asset(
    source: &Path,
    game_directory: &Path,
    placements: &[AssetPlacement],
) -> Result<Vec<PathBuf>, String> {
    let destinations = placement_destinations("", placements)?;
    let mut installed_files = Vec::with_capacity(destinations.len());
    for destination in destinations {
        let destination = safe_destination(game_directory, &destination)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("创建 Lip 安装目录失败: {error}"))?;
        }
        fs::copy(source, &destination).map_err(|error| {
            format!(
                "复制 Lip 资产失败 {} -> {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
        installed_files.push(destination);
    }
    Ok(installed_files)
}

fn normalized_archive_path(path: &Path, strip_archive_root: bool) -> Result<String, String> {
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str().map(ToOwned::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>();
    let start = usize::from(strip_archive_root);
    if components.len() <= start {
        return Err(format!("Lip ZIP 条目路径无效: {}", path.display()));
    }
    Ok(components[start..].join("/"))
}

fn placement_destinations(
    package_path: &str,
    placements: &[AssetPlacement],
) -> Result<Vec<PathBuf>, String> {
    let package_path = package_path.replace('\\', "/");
    let mut destinations = Vec::new();
    for placement in placements {
        validate_relative_path(&placement.destination)?;
        match placement.kind {
            PlacementKind::Dir => {
                let source = placement.src.trim_start_matches('/').replace('\\', "/");
                let source = source.trim_end_matches('/');
                let relative = if source.is_empty() {
                    Some(package_path.as_str())
                } else {
                    package_path
                        .strip_prefix(source)
                        .and_then(|value| value.strip_prefix('/').or(Some(value)))
                };
                if let Some(relative) = relative.filter(|value| !value.is_empty()) {
                    destinations.push(Path::new(&placement.destination).join(relative));
                }
            }
            PlacementKind::File if file_pattern_matches(&placement.src, &package_path) => {
                let destination = Path::new(&placement.destination);
                let final_path = if placement.src.contains('*') || placement.src.contains('?') {
                    let file_name = Path::new(&package_path)
                        .file_name()
                        .ok_or_else(|| format!("Lip placement 源路径无文件名: {package_path}"))?;
                    destination.join(file_name)
                } else {
                    destination.to_path_buf()
                };
                destinations.push(final_path);
            }
            PlacementKind::File => {}
        }
    }
    Ok(destinations)
}

fn file_pattern_matches(pattern: &str, package_path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    if !pattern.contains(['*', '?']) {
        return pattern == package_path;
    }
    wildcard_matches(pattern.as_bytes(), package_path.as_bytes())
}

fn wildcard_matches(pattern: &[u8], value: &[u8]) -> bool {
    let (mut pattern_index, mut value_index) = (0, 0);
    let (mut star_index, mut star_value_index) = (None, 0);
    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }
    pattern[pattern_index..]
        .iter()
        .all(|character| *character == b'*')
}

fn safe_destination(game_directory: &Path, relative: &Path) -> Result<PathBuf, String> {
    validate_relative_path(&relative.to_string_lossy())?;
    Ok(game_directory.join(relative))
}

fn validate_relative_path(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("Lip placement 目标路径不安全: {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
