use std::path::Path;
use std::path::PathBuf;
use tauri::command;
use tokio::fs;

use crate::core::version::version_manager::get_appx_version_list;

#[command]
pub async fn get_version_list() -> Result<serde_json::Value, String> {
    let path = Path::new("./BMCBL/versions");
    // 直接使用固定路径，不再依赖参数
    match path.to_str() {
        Some(folder_str) => Ok(get_appx_version_list(folder_str).await),
        None => Err("路径无效".into()),
    }
}

#[command]
pub async fn delete_version(folder_name: String) -> Result<String, String> {
    // version dir: ./BMCBL/versions/<folder_name>
    let version_dir = PathBuf::from("./BMCBL/versions").join(&folder_name);
    if !version_dir.exists() {
        return Err(format!("版本目录不存在: {}", version_dir.display()));
    }

    // remove dir recursively
    match fs::remove_dir_all(&version_dir).await {
        Ok(_) => Ok(format!("删除成功: {}", folder_name)),
        Err(e) => Err(format!("删除版本 {} 失败: {}", folder_name, e)),
    }
}
