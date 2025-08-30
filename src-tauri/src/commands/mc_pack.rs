use crate::core::minecraft::resource_packs::{read_all_resource_packs,read_all_behavior_packs,McPackInfo };

#[tauri::command]
pub async fn get_all_resource_packs() -> Result<Vec<McPackInfo>, String> {
    read_all_resource_packs()
        .await
        .map_err(|e| format!("读取失败: {:?}", e))
}



#[tauri::command]
pub async fn get_all_behavior_packs() -> Result<Vec<McPackInfo>, String> {
    read_all_behavior_packs()
        .await
        .map_err(|e| format!("读取失败: {:?}", e))
}