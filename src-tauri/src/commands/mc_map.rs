use crate::core::minecraft::map::{list_minecraft_worlds, McMapInfo};

#[tauri::command]
pub async fn list_minecraft_worlds_cmd() -> Result<Vec<McMapInfo>, String> {
    list_minecraft_worlds(0).await.map_err(|e| e.to_string())
}
