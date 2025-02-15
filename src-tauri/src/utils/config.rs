use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::{fs, io};
use tracing::{debug, error};

// 定义 CustomStyle 结构
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CustomStyle {
    theme_color: String,
    background_option: String,
    local_image_path: String,
    network_image_url: String,
    show_launch_animation: bool,
}

// 定义 Launcher 结构
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Launcher {
    pub(crate) debug: bool,
}

// 定义 Config 结构
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Config {
    custom_style: CustomStyle,
    pub(crate) launcher: Launcher,
}

// 获取配置文件路径
pub fn get_config_file_path() -> PathBuf {
    let config_dir = std::env::current_dir().unwrap().join("BMCBL/config");
    config_dir.join("settings.toml")
}

// 确保配置文件夹存在
pub fn ensure_config_dir() -> std::io::Result<()> {
    let config_dir = std::env::current_dir().unwrap().join("BMCBL/config");
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir)?;
    }
    Ok(())
}

// 确保配置文件存在
pub fn ensure_config_file() -> std::io::Result<()> {
    let config_file = get_config_file_path();
    if !config_file.exists() {
        let default_config = Config {
            custom_style: CustomStyle {
                theme_color: "".to_string(),
                background_option: "default".to_string(),
                local_image_path: "".to_string(),
                network_image_url: "".to_string(),
                show_launch_animation: true,
            },
            launcher: Launcher { debug: false },
        };
        let toml_content = toml::to_string(&default_config).unwrap();
        let mut file = fs::File::create(config_file)?;
        file.write_all(toml_content.as_bytes())?;
    }
    Ok(())
}

// 读取配置文件
fn get_default_config() -> Config {
    Config {
        custom_style: CustomStyle {
            theme_color: "".to_string(),
            background_option: "default".to_string(),
            local_image_path: "".to_string(),
            network_image_url: "".to_string(),
            show_launch_animation: true,
        },
        launcher: Launcher { debug: false },
    }
}

// 读取配置文件并补充缺失部分

pub fn read_config() -> io::Result<Config> {
    ensure_config_dir()?;
    ensure_config_file()?;

    let config_file = get_config_file_path();
    let content = fs::read_to_string(&config_file)?;

    let config: Config = match toml::from_str(&content) {
        Ok(parsed_config) => parsed_config,
        Err(err) => {
            error!("Failed to parse config on first attempt: {:?}", err);

            // 如果解析失败，尝试将默认配置与现有配置合并
            let default_config = get_default_config();
            if let Ok(existing_config) = toml::from_str::<toml::Value>(&content) {
                if let toml::Value::Table(existing_table) = existing_config {
                    if let toml::Value::Table(default_table) =
                        toml::Value::try_from(&default_config)
                            .unwrap_or(toml::Value::Table(Default::default()))
                    {
                        // 合并默认配置和现有配置
                        let merged_config = merge_tables(default_table, existing_table);
                        let updated_content =
                            toml::ser::to_string(&toml::Value::Table(merged_config))
                                .expect("Failed to serialize merged config");

                        fs::write(&config_file, updated_content)?;
                    }
                }
            }

            // 尝试再次读取并解析
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
                    // 递归合并嵌套表
                    *default_value =
                        toml::Value::Table(merge_tables(default_table, existing_table));
                } else {
                    // 如果类型不匹配或非嵌套表，直接覆盖
                    *default_value = existing_value;
                }
            }
            None => {
                // 如果默认值中不存在该键，直接插入
                default.insert(key, existing_value);
            }
        }
    }
    default
}

// 写入配置文件
pub fn write_config(config: &Config) -> std::io::Result<()> {
    ensure_config_dir()?;
    let config_file = get_config_file_path();
    let toml_content = toml::to_string(config).unwrap();
    let mut file = fs::File::create(config_file)?;
    file.write_all(toml_content.as_bytes())?;
    Ok(())
}

// Tauri命令，供前端调用，获取 custom_style
#[tauri::command]
pub fn get_custom_style() -> Result<CustomStyle, String> {
    match read_config() {
        Ok(config) => Ok(config.custom_style),
        Err(err) => {
            let error_message = format!("Failed to read config: {}", err);
            error!("{}", error_message); // 记录读取错误
            Err(error_message)
        }
    }
}

// Tauri命令，供前端调用，设置 custom_style
#[tauri::command]
pub fn set_custom_style(custom_style: CustomStyle) -> Result<(), String> {
    debug!("Received custom_style: {:?}", custom_style); // 打印接收到的参数
    let mut config = match read_config() {
        Ok(config) => config,
        Err(err) => {
            let error_message = format!("Failed to read config: {}", err);
            error!("{}", error_message); // 记录读取错误
            return Err(error_message);
        }
    };
    config.custom_style = custom_style;
    match write_config(&config) {
        Ok(_) => Ok(()),
        Err(err) => {
            let error_message = format!("Failed to write config: {}", err);
            error!("{}", error_message); // 记录写入错误
            Err(error_message)
        }
    }
}

// Tauri命令，供前端调用，获取 launcher.debug
#[tauri::command]
pub fn get_launcher_debug() -> Result<bool, String> {
    match read_config() {
        Ok(config) => Ok(config.launcher.debug),
        Err(err) => {
            let error_message = format!("Failed to read config: {}", err);
            error!("{}", error_message); // 记录读取错误
            Err(error_message)
        }
    }
}

// Tauri命令，供前端调用，设置 launcher.debug
#[tauri::command]
pub fn set_launcher_debug(debug: bool) -> Result<(), String> {
    let mut config = match read_config() {
        Ok(config) => config,
        Err(err) => {
            let error_message = format!("Failed to read config: {}", err);
            error!("{}", error_message); // 记录读取错误
            return Err(error_message);
        }
    };
    config.launcher.debug = debug;
    match write_config(&config) {
        Ok(_) => Ok(()),
        Err(err) => {
            let error_message = format!("Failed to write config: {}", err);
            error!("{}", error_message); // 记录写入错误
            Err(error_message)
        }
    }
}
