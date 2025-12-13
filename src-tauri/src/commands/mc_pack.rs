use crate::core::minecraft::resource_packs::{
    read_all_behavior_packs, read_all_resource_packs, McPackInfo,
};

#[tauri::command]
pub async fn get_all_resource_packs(lang: Option<String>) -> Result<Vec<McPackInfo>, String> {
    let lang_ref = lang.as_deref().unwrap_or("en_US");
    read_all_resource_packs(lang_ref)
        .await
        .map_err(|e| format!("读取失败: {:?}", e))
}

#[tauri::command]
pub async fn get_all_behavior_packs(lang: Option<String>) -> Result<Vec<McPackInfo>, String> {
    let lang_ref = lang.as_deref().unwrap_or("en_US");
    read_all_behavior_packs(lang_ref)
        .await
        .map_err(|e| format!("读取失败: {:?}", e))
}
