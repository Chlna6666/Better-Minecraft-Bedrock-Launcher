use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;

pub use super::defaults::{
    default_background_blur, default_error_report_sentry_dsn, default_font_source,
    default_glass_effect_enabled, default_gpu_adapter_name, get_default_config,
};
use super::defaults::{
    default_config_version, default_error_report_sentry_enabled, default_music_volume,
    default_renderer_backend, default_true, default_update_check_interval_minutes,
};

pub(super) const CURRENT_CONFIG_VERSION: u32 = 1;
pub const DEFAULT_ERROR_REPORT_SENTRY_DSN: &str = "https://a6851001eec5b056a734b518f20d4175@o4511448309891072.ingest.de.sentry.io/4511448317493328";
pub const MAX_BACKGROUND_BLUR: f32 = 10.0;
pub const FONT_SOURCE_DEFAULT: &str = "default";
pub const FONT_SOURCE_LOCAL: &str = "local";
pub const FONT_SOURCE_SYSTEM: &str = "system";
pub const DEFAULT_MUSIC_VOLUME: f32 = 0.5;

pub fn get_config_file_path() -> std::path::PathBuf {
    super::storage::get_config_file_path()
}

pub fn ensure_config_dir() -> std::io::Result<()> {
    super::storage::ensure_config_dir()
}

pub fn ensure_config_file() -> std::io::Result<()> {
    super::storage::ensure_config_file()
}

pub fn initialize_config_cache() -> std::io::Result<Config> {
    super::storage::initialize_config_cache()
}

pub fn read_config() -> std::io::Result<Config> {
    super::storage::read_config()
}

pub fn reload_config() -> std::io::Result<Config> {
    super::storage::reload_config()
}

pub fn write_config(config: &Config) -> std::io::Result<()> {
    super::storage::write_config(config)
}

pub fn update_config<T, F>(mutator: F) -> std::io::Result<T>
where
    F: FnOnce(&mut Config) -> T,
{
    super::storage::update_config(mutator)
}

pub fn resolved_error_report_sentry_dsn(launcher: &Launcher) -> Option<String> {
    if !launcher.error_report_sentry_enabled {
        return None;
    }

    let dsn = launcher.error_report_sentry_dsn.trim();
    Some(if dsn.is_empty() {
        default_error_report_sentry_dsn()
    } else {
        dsn.to_string()
    })
}

pub fn error_report_sentry_auto_enabled(launcher: &Launcher) -> bool {
    launcher.error_report_sentry_auto && resolved_error_report_sentry_dsn(launcher).is_some()
}

pub fn clamp_background_blur(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, MAX_BACKGROUND_BLUR)
    } else {
        default_background_blur()
    }
}

pub fn normalize_font_source(source: &str) -> String {
    match source.trim().to_ascii_lowercase().as_str() {
        FONT_SOURCE_LOCAL => FONT_SOURCE_LOCAL.to_string(),
        FONT_SOURCE_SYSTEM => FONT_SOURCE_SYSTEM.to_string(),
        _ => FONT_SOURCE_DEFAULT.to_string(),
    }
}

