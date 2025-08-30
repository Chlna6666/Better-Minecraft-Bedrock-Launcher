use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::{fs, io};
use tauri_plugin_store::JsonValue;
use tracing::{debug, error};

#[derive(Serialize, Deserialize, Debug, Clone,Default)]
pub struct CustomStyle {
    pub theme_color: String,
    pub background_option: String,
    pub local_image_path: String,
    pub network_image_url: String,
    pub show_launch_animation: bool,
}


#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct GameConfig {
    pub inject_on_launch: bool,
    pub lock_mouse_on_launch: bool,
    pub reduce_pixels: i32, // 减少的像素数
    pub unlock_mouse_hotkey: String,
    pub launcher_visibility: String, // "minimize", "close", "keep"
    pub keep_appx_after_install: bool, // 安装完成保留 APPX（默认关闭）
    pub modify_appx_manifest: bool,    // 是否修改 AppxManifest.xml
    pub uwp_minimize_fix: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone,Default)]
pub struct ProxyConfig {
    pub disable_all_proxy: bool,
    pub use_system_proxy: bool,
    pub enable_http_proxy: bool,
    pub http_proxy_url: String,
    pub enable_socks_proxy: bool,
    pub socks_proxy_url: String,
    pub enable_custom_proxy: bool,
    pub custom_proxy_url: String,
}


#[derive(Serialize, Deserialize, Debug, Clone,Default)]
pub struct DownloadConfig {
    pub multi_thread: bool,
    pub max_threads: u32,
    pub auto_thread_count: bool,
    pub proxy: ProxyConfig,
}
#[derive(Serialize, Deserialize, Debug, Clone,Default)]
pub struct Launcher {
    pub debug: bool,
    pub language: String, // "auto", "en-US", "zh-CN" 等
    pub custom_appx_api: String,
    pub download: DownloadConfig,
}

#[derive(Serialize, Deserialize, Debug, Clone,Default)]
pub struct Config {
    pub custom_style: CustomStyle,
    pub launcher: Launcher,
    pub game: GameConfig,
    pub agreement_accepted: bool,
}

pub fn get_config_file_path() -> PathBuf {
    let config_dir = std::env::current_dir().unwrap().join("BMCBL/config");
    config_dir.join("settings.toml")
}

pub fn ensure_config_dir() -> std::io::Result<()> {
    let config_dir = std::env::current_dir().unwrap().join("BMCBL/config");
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir)?;
    }
    Ok(())
}

pub fn ensure_config_file() -> std::io::Result<()> {
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
            custom_appx_api: "https://raw.githubusercontent.com/LiteLDev/mc-w10-versiondb-auto-update/refs/heads/master/versions.json.min".to_string(),
            download: DownloadConfig {
                multi_thread: false,
                max_threads: 8,
                auto_thread_count: true,
                proxy: ProxyConfig {
                    disable_all_proxy: true,
                    use_system_proxy: false,
                    enable_http_proxy: false,
                    http_proxy_url: "".to_string(),
                    enable_socks_proxy: false,
                    socks_proxy_url: "".to_string(),
                    enable_custom_proxy: false,
                    custom_proxy_url: "".to_string(),
                },
            },
        },
        game: GameConfig {
            inject_on_launch: true,
            lock_mouse_on_launch: false,
            reduce_pixels: 10,
            unlock_mouse_hotkey: "ALT".to_string(),
            launcher_visibility: "keep".to_string(),
            keep_appx_after_install: false,
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

    debug!("Read and updated config: {:?}", config);
    Ok(config)
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

pub fn get_nested_value(data: &JsonValue, key: &str) -> Option<JsonValue> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current = data;
    for part in parts {
        current = current.get(part)?;
    }
    Some(current.clone())
}

pub fn set_nested_value(data: &mut JsonValue, key: &str, value: JsonValue) -> Result<(), String> {
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
