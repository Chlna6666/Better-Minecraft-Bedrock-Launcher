use anyhow::{Context as _, Result, anyhow};
use gpui::DefaultFontConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const DEFAULT_APP_FONT_FAMILY: &str = ".SystemUIFont";

pub fn default_app_font_display() -> String {
    "Default(System UI)".to_string()
}

pub fn default_app_font_config() -> DefaultFontConfig {
    DefaultFontConfig::system_family(DEFAULT_APP_FONT_FAMILY)
}

pub fn font_config_for_selection(
    font_source: &str,
    local_font_path: &str,
    local_font_family: &str,
    system_font_family: &str,
) -> DefaultFontConfig {
    match crate::config::config::normalize_font_source(font_source).as_str() {
        crate::config::config::FONT_SOURCE_LOCAL => {
            let path = local_font_path.trim();
            if path.is_empty() || !Path::new(path).is_file() {
                return default_app_font_config();
            }

            let family = non_empty(local_font_family)
                .or_else(|| font_family_from_path(path))
                .or_else(|| font_family_fallback_from_path(path))
                .unwrap_or_else(|| DEFAULT_APP_FONT_FAMILY.to_string());
            local_font_paths_for_family(path, &family)
                .into_iter()
                .fold(DefaultFontConfig::system_family(family), |config, path| {
                    config.with_path(path)
                })
        }
        crate::config::config::FONT_SOURCE_SYSTEM => {
            if let Some(family) = non_empty(system_font_family) {
                DefaultFontConfig::system_family(family)
            } else {
                default_app_font_config()
            }
        }
        _ => default_app_font_config(),
    }
}

pub fn read_local_font_family(path: &str) -> Result<String> {
    let path = path.trim();
    if path.is_empty() {
        return Err(anyhow!("字体路径为空"));
    }

    let bytes = std::fs::read(path).with_context(|| format!("读取字体文件失败: {path}"))?;
    Ok(font_family_from_bytes(&bytes)
        .or_else(|| font_family_fallback_from_path(path))
        .unwrap_or_else(|| DEFAULT_APP_FONT_FAMILY.to_string()))
}

pub fn is_system_font_family(family: &str) -> bool {
    system_font_paths_for_family(family).is_some()
}

fn system_font_paths_for_family(family: &str) -> Option<Vec<PathBuf>> {
    let key = family_catalog_key(family)?;
    system_font_catalog().paths_by_family.get(&key).cloned()
}

fn local_font_paths_for_family(path: &str, family: &str) -> Vec<PathBuf> {
    let primary_path = PathBuf::from(path);
    let Some(parent) = primary_path.parent() else {
        return vec![primary_path];
    };
    let Some(key) = family_catalog_key(family) else {
        return vec![primary_path];
    };

    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(parent)
        .follow_links(false)
        .max_depth(1)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let candidate = entry.path();
        if !is_supported_font_path(candidate) {
            continue;
        }

        let Some(candidate_family) = font_family_from_path(candidate) else {
            continue;
        };
        if family_catalog_key(&candidate_family).as_deref() != Some(key.as_str()) {
            continue;
        }

        let candidate = candidate.to_path_buf();
        if !paths.iter().any(|existing| existing == &candidate) {
            paths.push(candidate);
        }
    }

    if !paths.iter().any(|existing| existing == &primary_path) {
        paths.push(primary_path);
    }
    paths
}

pub fn font_family_from_path(path: impl AsRef<Path>) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    font_family_from_bytes(&bytes)
}

pub fn font_family_from_bytes(bytes: &[u8]) -> Option<String> {
    font_families_from_bytes(bytes).into_iter().next()
}

fn font_families_from_bytes(bytes: &[u8]) -> Vec<String> {
    let face_count = ttf_parser::fonts_in_collection(bytes).unwrap_or(1).max(1);
    let mut families = Vec::new();
    for index in 0..face_count {
        if let Ok(face) = ttf_parser::Face::parse(bytes, index) {
            if let Some(family) = font_family_from_face(&face) {
                families.push(family);
            }
        }
    }
    families.sort_by_key(|family| family.to_ascii_lowercase());
    families.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    families
}

struct SystemFontCatalog {
    families: Vec<String>,
    paths_by_family: HashMap<String, Vec<PathBuf>>,
}

fn system_font_catalog() -> &'static SystemFontCatalog {
    static SYSTEM_FONT_CATALOG: OnceLock<SystemFontCatalog> = OnceLock::new();
    SYSTEM_FONT_CATALOG.get_or_init(scan_system_font_catalog)
}

fn scan_system_font_catalog() -> SystemFontCatalog {
    #[cfg(target_os = "windows")]
    {
        let catalog = scan_windows_font_registry_catalog();
        if !catalog.families.is_empty() {
            return catalog;
        }
    }

    scan_system_font_file_catalog()
}

