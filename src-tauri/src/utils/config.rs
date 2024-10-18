use crate::{debug, error};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
pub struct CustomStyle {
    theme_color: String,
    background_option: String,
    local_image_path: String,
    network_image_url: String,
    show_launch_animation: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    custom_style: CustomStyle,
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
        };
        let toml_content = toml::to_string(&default_config).unwrap();
        let mut file = fs::File::create(config_file)?;
        file.write_all(toml_content.as_bytes())?;
    }
    Ok(())
}

// 读取配置文件
pub fn read_config() -> std::io::Result<Config> {
    ensure_config_dir()?;
    ensure_config_file()?;

    let config_file = get_config_file_path();
    let content = fs::read_to_string(config_file)?;
    let config: Config = toml::from_str(&content).unwrap_or_else(|err| {
        error!("Failed to parse config: {:?}", err); // 记录解析错误
        Config {
            custom_style: CustomStyle {
                theme_color: "".to_string(),
                background_option: "default".to_string(),
                local_image_path: "".to_string(),
                network_image_url: "".to_string(),
                show_launch_animation: true,
            },
        }
    });
    debug!("[Config] Read config: {:?}", config);
    Ok(config)
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

// Tauri命令，供前端调用，获取配置
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

// Tauri命令，供前端调用，写入配置
#[tauri::command]
pub fn set_custom_style(custom_style: CustomStyle) -> Result<(), String> {
    debug!("[Config] Received custom_style: {:?}", custom_style); // 打印接收到的参数
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
