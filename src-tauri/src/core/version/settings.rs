use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;
use tracing::{error, info};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VersionConfig {
    #[serde(default)]
    pub enable_debug_console: bool,
    #[serde(default)]
    pub enable_redirection: bool,
    #[serde(default)]
    pub editor_mode: bool,

    // Per-version settings migrated from global config.game
    #[serde(default = "default_true")]
    pub inject_on_launch: bool,
    #[serde(default)]
    pub lock_mouse_on_launch: bool,
    #[serde(default = "default_unlock_hotkey")]
    pub unlock_mouse_hotkey: String,
    #[serde(default)]
    pub reduce_pixels: i32,
}

fn default_true() -> bool {
    true
}

fn default_unlock_hotkey() -> String {
    "ALT".to_string()
}

impl Default for VersionConfig {
    fn default() -> Self {
        Self {
            enable_debug_console: false,
            enable_redirection: false,
            editor_mode: false,
            inject_on_launch: true,
            lock_mouse_on_launch: false,
            unlock_mouse_hotkey: "ALT".to_string(),
            reduce_pixels: 0,
        }
    }
}

#[tauri::command]
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

    let config: VersionConfig = serde_json::from_str(&content)
        .unwrap_or_else(|_| VersionConfig::default());

    Ok(config)
}

#[tauri::command]
pub async fn save_version_config(folder_name: String, config: VersionConfig) -> Result<(), String> {
    let versions_root = Path::new("./BMCBL/versions");
    let version_dir = versions_root.join(&folder_name);

    if !version_dir.exists() {
        return Err("版本目录不存在".to_string());
    }

    let config_path = version_dir.join("config.json");
    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("序列化失败: {}", e))?;

    fs::write(&config_path, json)
        .await
        .map_err(|e| format!("无法保存配置文件: {}", e))?;

    info!("版本配置已保存: {}", folder_name);
    Ok(())
}