fn scan_system_font_file_catalog() -> SystemFontCatalog {
    let mut families = Vec::new();
    let mut paths_by_family = HashMap::new();

    for dir in system_font_dirs() {
        if !dir.is_dir() {
            continue;
        }

        for entry in walkdir::WalkDir::new(dir)
            .follow_links(false)
            .max_depth(4)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            if !is_supported_font_path(entry.path()) {
                continue;
            }

            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };

            for family in font_families_from_bytes(&bytes) {
                add_font_family_path(
                    &mut families,
                    &mut paths_by_family,
                    family,
                    entry.path().to_path_buf(),
                );
            }
        }
    }

    families.sort_by_key(|family| family.to_ascii_lowercase());
    families.dedup_by(|left, right| left.eq_ignore_ascii_case(right));

    SystemFontCatalog {
        families,
        paths_by_family,
    }
}

fn add_font_family_path(
    families: &mut Vec<String>,
    paths_by_family: &mut HashMap<String, Vec<PathBuf>>,
    family: String,
    path: PathBuf,
) {
    let Some(key) = family_catalog_key(&family) else {
        return;
    };

    let paths = paths_by_family.entry(key).or_default();
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
    families.push(family);
}

#[cfg(target_os = "windows")]
fn scan_windows_font_registry_catalog() -> SystemFontCatalog {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    const FONTS_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts";

    let mut families = Vec::new();
    let mut paths_by_family = HashMap::new();

    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let root_key = RegKey::predef(root);
        read_windows_font_registry_key(
            &root_key,
            root,
            FONTS_KEY,
            &mut families,
            &mut paths_by_family,
        );
    }

    families.sort_by_key(|family| family.to_ascii_lowercase());
    families.dedup_by(|left, right| left.eq_ignore_ascii_case(right));

    SystemFontCatalog {
        families,
        paths_by_family,
    }
}

#[cfg(target_os = "windows")]
fn read_windows_font_registry_key(
    root_key: &winreg::RegKey,
    root: winreg::HKEY,
    path: &str,
    families: &mut Vec<String>,
    paths_by_family: &mut HashMap<String, Vec<PathBuf>>,
) {
    use winreg::enums::{KEY_READ, KEY_WOW64_64KEY};

    let key = root_key
        .open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY)
        .or_else(|_| root_key.open_subkey_with_flags(path, KEY_READ));
    let Ok(key) = key else {
        return;
    };

    for entry in key.enum_values().filter_map(std::result::Result::ok) {
        let (display_name, _) = entry;
        let Ok(path_value) = key.get_value::<String, _>(&display_name) else {
            continue;
        };
        let Some(font_path) = resolve_windows_font_registry_path(root, &path_value) else {
            continue;
        };

        for family in font_family_names_from_registry_value(&display_name) {
            add_font_family_path(families, paths_by_family, family, font_path.clone());
        }
    }
}

#[cfg(target_os = "windows")]
fn resolve_windows_font_registry_path(root: winreg::HKEY, registry_value: &str) -> Option<PathBuf> {
    use winreg::enums::HKEY_CURRENT_USER;

    let expanded = expand_known_windows_font_path_prefix(registry_value);
    let candidate = PathBuf::from(expanded);
    let candidates = if candidate.is_absolute() {
        vec![candidate]
    } else {
        let mut dirs = Vec::new();
        if root == HKEY_CURRENT_USER {
            if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
                dirs.push(PathBuf::from(local_app_data).join(r"Microsoft\Windows\Fonts"));
            }
            if let Some(app_data) = std::env::var_os("APPDATA") {
                dirs.push(PathBuf::from(app_data).join(r"Microsoft\Windows\Fonts"));
            }
        }
        dirs.extend(system_font_dirs());
        dirs.into_iter()
            .map(|dir| dir.join(&candidate))
            .collect::<Vec<_>>()
    };

    candidates
        .into_iter()
        .find(|path| is_supported_font_path(path) && path.is_file())
}

#[cfg(target_os = "windows")]
fn expand_known_windows_font_path_prefix(value: &str) -> String {
    let trimmed = value.trim().trim_matches('"');
    for (token, variable) in [
        ("%SystemRoot%", "SYSTEMROOT"),
        ("%WINDIR%", "WINDIR"),
        ("%LOCALAPPDATA%", "LOCALAPPDATA"),
        ("%APPDATA%", "APPDATA"),
    ] {
        if trimmed.len() < token.len() || !trimmed[..token.len()].eq_ignore_ascii_case(token) {
            continue;
        }
        let Some(root) = std::env::var_os(variable) else {
            continue;
        };
        let remainder = trimmed[token.len()..].trim_start_matches(['\\', '/']);
        return PathBuf::from(root)
            .join(remainder)
            .to_string_lossy()
            .into_owned();
    }
    trimmed.to_string()
}

