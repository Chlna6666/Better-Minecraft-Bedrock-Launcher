use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::{fs, io};
use tracing::{debug, error};
use crate::utils::file_ops;

fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CustomStyle {
    pub theme_color: String,
    pub background_option: String,
    pub local_image_path: String,
    pub network_image_url: String,
    pub show_launch_animation: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct GameConfig {
    pub launcher_visibility: String,   // "minimize", "close", "keep"
    #[serde(default, alias = "keep_appx_after_install")]
    pub keep_downloaded_game_package: bool, // 安装完成保留下载的游戏包（默认关闭）
    pub modify_appx_manifest: bool,    // 是否修改 AppxManifest.xml
    pub uwp_minimize_fix: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
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

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
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
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Launcher {
    pub debug: bool,
    pub language: String, // "auto", "en-US", "zh-CN" 等
    #[serde(default = "default_true")]
    pub gpu_acceleration: bool, // WebView2 GPU 加速 (默认开启)
    #[serde(default = "default_true")]
    pub stats_upload: bool, // 上传基础统计信息 (默认开启)
    pub custom_appx_api: String,
    pub download: DownloadConfig,
    #[serde(default)]
    pub update_channel: UpdateChannel, // "stable" 或 "nightly"
    pub auto_check_updates: bool,
    pub check_on_start: bool,
    pub update_check_interval_minutes: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Config {
    pub custom_style: CustomStyle,
    pub launcher: Launcher,
    pub game: GameConfig,
    pub agreement_accepted: bool,
}

pub fn get_config_file_path() -> PathBuf {
    file_ops::bmcbl_subdir("config").join("settings.toml")
}

pub fn ensure_config_dir() -> io::Result<()> {
    let config_dir = file_ops::bmcbl_subdir("config");
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir)?;
    }
    Ok(())
}

pub fn ensure_config_file() -> io::Result<()> {
    let config_file = get_config_file_path();
    if !config_file.exists() {
        let default_config = get_default_config();
        let toml_content = toml::to_string(&default_config).unwrap();
        let mut file = fs::File::create(config_file)?;
        file.write_all(toml_content.as_bytes())?;
    }
    Ok(())
}

pub fn get_default_config() -> Config {
    Config {
        custom_style: CustomStyle {
            theme_color: "#a0d9b6".to_string(),
            background_option: "default".to_string(),
            local_image_path: "".to_string(),
            network_image_url: "".to_string(),
            show_launch_animation: true,
        },
        launcher: Launcher {
            debug: false,
            language: "auto".to_string(),
            gpu_acceleration: true,
            stats_upload: true,
            custom_appx_api: "https://data.mcappx.com/v2/bedrock.json".to_string(),
            download: DownloadConfig {
                multi_thread: false,
                max_threads: 8,
                auto_thread_count: true,
                proxy: ProxyConfig {
                    proxy_type: ProxyType::None,
                    http_proxy_url: "".to_string(),
                    socks_proxy_url: "".to_string(),
                },
                curseforge_api_source: "mirror".to_string(),
                curseforge_api_base: "https://mod.mcimirror.top/curseforge".to_string(),
            },
            update_channel: UpdateChannel::Stable,
            auto_check_updates: true,
            check_on_start: false,
            update_check_interval_minutes: 60,
        },
        game: GameConfig {
            launcher_visibility: "keep".to_string(),
            keep_downloaded_game_package: false,
            modify_appx_manifest: true,
            uwp_minimize_fix: true,
        },
        agreement_accepted: false,
    }
}

pub fn read_config() -> io::Result<Config> {
    ensure_config_dir()?;
    ensure_config_file()?;

    let config_file = get_config_file_path();
    let content = fs::read_to_string(&config_file)?;
    let has_legacy_keep_appx = content.contains("keep_appx_after_install");

    let config: Config = match toml::from_str(&content) {
        Ok(parsed_config) => parsed_config,
        Err(err) => {
            error!("Failed to parse config on first attempt: {:?}", err);

            let default_config = get_default_config();
            if let Ok(existing_config) = toml::from_str::<toml::Value>(&content) {
                if let toml::Value::Table(existing_table) = existing_config {
                    if let toml::Value::Table(default_table) =
                        toml::Value::try_from(&default_config)
                            .unwrap_or(toml::Value::Table(Default::default()))
                    {
                        let merged_config = merge_tables(default_table, existing_table);
                        let updated_content =
                            toml::ser::to_string(&toml::Value::Table(merged_config))
                                .expect("Failed to serialize merged config");
                        fs::write(&config_file, updated_content)?;
                    }
                }
            }

            let updated_content = fs::read_to_string(&config_file)?;
            toml::from_str(&updated_content).unwrap_or_else(|second_err| {
                error!("Failed to parse config on second attempt: {:?}", second_err);
                get_default_config()
            })
        }
    };

    let mut config = config;
    let mut migrated = false;
    let normalized_lang = normalize_language_code(&config.launcher.language);
    if normalized_lang != config.launcher.language {
        config.launcher.language = normalized_lang;
        migrated = true;
    }
    if has_legacy_keep_appx {
        migrated = true;
    }
    if migrated {
        let _ = write_config(&config);
        debug!("Migrated config");
    }

    debug!("Read and updated config: {:?}", config);
    Ok(config)
}

fn normalize_language_code(lang: &str) -> String {
    let trimmed = lang.trim();
    if trimmed.eq_ignore_ascii_case("auto") || trimmed.is_empty() {
        return trimmed.to_string();
    }
    trimmed.replace('_', "-")
}

fn merge_tables(
    mut default: toml::map::Map<String, toml::Value>,
    existing: toml::map::Map<String, toml::Value>,
) -> toml::map::Map<String, toml::Value> {
    for (key, existing_value) in existing {
        match default.get_mut(&key) {
            Some(default_value) => {
                if let (toml::Value::Table(default_table), toml::Value::Table(existing_table)) =
                    (default_value.clone(), existing_value.clone())
                {
                    *default_value =
                        toml::Value::Table(merge_tables(default_table, existing_table));
                } else {
                    *default_value = existing_value;
                }
            }
            None => {
                default.insert(key, existing_value);
            }
        }
    }
    default
}

pub fn write_config(config: &Config) -> std::io::Result<()> {
    ensure_config_dir()?;
    let config_file = get_config_file_path();
    let toml_content = toml::to_string(config).unwrap();
    let mut file = fs::File::create(config_file)?;
    file.write_all(toml_content.as_bytes())?;
    Ok(())
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
