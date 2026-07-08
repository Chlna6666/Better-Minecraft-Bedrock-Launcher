use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;
use tracing::{error, info};

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
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
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

pub async fn get_version_config(folder_name: String) -> Result<VersionConfig, String> {
    let versions_root = Path::new("./BMCBL/versions");
    let config_path = versions_root.join(folder_name).join("config.json");

    if (!config_path.exists()) {
        // 如果文件不存在，返回默认配置
        return Ok(VersionConfig::default());
    }

    let content = fs::read_to_string(&config_path)
        .await
        .map_err(|e| format!("无法读取配置文件: {}", e))?;

    // Small migration: older builds used `inject_on_launch` (true = load mods).
    // Now we use `disable_mod_loading` (true = disable mod loading/injection).
    let mut config: VersionConfig = match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                let has_disable = obj
                    .get("disable_mod_loading")
                    .and_then(|x| x.as_bool())
                    .is_some();
                if !has_disable {
                    if let Some(inject) = obj.get("inject_on_launch").and_then(|x| x.as_bool()) {
                        obj.insert(
                            "disable_mod_loading".to_string(),
                            serde_json::Value::Bool(!inject),
                        );
                    }
                }
            }
            serde_json::from_value(v).unwrap_or_else(|_| VersionConfig::default())
        }
        Err(_) => serde_json::from_str(&content).unwrap_or_else(|_| VersionConfig::default()),
    };
    config.normalize_managed_redirections();

    Ok(config)
}

pub async fn save_version_config(folder_name: String, config: VersionConfig) -> Result<(), String> {
    let versions_root = Path::new("./BMCBL/versions");
    let version_dir = versions_root.join(&folder_name);

    if !version_dir.exists() {
        return Err("版本目录不存在".to_string());
    }

    let config_path = version_dir.join("config.json");
    let json = serde_json::to_string_pretty(&config).map_err(|e| format!("序列化失败: {}", e))?;

    fs::write(&config_path, json)
        .await
        .map_err(|e| format!("无法保存配置文件: {}", e))?;

    info!("版本配置已保存: {}", folder_name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