fn font_family_names_from_registry_value(display_name: &str) -> Vec<String> {
    let trimmed = display_name.trim();
    let family = trimmed
        .rfind(" (")
        .filter(|_| trimmed.ends_with(')'))
        .map_or(trimmed, |index| &trimmed[..index])
        .trim();

    let mut families = Vec::new();
    push_unique_family_name(&mut families, family);
    if let Some(base_family) = strip_font_style_suffix(family) {
        push_unique_family_name(&mut families, base_family.as_str());
    }
    families
}

fn push_unique_family_name(families: &mut Vec<String>, family: &str) {
    let Some(family) = non_empty(family) else {
        return;
    };
    if !families
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(&family))
    {
        families.push(family);
    }
}

fn strip_font_style_suffix(family: &str) -> Option<String> {
    const STYLE_SUFFIXES: &[&str] = &[
        " Bold Italic",
        " Bold Oblique",
        " Semi Bold Italic",
        " SemiBold Italic",
        " Semibold Italic",
        " Extra Bold Italic",
        " ExtraBold Italic",
        " Black Italic",
        " Light Italic",
        " Regular",
        " Bold",
        " Italic",
        " Oblique",
        " Semi Bold",
        " SemiBold",
        " Semibold",
        " Demi Bold",
        " DemiBold",
        " Extra Bold",
        " ExtraBold",
        " Ultra Bold",
        " UltraBold",
        " Medium",
        " Light",
        " Thin",
        " Black",
        " Heavy",
        " Condensed",
    ];

    let lower_family = family.to_ascii_lowercase();
    for suffix in STYLE_SUFFIXES {
        let lower_suffix = suffix.to_ascii_lowercase();
        if !lower_family.ends_with(&lower_suffix) || family.len() <= suffix.len() {
            continue;
        }
        let base = family[..family.len() - suffix.len()].trim();
        if let Some(base) = non_empty(base) {
            return Some(base);
        }
    }
    None
}

fn is_supported_font_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "ttf" | "otf" | "ttc" | "otc"
            )
        })
}

#[cfg(target_os = "windows")]
fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(system_root) = std::env::var_os("SYSTEMROOT") {
        dirs.push(PathBuf::from(system_root).join("Fonts"));
    } else {
        dirs.push(PathBuf::from(r"C:\Windows\Fonts"));
    }

    if let Some(user_profile) = std::env::var_os("USERPROFILE") {
        let user_profile = PathBuf::from(user_profile);
        dirs.push(user_profile.join(r"AppData\Local\Microsoft\Windows\Fonts"));
        dirs.push(user_profile.join(r"AppData\Roaming\Microsoft\Windows\Fonts"));
    }

    dirs
}

#[cfg(target_os = "macos")]
fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/Library/Fonts"),
        PathBuf::from("/System/Library/Fonts"),
        PathBuf::from("/Network/Library/Fonts"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Library/Fonts"));
    }
    dirs
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".fonts"));
        dirs.push(home.join(".local/share/fonts"));
    }
    dirs
}

fn font_family_from_face(face: &ttf_parser::Face<'_>) -> Option<String> {
    [
        ttf_parser::name_id::TYPOGRAPHIC_FAMILY,
        ttf_parser::name_id::FAMILY,
        ttf_parser::name_id::FULL_NAME,
    ]
    .into_iter()
    .find_map(|name_id| {
        face.names()
            .into_iter()
            .filter(|name| name.name_id == name_id)
            .find_map(|name| name.to_string().and_then(non_empty))
    })
}

fn font_family_fallback_from_path(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(non_empty)
}

fn non_empty(value: impl AsRef<str>) -> Option<String> {
    let trimmed = value.as_ref().trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn family_catalog_key(value: &str) -> Option<String> {
    non_empty(value).map(|value| value.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_font_selection_uses_native_family_without_file_registration() {
        let config = font_config_for_selection(
            crate::config::config::FONT_SOURCE_SYSTEM,
            "",
            "",
            "Segoe UI",
        );

        assert_eq!(config.family.as_ref(), "Segoe UI");
        assert!(config.sources.is_empty());
    }

    #[test]
    fn registry_font_name_strips_file_type_suffix() {
        assert_eq!(
            font_family_names_from_registry_value("Arial (TrueType)"),
            vec!["Arial".to_string()]
        );
    }

    #[test]
    fn registry_font_name_keeps_style_and_base_family() {
        assert_eq!(
            font_family_names_from_registry_value("Arial Bold Italic (TrueType)"),
            vec!["Arial Bold Italic".to_string(), "Arial".to_string()]
        );
    }
}
