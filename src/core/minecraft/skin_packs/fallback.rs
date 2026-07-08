use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::warn;

use super::McSkinPackSkinInfo;
use crate::core::minecraft::skin_pack_preview::{generate_skin_preview, skin_texture_dimensions};

const SKIN_MIN_TEXTURE_WIDTH: u32 = 64;
const SKIN_MIN_TEXTURE_HEIGHT: u32 = 32;
const PARALLEL_FALLBACK_PREVIEW_THRESHOLD: usize = 12;

pub(super) fn skin_infos_from_pngs(folder_path: &Path) -> Vec<McSkinPackSkinInfo> {
    let texture_paths = skin_texture_paths(folder_path);
    if texture_paths.len() < PARALLEL_FALLBACK_PREVIEW_THRESHOLD {
        return texture_paths
            .iter()
            .map(|path| skin_info_from_texture_path(path))
            .collect();
    }

    texture_paths
        .par_iter()
        .map(|path| skin_info_from_texture_path(path))
        .collect()
}

fn skin_texture_paths(folder_path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(folder_path) else {
        return Vec::new();
    };

    let mut texture_paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| is_png_file(path))
        .filter(|path| !is_pack_artwork(path))
        .filter(|path| is_probable_skin_texture(path))
        .collect::<Vec<_>>();
    texture_paths.sort_by_key(|path| {
        path.file_name()
            .map(|file_name| file_name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    });
    texture_paths
}

fn skin_info_from_texture_path(texture_path: &Path) -> McSkinPackSkinInfo {
    let preview_path = match generate_skin_preview(texture_path) {
        Ok(path) => Some(path.to_string_lossy().to_string()),
        Err(error) => {
            warn!(
                "generate fallback skin preview failed {}: {error:?}",
                texture_path.display()
            );
            None
        }
    };

    McSkinPackSkinInfo {
        display_name: display_name_from_texture_path(texture_path),
        localization_name: None,
        full_texture_path: Some(texture_path.to_string_lossy().to_string()),
        preview_path,
        model_label: model_label_from_texture_path(texture_path),
        geometry_path: None,
        geometry_identifier: None,
    }
}

fn is_png_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
}

fn is_pack_artwork(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return true;
    };
    let stem = stem.to_ascii_lowercase();
    stem == "pack_icon" || stem.contains("cape")
}

fn is_probable_skin_texture(path: &Path) -> bool {
    skin_texture_dimensions(path)
        .map(|(width, height)| is_skin_texture_size(width, height))
        .unwrap_or(false)
}

fn is_skin_texture_size(width: u32, height: u32) -> bool {
    width >= SKIN_MIN_TEXTURE_WIDTH
        && height >= SKIN_MIN_TEXTURE_HEIGHT
        && width % SKIN_MIN_TEXTURE_WIDTH == 0
        && (height == width || height.saturating_mul(2) == width)
}

fn display_name_from_texture_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| {
            stem.split(['_', '-'])
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Skin".to_string())
}

fn model_label_from_texture_path(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if stem.contains("alex") || stem.contains("slim") {
        "Alex".to_string()
    } else {
        "Steve".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_texture_size_accepts_standard_and_hd_layouts() {
        for (width, height) in [(64, 32), (64, 64), (128, 64), (128, 128), (256, 128)] {
            assert!(
                is_skin_texture_size(width, height),
                "expected {width}x{height} to be accepted"
            );
        }
    }

    #[test]
    fn skin_texture_size_rejects_pack_icons_and_invalid_ratios() {
        for (width, height) in [(32, 32), (64, 16), (128, 32), (128, 96)] {
            assert!(
                !is_skin_texture_size(width, height),
                "expected {width}x{height} to be rejected"
            );
        }
    }

    #[test]
    fn display_name_uses_texture_file_stem() {
        assert_eq!(
            display_name_from_texture_path(Path::new("rainbow_alex-slim.png")),
            "rainbow alex slim"
        );
    }
}
