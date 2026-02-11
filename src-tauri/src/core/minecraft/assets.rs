// src-tauri/src/commands/assets.rs
use serde::{Deserialize};
use serde_json::json;
use std::fs;
use tauri::command;
use crate::core::minecraft::paths::{GamePathOptions, BuildType, Edition, resolve_target_parent};
use crate::core::minecraft::import::{import_files_batch, inspect_archive, check_import_file, PackagePreview, ImportCheckResult}; // 引入新模块

#[derive(Debug, Deserialize)]
pub struct DeleteAssetPayload {
    pub build_type: BuildType,
    pub edition: Edition,
    pub version_name: String,
    pub enable_isolation: bool,
    pub user_id: Option<String>,
    pub delete_type: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct ImportAssetPayload {
    pub build_type: BuildType,
    pub edition: Edition,
    pub version_name: String,
    pub enable_isolation: bool,
    pub user_id: Option<String>,
    pub file_paths: Vec<String>,
    pub overwrite: bool, // [新增] 覆盖选项
    pub allow_shared_fallback: bool, // [新增] 允许回退到 Shared
}

#[derive(Debug, Deserialize)]
pub struct CheckImportPayload {
    pub build_type: BuildType,
    pub edition: Edition,
    pub version_name: String,
    pub enable_isolation: bool,
    pub user_id: Option<String>,
    pub file_path: String,
    pub allow_shared_fallback: bool, // [新增] 允许回退到 Shared
}

fn map_delete_type_to_dir(delete_type: &str) -> Option<&'static str> {
    match delete_type {
        "maps" => Some("minecraftWorlds"),
        "mapTemplates" => Some("world_templates"),
        "skins" => Some("skin_packs"),
        "behaviorPacks" => Some("behavior_packs"),
        "resourcePacks" => Some("resource_packs"),
        _ => None,
    }
}

#[command]
pub fn delete_game_asset(payload: DeleteAssetPayload) -> Result<serde_json::Value, String> {
    if payload.name.is_empty() || payload.name.contains("..") || payload.name.contains('/') || payload.name.contains('\\') {
        return Err("Invalid name".into());
    }

    let dir_name = map_delete_type_to_dir(&payload.delete_type)
        .ok_or_else(|| "unsupported delete_type".to_string())?;

    let options = GamePathOptions {
        build_type: payload.build_type,
        edition: payload.edition,
        version_name: payload.version_name,
        enable_isolation: payload.enable_isolation,
        user_id: payload.user_id,
        allow_shared_fallback: false,
    };

    let is_shared_preferred = payload.delete_type == "resourcePacks"
        || payload.delete_type == "behaviorPacks";

    let parent_dir = resolve_target_parent(&options, dir_name, is_shared_preferred)
        .ok_or_else(|| "Could not resolve target directory".to_string())?;

    let target_path = parent_dir.join(&payload.name);

    if !target_path.exists() {
        return Ok(json!({ "success": false, "message": format!("Path not found: {}", target_path.display()) }));
    }

    fs::remove_dir_all(&target_path).map_err(|e| format!("Delete failed: {}", e))?;

    Ok(json!({ "success": true }))
}

// [新增] 导入资源命令
#[command]
pub async fn import_assets(payload: ImportAssetPayload) -> Result<serde_json::Value, String> {
    let options = GamePathOptions {
        build_type: payload.build_type,
        edition: payload.edition,
        version_name: payload.version_name,
        enable_isolation: payload.enable_isolation,
        user_id: payload.user_id,
        allow_shared_fallback: payload.allow_shared_fallback,
    };

    // 在 Blocking Thread 中执行解压和 IO 操作
    let result = tokio::task::spawn_blocking(move || {
        import_files_batch(payload.file_paths, &options, payload.overwrite)
    })
        .await
        .map_err(|e| format!("Task failed: {:?}", e))?
        .map_err(|e| format!("Import failed: {:?}", e))?;

    let (success, fail) = result;
    Ok(json!({
        "success": true,
        "imported_count": success,
        "failed_count": fail
    }))
}

#[command]
pub async fn inspect_import_file(file_path: String, lang: Option<String>) -> Result<PackagePreview, String> {
    let path = std::path::PathBuf::from(file_path);
    if !path.exists() {
        return Err("文件不存在".to_string());
    }

    // 在 blocking thread 中执行，因为涉及 ZIP 解压读取
    tokio::task::spawn_blocking(move || {
        inspect_archive(&path, lang.as_deref()).map_err(|e| e.to_string())
    })
        .await
        .map_err(|e| format!("Task failed: {:?}", e))?
}

// [新增] 检查导入冲突命令
#[command]
pub async fn check_import_conflict(payload: CheckImportPayload) -> Result<ImportCheckResult, String> {
    let options = GamePathOptions {
        build_type: payload.build_type,
        edition: payload.edition,
        version_name: payload.version_name,
        enable_isolation: payload.enable_isolation,
        user_id: payload.user_id,
        allow_shared_fallback: payload.allow_shared_fallback,
    };

    let path = std::path::PathBuf::from(payload.file_path);
    if !path.exists() {
        return Err("文件不存在".to_string());
    }

    tokio::task::spawn_blocking(move || {
        check_import_file(&path, &options).map_err(|e| e.to_string())
    })
        .await
        .map_err(|e| format!("Task failed: {:?}", e))?
}