pub fn clamp_music_volume(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        default_music_volume()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CustomStyle {
    pub theme_color: String,
    pub background_option: String,
    pub local_image_path: String,
    pub network_image_url: String,
    #[serde(default = "default_background_blur")]
    pub background_blur: f32,
    pub show_launch_animation: bool,
    #[serde(default = "default_glass_effect_enabled")]
    pub glass_effect_enabled: bool,
    #[serde(default = "default_font_source")]
    pub font_source: String,
    #[serde(default)]
    pub local_font_path: String,
    #[serde(default)]
    pub local_font_family: String,
    #[serde(default)]
    pub system_font_family: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct GameConfig {
    pub launcher_visibility: String, // "minimize", "close", "keep"
    #[serde(default, alias = "keep_appx_after_install")]
    pub keep_downloaded_game_package: bool, // 安装完成保留下载的游戏包（默认关闭）
    pub modify_appx_manifest: bool,  // 是否修改 AppxManifest.xml
    pub uwp_minimize_fix: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProxyType {
    #[default]
    None,
    System,
    Http,
    Socks5,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[serde(alias = "stable")]
    Stable,
    Nightly,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(default)]
pub struct ProxyConfig {
    pub proxy_type: ProxyType,
    pub http_proxy_url: String,
    pub socks_proxy_url: String,
}

impl Default for UpdateChannel {
    fn default() -> Self {
        UpdateChannel::Stable
    }
}

impl FromStr for UpdateChannel {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "nightly" => Ok(UpdateChannel::Nightly),
            _ => Ok(UpdateChannel::Stable),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct DownloadConfig {
    pub multi_thread: bool,
    pub max_threads: u32,
    pub auto_thread_count: bool,
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub curseforge_api_source: String,
    #[serde(default)]
    pub curseforge_api_base: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct MusicConfig {
    #[serde(default = "default_true")]
    pub auto_play_on_startup: bool,
    #[serde(default = "default_music_volume")]
    pub volume: f32,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub playback_mode: crate::music::MusicPlaybackMode,
    #[serde(default)]
    pub last_track_path: String,
}

impl Default for MusicConfig {
    fn default() -> Self {
        Self {
            auto_play_on_startup: true,
            volume: default_music_volume(),
            muted: false,
            playback_mode: crate::music::MusicPlaybackMode::Repeat,
            last_track_path: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Launcher {
    pub debug: bool,
    /// When non-zero, enables "port debug" mode (opens a debug window and binds a local TCP port).
    #[serde(default)]
    pub debug_port: u16,
    pub language: String, // "auto", "en-US", "zh-CN" 等
    #[serde(default = "default_renderer_backend")]
    pub renderer_backend: String,
    #[serde(default = "default_gpu_adapter_name")]
    pub gpu_adapter_name: String,
    #[serde(default = "default_true")]
    pub stats_upload: bool, // 上传基础统计信息 (默认开启)
    #[serde(default = "default_error_report_sentry_enabled")]
    pub error_report_sentry_enabled: bool,
    #[serde(default = "default_error_report_sentry_dsn")]
    pub error_report_sentry_dsn: String,
    #[serde(default)]
    pub error_report_sentry_auto: bool,
    pub custom_appx_api: String,
    pub download: DownloadConfig,
    #[serde(default)]
    pub update_channel: UpdateChannel, // "stable" 或 "nightly"
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
    #[serde(default, skip_serializing)]
    pub check_on_start: bool,
    #[serde(default = "default_update_check_interval_minutes")]
    pub update_check_interval_minutes: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Config {
    #[serde(default = "default_config_version")]
    pub config_version: u32,
    pub custom_style: CustomStyle,
    pub launcher: Launcher,
    pub game: GameConfig,
    #[serde(default)]
    pub music: MusicConfig,
    pub agreement_accepted: bool,
}

pub(super) fn normalize_language_code(lang: &str) -> String {
    let trimmed = lang.trim();
    if trimmed.eq_ignore_ascii_case("auto") || trimmed.is_empty() {
        return trimmed.to_string();
    }
    trimmed.replace('_', "-")
}

pub fn normalize_renderer_backend(renderer_backend: &str) -> String {
    match renderer_backend.trim().to_ascii_lowercase().as_str() {
        "" | "auto" | "default" => "auto".to_string(),
        "vk" | "vulkan" | "nova" | "blade" | "nova-vulkan" | "nova_vulkan" => "vulkan".to_string(),
        "dx12" | "directx" | "directx12" | "d3d12" | "nova-dx12" | "nova_dx12" => {
            "dx12".to_string()
        }
        "dx11" | "directx11" | "d3d11" => "dx12".to_string(),
        _ => "auto".to_string(),
    }
}

pub fn normalize_gpu_adapter_name(gpu_adapter_name: &str) -> String {
    let trimmed = gpu_adapter_name.trim();
    let legacy_label = trimmed.to_ascii_lowercase().replace('-', "_");
    if trimmed.is_empty()
        || matches!(
            legacy_label.as_str(),
            "auto"
                | "default"
                | "discrete"
                | "dedicated"
                | "high"
                | "high_performance"
                | "performance"
                | "dgpu"
                | "integrated"
                | "igpu"
                | "low"
                | "low_power"
                | "power_saving"
                | "powersaving"
        )
    {
        default_gpu_adapter_name()
    } else {
        trimmed.to_string()
    }
}

pub fn get_nested_value(data: &Value, key: &str) -> Option<Value> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current = data;
    for part in parts {
        current = current.get(part)?;
    }
    Some(current.clone())
}

pub fn set_nested_value(data: &mut Value, key: &str, value: Value) -> Result<(), String> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current = data;

    for i in 0..parts.len() {
        let part = parts[i];
        if i == parts.len() - 1 {
            return if let Some(obj) = current.as_object_mut() {
                obj.insert(part.to_string(), value);
                Ok(())
            } else {
                Err(format!("Key '{}' is not an object", part))
            };
        } else {
            current = current
                .get_mut(part)
                .ok_or_else(|| format!("Key '{}' not found", part))?;
        }
    }

    Err("Invalid key".to_string())
}

pub fn merge_json_values(target: &mut Value, overlay: Value) {
    match (target, overlay) {
        (Value::Object(target_map), Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                match target_map.get_mut(&key) {
                    Some(target_value) => merge_json_values(target_value, overlay_value),
                    None => {
                        target_map.insert(key, overlay_value);
                    }
                }
            }
        }
        (target_value, overlay_value) => {
            *target_value = overlay_value;
        }
    }
}
