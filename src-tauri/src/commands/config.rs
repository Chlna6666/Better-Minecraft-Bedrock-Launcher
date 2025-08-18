use tauri_plugin_store::JsonValue;
use crate::config::config::{get_nested_value, read_config, set_nested_value, write_config};

#[tauri::command]
pub fn get_config(key: Option<String>) -> Result<JsonValue, String> {
    let config = read_config().map_err(|e| format!("Failed to read config: {}", e))?;
    let config_json = serde_json::to_value(config).map_err(|e| e.to_string())?;

    if let Some(k) = key {
        get_nested_value(&config_json, &k)
            .ok_or_else(|| format!("Key '{}' not found", k))
    } else {
        Ok(config_json)
    }
}

#[tauri::command]
pub fn set_config(key: Option<String>, value: Option<JsonValue>, config: Option<JsonValue>) -> Result<(), String> {
    let mut current = read_config().map_err(|e| format!("Failed to read config: {}", e))?;
    let mut config_json = serde_json::to_value(&current).map_err(|e| e.to_string())?;

    if let Some(cfg) = config {
        // 整个配置替换
        config_json = cfg;
    } else if let (Some(k), Some(v)) = (key, value) {
        set_nested_value(&mut config_json, &k, v)
            .map_err(|e| format!("Failed to set value: {}", e))?;
    } else {
        return Err("Either full config or key+value required".to_string());
    }

    current = serde_json::from_value(config_json).map_err(|e| format!("Failed to deserialize: {}", e))?;
    write_config(&current).map_err(|e| format!("Failed to write config: {}", e))?;

    Ok(())
}