use serde::{Deserialize, Serialize};
use std::fs as std_fs;
use std::path::{Path, PathBuf};
use tracing::info;

use super::game_info::write_json_atomically;

pub const VANILLA_SKIN_PACK_REDIRECTION_SOURCE: &str = r"data\skin_packs\vanilla";

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct FileRedirectionConfig {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VersionConfig {
    #[serde(default)]
    pub enable_debug_console: bool,
    #[serde(default)]
    pub enable_redirection: bool,
    #[serde(default)]
    pub editor_mode: bool,

    // Disable mod loading/injection (managed by BLoader.dll). Default: false (load mods).
    #[serde(default)]
    pub disable_mod_loading: bool,
    #[serde(default)]
    pub lock_mouse_on_launch: bool,
    #[serde(default = "default_unlock_hotkey")]
    pub unlock_mouse_hotkey: String,
    #[serde(default = "default_reduce_pixels")]
    pub reduce_pixels: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vanilla_skin_pack_redirect: Option<String>,
    #[serde(default)]
    pub file_redirections: Vec<FileRedirectionConfig>,
    #[serde(default = "default_true")]
    pub shortcut_silent_launch: bool,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct BmcblInstanceConfig {
    #[serde(default)]
    editor_mode: bool,
    #[serde(default)]
    lock_mouse_on_launch: bool,
    #[serde(default = "default_unlock_hotkey")]
    unlock_mouse_hotkey: String,
    #[serde(default = "default_reduce_pixels")]
    reduce_pixels: i32,
    #[serde(default = "default_true")]
    shortcut_silent_launch: bool,
    #[serde(default, flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for BmcblInstanceConfig {
    fn default() -> Self {
        Self {
            editor_mode: false,
            lock_mouse_on_launch: false,
            unlock_mouse_hotkey: default_unlock_hotkey(),
            reduce_pixels: default_reduce_pixels(),
            shortcut_silent_launch: true,
            extra: serde_json::Map::new(),
        }
    }
}

const BMCBL_CONFIG_KEYS: [&str; 5] = [
    "editor_mode",
    "lock_mouse_on_launch",
    "unlock_mouse_hotkey",
    "reduce_pixels",
    "shortcut_silent_launch",
];

fn default_true() -> bool {
    true
}

fn default_unlock_hotkey() -> String {
    "ALT".to_string()
}

fn default_reduce_pixels() -> i32 {
    20
}

impl Default for VersionConfig {
    fn default() -> Self {
        Self {
            enable_debug_console: false,
            enable_redirection: false,
            editor_mode: false,
            disable_mod_loading: false,
            lock_mouse_on_launch: false,
            unlock_mouse_hotkey: "ALT".to_string(),
            reduce_pixels: 20,
            vanilla_skin_pack_redirect: None,
            file_redirections: Vec::new(),
            shortcut_silent_launch: true,
            extra: serde_json::Map::new(),
        }
    }
}

impl VersionConfig {
    pub fn set_vanilla_skin_pack_redirect(&mut self, target: Option<String>) {
        self.file_redirections
            .retain(|redirection| !is_vanilla_skin_pack_redirection_source(&redirection.source));
        self.vanilla_skin_pack_redirect = target
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);

        if let Some(target) = self.vanilla_skin_pack_redirect.clone() {
            self.file_redirections.push(FileRedirectionConfig {
                source: VANILLA_SKIN_PACK_REDIRECTION_SOURCE.to_string(),
                target,
                kind: Some("directory".to_string()),
            });
        }
    }

    pub fn normalize_managed_redirections(&mut self) {
        if self.vanilla_skin_pack_redirect.is_none() {
            self.vanilla_skin_pack_redirect = self
                .file_redirections
                .iter()
                .find(|redirection| {
                    is_vanilla_skin_pack_redirection_source(&redirection.source)
                        && !redirection.target.trim().is_empty()
                })
                .map(|redirection| redirection.target.clone());
        }

        if let Some(target) = self.vanilla_skin_pack_redirect.clone() {
            self.set_vanilla_skin_pack_redirect(Some(target));
        }
    }

    pub fn effective_file_redirections(&self, package_folder: &Path) -> Vec<FileRedirectionConfig> {
        self.file_redirections
            .iter()
            .filter(|redirection| !redirection.source.trim().is_empty())
            .filter(|redirection| !redirection.target.trim().is_empty())
            .map(|redirection| FileRedirectionConfig {
                source: resolve_redirection_source(package_folder, &redirection.source),
                target: redirection.target.clone(),
                kind: redirection.kind.clone(),
            })
            .collect()
    }
}

fn resolve_redirection_source(package_folder: &Path, source: &str) -> String {
    let source_path = Path::new(source);
    if source_path.is_absolute() {
        return source.to_string();
    }

    package_folder
        .join(source.replace('/', r"\"))
        .to_string_lossy()
        .to_string()
}

fn is_vanilla_skin_pack_redirection_source(source: &str) -> bool {
    let normalized = normalize_redirection_source(source);
    let expected = normalize_redirection_source(VANILLA_SKIN_PACK_REDIRECTION_SOURCE);
    normalized == expected || normalized.ends_with(&format!(r"\{expected}"))
}

fn normalize_redirection_source(source: &str) -> String {
    source
        .trim()
        .trim_matches(['\\', '/'])
        .replace('/', r"\")
        .to_ascii_lowercase()
}

fn parse_version_config(content: &str) -> VersionConfig {
    let mut config: VersionConfig = match serde_json::from_str::<serde_json::Value>(content) {
        Ok(mut value) => {
            if let Some(object) = value.as_object_mut() {
                let has_disable = object
                    .get("disable_mod_loading")
                    .and_then(serde_json::Value::as_bool)
                    .is_some();
                if !has_disable
                    && let Some(inject) = object
                        .get("inject_on_launch")
                        .and_then(serde_json::Value::as_bool)
                {
                    object.insert(
                        "disable_mod_loading".to_string(),
                        serde_json::Value::Bool(!inject),
                    );
                }
            }
            serde_json::from_value(value).unwrap_or_default()
        }
        Err(_) => serde_json::from_str(content).unwrap_or_default(),
    };
    config.normalize_managed_redirections();
    config
}

fn instance_directory(folder_name: &str) -> PathBuf {
    crate::utils::file_ops::bmcbl_subdir("versions").join(folder_name)
}

fn bmcbl_config_path(instance_directory: &Path) -> PathBuf {
    instance_directory.join("config/BMCBL/config.json")
}

fn read_json_object(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    if !path.exists() {
        return Ok(serde_json::Map::new());
    }
    let contents = std_fs::read_to_string(path)
        .map_err(|error| format!("无法读取配置文件 {}：{error}", path.display()))?;
    serde_json::from_str::<serde_json::Value>(&contents)
        .map_err(|error| format!("无法解析配置文件 {}：{error}", path.display()))?
        .as_object()
        .cloned()
        .ok_or_else(|| format!("配置文件根节点不是对象：{}", path.display()))
}

fn normalized_bmcbl_object(
    mut object: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    if let Some(config) = object
        .remove("config")
        .and_then(|value| value.as_object().cloned())
    {
        for (key, value) in config {
            object.entry(key).or_insert(value);
        }
    }
    if !object.contains_key("editor_mode")
        && let Some(value) = object.remove("isEditModel")
    {
        object.insert("editor_mode".to_string(), value);
    }
    object
}

fn load_version_config_from_directory(instance_directory: &Path) -> Result<VersionConfig, String> {
    let root_path = instance_directory.join("config.json");
    let mut root = read_json_object(&root_path)?;
    let bmcbl_path = bmcbl_config_path(instance_directory);
    let bmcbl_exists = bmcbl_path.exists();
    let mut bmcbl = normalized_bmcbl_object(read_json_object(&bmcbl_path)?);
    let bmcbl_incomplete = BMCBL_CONFIG_KEYS
        .iter()
        .any(|key| !bmcbl.contains_key(*key));
    let mut migrated = false;
    for key in BMCBL_CONFIG_KEYS {
        if !bmcbl.contains_key(key)
            && let Some(value) = root.get(key).cloned()
        {
            bmcbl.insert(key.to_string(), value);
            migrated = true;
        }
    }

    if migrated || !bmcbl_exists || bmcbl_incomplete {
        let parsed: BmcblInstanceConfig =
            serde_json::from_value(serde_json::Value::Object(bmcbl.clone()))
                .map_err(|error| format!("无法解析 BMCBL 实例配置：{error}"))?;
        write_json_atomically(&bmcbl_path, &parsed)?;
    }
    let removed = BMCBL_CONFIG_KEYS
        .iter()
        .any(|key| root.remove(*key).is_some());
    if removed {
        write_json_atomically(&root_path, &serde_json::Value::Object(root.clone()))?;
    }

    let mut merged = root;
    for (key, value) in bmcbl {
        merged.insert(key, value);
    }
    Ok(parse_version_config(
        &serde_json::Value::Object(merged).to_string(),
    ))
}

fn save_version_config_to_directory(
    instance_directory: &Path,
    config: &VersionConfig,
) -> Result<(), String> {
    let root_path = instance_directory.join("config.json");
    let mut root = read_json_object(&root_path)?;
    let serialized =
        serde_json::to_value(config).map_err(|error| format!("无法序列化实例配置：{error}"))?;
    let serialized = serialized
        .as_object()
        .ok_or_else(|| "实例配置序列化结果不是对象".to_string())?;
    let previous_bmcbl =
        normalized_bmcbl_object(read_json_object(&bmcbl_config_path(instance_directory))?);
    for (key, value) in serialized {
        if !BMCBL_CONFIG_KEYS.contains(&key.as_str()) && !previous_bmcbl.contains_key(key) {
            root.insert(key.clone(), value.clone());
        }
    }
    for key in BMCBL_CONFIG_KEYS {
        root.remove(key);
    }

    let settings = BmcblInstanceConfig {
        editor_mode: config.editor_mode,
        lock_mouse_on_launch: config.lock_mouse_on_launch,
        unlock_mouse_hotkey: config.unlock_mouse_hotkey.clone(),
        reduce_pixels: config.reduce_pixels,
        shortcut_silent_launch: config.shortcut_silent_launch,
        extra: previous_bmcbl
            .into_iter()
            .filter(|(key, _)| !BMCBL_CONFIG_KEYS.contains(&key.as_str()))
            .collect(),
    };
    write_json_atomically(&bmcbl_config_path(instance_directory), &settings)?;
    write_json_atomically(&root_path, &serde_json::Value::Object(root))
}

pub fn get_version_config_blocking(folder_name: &str) -> Result<VersionConfig, String> {
    load_version_config_from_directory(&instance_directory(folder_name))
}

pub async fn get_version_config(folder_name: String) -> Result<VersionConfig, String> {
    crate::tasks::runtime::run_io_blocking(move || get_version_config_blocking(&folder_name))
        .await
        .map_err(|error| format!("读取实例配置任务失败：{error}"))?
}

pub async fn save_version_config(folder_name: String, config: VersionConfig) -> Result<(), String> {
    let folder_name_for_log = folder_name.clone();
    crate::tasks::runtime::run_io_blocking(move || {
        let directory = instance_directory(&folder_name);
        if !directory.exists() {
            return Err("版本目录不存在".to_string());
        }
        save_version_config_to_directory(&directory, &config)
    })
    .await
    .map_err(|error| format!("保存实例配置任务失败：{error}"))??;
    info!("版本配置已保存: {}", folder_name_for_log);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_config_migrates_legacy_inject_setting() {
        let config = parse_version_config(r#"{"inject_on_launch":false}"#);

        assert!(config.disable_mod_loading);
    }

    #[test]
    fn instance_directory_uses_application_versions_root() {
        assert_eq!(
            instance_directory("26.20"),
            crate::utils::file_ops::bmcbl_subdir("versions").join("26.20")
        );
    }

    #[test]
    fn vanilla_skin_pack_redirect_updates_managed_file_redirection() {
        let mut config = VersionConfig::default();

        config.set_vanilla_skin_pack_redirect(Some(r"C:\packs\skin".to_string()));

        assert_eq!(
            config.vanilla_skin_pack_redirect.as_deref(),
            Some(r"C:\packs\skin")
        );
        assert_eq!(config.file_redirections.len(), 1);
        assert_eq!(
            config.file_redirections[0].source,
            VANILLA_SKIN_PACK_REDIRECTION_SOURCE
        );
        assert_eq!(config.file_redirections[0].target, r"C:\packs\skin");

        config.set_vanilla_skin_pack_redirect(None);

        assert!(config.vanilla_skin_pack_redirect.is_none());
        assert!(config.file_redirections.is_empty());
    }

    #[test]
    fn effective_file_redirections_resolve_relative_sources_from_package_folder() {
        let mut config = VersionConfig::default();
        config.set_vanilla_skin_pack_redirect(Some(r"C:\packs\skin".to_string()));

        let redirections = config.effective_file_redirections(Path::new(r"C:\Games\Minecraft"));

        assert_eq!(redirections.len(), 1);
        assert_eq!(
            redirections[0].source,
            Path::new(r"C:\Games\Minecraft")
                .join(VANILLA_SKIN_PACK_REDIRECTION_SOURCE)
                .to_string_lossy()
                .to_string()
        );
    }

    #[test]
    fn migration_moves_launcher_fields_without_touching_bloader_fields() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        std::fs::write(
            directory.path().join("config.json"),
            r#"{
                "editor_mode": true,
                "lock_mouse_on_launch": true,
                "unlock_mouse_hotkey": "CTRL",
                "reduce_pixels": 32,
                "shortcut_silent_launch": false,
                "enable_network_hook": true,
                "panel_ui": {"enabled": true},
                "future_bloader_field": 17
            }"#,
        )
        .map_err(|error| error.to_string())?;

        let config = load_version_config_from_directory(directory.path())?;
        assert!(config.editor_mode);
        assert!(config.lock_mouse_on_launch);
        assert_eq!(config.unlock_mouse_hotkey, "CTRL");
        assert_eq!(config.reduce_pixels, 32);
        assert!(!config.shortcut_silent_launch);

        let root = read_json_object(&directory.path().join("config.json"))?;
        for key in BMCBL_CONFIG_KEYS {
            assert!(!root.contains_key(key));
        }
        assert_eq!(
            root.get("enable_network_hook"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            root.get("panel_ui"),
            Some(&serde_json::json!({"enabled": true}))
        );
        assert_eq!(
            root.get("future_bloader_field"),
            Some(&serde_json::json!(17))
        );

        let bmcbl = read_json_object(&bmcbl_config_path(directory.path()))?;
        assert_eq!(bmcbl.get("editor_mode"), Some(&serde_json::json!(true)));
        assert_eq!(bmcbl.get("reduce_pixels"), Some(&serde_json::json!(32)));
        Ok(())
    }

    #[test]
    fn saving_preserves_unknown_fields_in_both_files() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        std::fs::create_dir_all(directory.path().join("config/BMCBL"))
            .map_err(|error| error.to_string())?;
        std::fs::write(
            directory.path().join("config.json"),
            r#"{"network_hook":{"enabled":true},"unknown_root":"keep"}"#,
        )
        .map_err(|error| error.to_string())?;
        std::fs::write(
            bmcbl_config_path(directory.path()),
            r#"{"editor_mode":false,"unknown_bmcbl":"keep"}"#,
        )
        .map_err(|error| error.to_string())?;

        let mut config = load_version_config_from_directory(directory.path())?;
        config.editor_mode = true;
        config.enable_debug_console = true;
        save_version_config_to_directory(directory.path(), &config)?;

        let root = read_json_object(&directory.path().join("config.json"))?;
        assert_eq!(root.get("unknown_root"), Some(&serde_json::json!("keep")));
        assert_eq!(
            root.get("network_hook"),
            Some(&serde_json::json!({"enabled": true}))
        );
        assert!(!root.contains_key("editor_mode"));
        let bmcbl = read_json_object(&bmcbl_config_path(directory.path()))?;
        assert_eq!(bmcbl.get("editor_mode"), Some(&serde_json::json!(true)));
        assert_eq!(bmcbl.get("unknown_bmcbl"), Some(&serde_json::json!("keep")));
        Ok(())
    }

    #[test]
    fn malformed_root_config_is_not_migrated_or_rewritten() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let root_path = directory.path().join("config.json");
        std::fs::write(&root_path, "not-json").map_err(|error| error.to_string())?;

        assert!(load_version_config_from_directory(directory.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(root_path).map_err(|error| error.to_string())?,
            "not-json"
        );
        assert!(!bmcbl_config_path(directory.path()).exists());
        Ok(())
    }
}
