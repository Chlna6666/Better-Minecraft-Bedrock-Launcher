use crate::config::config::{read_config, Config};
use crate::utils::system_info::get_system_language;

#[tauri::command]
pub async fn get_locale() -> String {
    let config: Config = read_config().unwrap_or_default();
    match config.launcher.language.as_str() {
        "auto" => get_system_language(),
        "" => "en-US".to_string(),
        other => other.replace('_', "-"),
    }
}
