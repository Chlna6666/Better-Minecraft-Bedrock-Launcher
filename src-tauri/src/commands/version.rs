use std::path::Path;
use tauri::command;
use crate::core::version::version_manager::get_appx_version_list;

#[command]
pub async fn get_version_list(_file_name: String) -> Result<serde_json::Value, String> {
    let path = Path::new("./BMCBL/versions");
    match path.to_str() {
        Some(folder_str) => Ok(get_appx_version_list(folder_str).await),
        None => Err("路径无效".into()),
    }
}
