use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;

use super::defaults::{
    default_appx_api, default_config_version, default_error_report_sentry_enabled,
    default_log_active_size_mb, default_log_archive_files, default_log_compression_level,
    default_log_retention_days, default_log_total_size_mb, default_proton_gdk_source,
    default_renderer_backend, default_true, default_update_check_interval_minutes,
};
pub use super::defaults::{
    default_background_blur, default_error_report_sentry_dsn, default_font_source,
    default_glass_effect_enabled, default_gpu_adapter_name, default_online_player_name,
    default_theme_mode, get_default_config,
};

pub(super) const CURRENT_CONFIG_VERSION: u32 = 5;
pub(super) const LEGACY_DEFAULT_APPX_API: &str = "https://data.mcappx.com/v2/bedrock.json";
pub(super) const INCORRECT_MIRROR_APPX_API: &str =
    "https://api.chlna6666.com/api/v1/bedrock/versions";
pub const DEFAULT_APPX_API: &str = "https://api.chlna6666.com/api/v1/bedrock/mcappx";
pub const DEFAULT_ERROR_REPORT_SENTRY_DSN: &str = "https://a6851001eec5b056a734b518f20d4175@o4511448309891072.ingest.de.sentry.io/4511448317493328";
pub const MAX_BACKGROUND_BLUR: f32 = 10.0;
pub const FONT_SOURCE_DEFAULT: &str = "default";
pub const FONT_SOURCE_LOCAL: &str = "local";
pub const FONT_SOURCE_SYSTEM: &str = "system";
pub const THEME_MODE_LIGHT: &str = "light";
pub const THEME_MODE_DARK: &str = "dark";
pub const MIN_LOG_RETENTION_DAYS: u32 = 1;
pub const MAX_LOG_RETENTION_DAYS: u32 = 3_650;
pub const MIN_LOG_ACTIVE_SIZE_MB: u32 = 1;
pub const MAX_LOG_ACTIVE_SIZE_MB: u32 = 512;
pub const MIN_LOG_ARCHIVE_FILES: u32 = 1;
pub const MAX_LOG_ARCHIVE_FILES: u32 = 1_024;
pub const MIN_LOG_TOTAL_SIZE_MB: u32 = 16;
pub const MAX_LOG_TOTAL_SIZE_MB: u32 = 8_192;
pub const MIN_LOG_COMPRESSION_LEVEL: i32 = 1;
pub const MAX_LOG_COMPRESSION_LEVEL: i32 = 9;

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

/// 只读取代理配置（避免深拷贝整个 Config）。
pub fn read_proxy_config() -> std::io::Result<ProxyConfig> {
    super::storage::read_proxy_config()
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

pub fn persist_language(code: &str) -> std::io::Result<()> {
    super::storage::persist_language(code)
}

/// 立即把内存中未落盘的配置写入磁盘（供应用退出路径调用）。
pub fn flush_config_now() {
    super::storage::flush_config_now()
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

pub fn normalize_theme_mode(mode: &str) -> String {
    match mode.trim().to_ascii_lowercase().as_str() {
        THEME_MODE_DARK => THEME_MODE_DARK.to_string(),
        _ => THEME_MODE_LIGHT.to_string(),
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct LogManagementConfig {
    #[serde(default = "default_log_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_log_active_size_mb")]
    pub active_file_size_mb: u32,
    #[serde(default = "default_log_archive_files")]
    pub max_archive_files: u32,
    #[serde(default = "default_log_total_size_mb")]
    pub max_total_size_mb: u32,
    #[serde(default = "default_log_compression_level")]
    pub compression_level: i32,
}

impl Default for LogManagementConfig {
    fn default() -> Self {
        Self {
            retention_days: default_log_retention_days(),
            active_file_size_mb: default_log_active_size_mb(),
            max_archive_files: default_log_archive_files(),
            max_total_size_mb: default_log_total_size_mb(),
            compression_level: default_log_compression_level(),
        }
    }
}

impl LogManagementConfig {
    #[must_use]
    pub fn normalized(&self) -> Self {
        Self {
            retention_days: self
                .retention_days
                .clamp(MIN_LOG_RETENTION_DAYS, MAX_LOG_RETENTION_DAYS),
            active_file_size_mb: self
                .active_file_size_mb
                .clamp(MIN_LOG_ACTIVE_SIZE_MB, MAX_LOG_ACTIVE_SIZE_MB),
            max_archive_files: self
                .max_archive_files
                .clamp(MIN_LOG_ARCHIVE_FILES, MAX_LOG_ARCHIVE_FILES),
            max_total_size_mb: self
                .max_total_size_mb
                .clamp(MIN_LOG_TOTAL_SIZE_MB, MAX_LOG_TOTAL_SIZE_MB),
            compression_level: self
                .compression_level
                .clamp(MIN_LOG_COMPRESSION_LEVEL, MAX_LOG_COMPRESSION_LEVEL),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CustomStyle {
    pub theme_color: String,
    #[serde(default = "default_theme_mode")]
    pub theme_mode: String,
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
pub struct OnlineConfig {
    pub bootstrap_peers: String,
    pub player_name: String,
    pub game_ports: String,
    pub disable_p2p: bool,
}

impl Default for OnlineConfig {
    fn default() -> Self {
        Self {
            bootstrap_peers: String::new(),
            player_name: default_online_player_name(),
            game_ports: "7551".to_string(),
            disable_p2p: false,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(default)]
pub struct AppStateConfig {
    pub agreement_accepted_version: u32,
    pub onboarding_completed_version: u32,
    /// v4 及更早版本写入过的平台完成状态。只读取用于迁移，不再写回 settings.toml。
    #[serde(
        default,
        rename = "onboarding_windows_completed_version",
        skip_serializing
    )]
    pub(super) legacy_onboarding_windows_completed_version: u32,
    /// v4 及更早版本写入过的平台完成状态。只读取用于迁移，不再写回 settings.toml。
    #[serde(
        default,
        rename = "onboarding_linux_completed_version",
        skip_serializing
    )]
    pub(super) legacy_onboarding_linux_completed_version: u32,
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
    #[serde(default = "default_appx_api")]
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
    #[serde(default)]
    pub log_management: LogManagementConfig,
    #[cfg(target_os = "linux")]
    #[serde(default = "default_proton_gdk_source")]
    pub proton_gdk_source: String,
    #[cfg(target_os = "linux")]
    #[serde(default)]
    pub proton_gdk_runner: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Config {
    #[serde(default = "default_config_version")]
    pub config_version: u32,
    pub custom_style: CustomStyle,
    pub launcher: Launcher,
    pub game: GameConfig,
    #[serde(default)]
    pub online: OnlineConfig,
    #[serde(default)]
    pub app_state: AppStateConfig,
    /// v3 及更早版本的协议布尔状态。只读取用于迁移，不再写回 settings.toml。
    #[serde(default, skip_serializing)]
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
            #[cfg(target_os = "linux")]
            {
                "auto".to_string()
            }
            #[cfg(not(target_os = "linux"))]
            {
                "dx12".to_string()
            }
        }
        "dx11" | "directx11" | "d3d11" => {
            #[cfg(target_os = "linux")]
            {
                "auto".to_string()
            }
            #[cfg(not(target_os = "linux"))]
            {
                "dx12".to_string()
            }
        }
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
                        target_map.insert(key.to_string(), overlay_value);
                    }
                }
            }
        }
        (target_value, overlay_value) => {
            *target_value = overlay_value;
        }
    }
}
