// src-tauri/src/commands/assets.rs
use crate::core::minecraft::import::{
    ImportCheckResult, PackagePreview, check_import_file, import_files_batch,
    import_files_batch_cancellable, inspect_archive,
};
use crate::core::minecraft::paths::{BuildType, Edition, GamePathOptions, resolve_target_parent};
use serde::Deserialize;
use serde_json::json;
use std::fs;
use std::sync::Arc;
use tokio::sync::oneshot;
use tracing::{debug, error};

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
pub struct ImportAssetsRequest {
    pub build_type: BuildType,
    pub edition: Edition,
    pub version_name: String,
    pub enable_isolation: bool,
    pub user_id: Option<String>,
    pub file_paths: Vec<String>,
    pub overwrite: bool,             // [新增] 覆盖选项
    pub allow_shared_fallback: bool, // [新增] 允许回退到 Shared
}

#[derive(Debug, Deserialize)]
pub struct CheckImportRequest {
    pub build_type: BuildType,
    pub edition: Edition,
    pub version_name: String,
    pub enable_isolation: bool,
    pub user_id: Option<String>,
    pub file_path: String,
    pub allow_shared_fallback: bool, // [新增] 允许回退到 Shared
}

#[derive(Debug, Clone)]
pub struct ImportAssetsResult {
    pub imported_count: usize,
    pub failed_count: usize,
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

pub fn delete_game_asset(payload: DeleteAssetPayload) -> Result<serde_json::Value, String> {
    if payload.name.is_empty()
        || payload.name.contains("..")
        || payload.name.contains('/')
        || payload.name.contains('\\')
    {
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

    let is_shared_preferred = matches!(
        payload.delete_type.as_str(),
        "resourcePacks" | "behaviorPacks" | "skins"
    );

    let parent_dir = resolve_target_parent(&options, dir_name, is_shared_preferred)
        .ok_or_else(|| "Could not resolve target directory".to_string())?;

    let target_path = parent_dir.join(&payload.name);

    if !target_path.exists() {
        return Ok(
            json!({ "success": false, "message": format!("Path not found: {}", target_path.display()) }),
        );
    }

    fs::remove_dir_all(&target_path).map_err(|e| format!("Delete failed: {}", e))?;

    Ok(json!({ "success": true }))
}

pub struct ImportAssetsTaskHandle {
    pub task_id: Arc<str>,
    pub result: oneshot::Receiver<Result<ImportAssetsResult, String>>,
}

pub fn start_import_assets_task(
    request: ImportAssetsRequest,
    title: impl Into<String>,
) -> Result<ImportAssetsTaskHandle, String> {
    let detail = Some(format!(
        "{} · {} 个文件",
        request.version_name,
        request.file_paths.len()
    ));
    let task_id = crate::tasks::task_manager::create_task_with_details(
        None,
        title,
        detail,
        "queued",
        None,
        false,
    );
    let worker_task_id = task_id.clone();
    let (sender, receiver) = oneshot::channel();

    if let Err(error) = crate::tasks::runtime::spawn_archive_task(task_id.clone(), async move {
        crate::tasks::task_manager::reset_progress(
            &worker_task_id,
            None,
            Some("installing_assets"),
        );
        let result = import_assets_for_task(request, Some(worker_task_id.clone())).await;

        if crate::tasks::task_manager::is_cancelled(&worker_task_id) {
            let _ = sender.send(Err("导入已取消".to_string()));
            return;
        }

        match &result {
            Ok(result) if result.failed_count == 0 && result.imported_count > 0 => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "completed",
                    Some(format!("已导入 {} 个内容包", result.imported_count)),
                );
            }
            Ok(result) => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "error",
                    Some(format!(
                        "导入未完整完成：成功 {}，失败 {}",
                        result.imported_count, result.failed_count
                    )),
                );
            }
            Err(error) => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "error",
                    Some(error.clone()),
                );
            }
        }
        let _ = sender.send(result);
    }) {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        return Err(error);
    }

    Ok(ImportAssetsTaskHandle {
        task_id: Arc::from(task_id),
        result: receiver,
    })
}

pub async fn import_assets(request: ImportAssetsRequest) -> Result<ImportAssetsResult, String> {
    import_assets_for_task(request, None).await
}

async fn import_assets_for_task(
    request: ImportAssetsRequest,
    task_id: Option<String>,
) -> Result<ImportAssetsResult, String> {
    debug!(
        "Import assets request: count={}, build={:?}, edition={:?}, version={}, isolation={}, shared_fallback={}, overwrite={}",
        request.file_paths.len(),
        request.build_type,
        request.edition,
        request.version_name,
        request.enable_isolation,
        request.allow_shared_fallback,
        request.overwrite
    );
    let options = GamePathOptions {
        build_type: request.build_type,
        edition: request.edition,
        version_name: request.version_name,
        enable_isolation: request.enable_isolation,
        user_id: request.user_id,
        allow_shared_fallback: request.allow_shared_fallback,
    };
    let overwrite = request.overwrite;
    let files = request.file_paths;
    let cancel_task_id = task_id.clone();

    let result = crate::tasks::runtime::run_archive_blocking(move || {
        if let Some(task_id) = cancel_task_id {
            import_files_batch_cancellable(files, &options, overwrite, || {
                crate::tasks::task_manager::is_cancelled(&task_id)
            })
        } else {
            import_files_batch(files, &options, overwrite)
        }
    })
    .await
    .map_err(|error| {
        error!("Import assets task failed: {error:?}");
        format!("Task failed: {:?}", error)
    })?
    .map_err(|error| {
        error!("Import assets execution failed: {error:?}");
        format!("Import failed: {:?}", error)
    })?;

    let (success, fail) = result;
    debug!("Import assets result: success={}, fail={}", success, fail);
    Ok(ImportAssetsResult {
        imported_count: success,
        failed_count: fail,
    })
}

pub async fn inspect_import_file(
    file_path: String,
    lang: Option<String>,
) -> Result<PackagePreview, String> {
    let path = std::path::PathBuf::from(file_path);
    if !path.exists() {
        return Err("文件不存在".to_string());
    }

    debug!(
        "Inspect import file request: path={}, lang={}",
        path.display(),
        lang.as_deref().unwrap_or("default")
    );
    let path_for_log = path.display().to_string();

    // 在 blocking thread 中执行，因为涉及 ZIP 解压读取
    crate::tasks::runtime::run_io_blocking(move || {
        inspect_archive(&path, lang.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|error| {
        error!(
            "Inspect import file task failed: path={}, error={error:?}",
            path_for_log
        );
        format!("Task failed: {:?}", error)
    })?
}

// [新增] 检查导入冲突命令
pub async fn check_import_conflict(
    request: CheckImportRequest,
) -> Result<ImportCheckResult, String> {
    debug!(
        "Check import conflict request: path={}, build={:?}, edition={:?}, version={}, isolation={}, shared_fallback={}",
        request.file_path,
        request.build_type,
        request.edition,
        request.version_name,
        request.enable_isolation,
        request.allow_shared_fallback
    );
    let options = GamePathOptions {
        build_type: request.build_type,
        edition: request.edition,
        version_name: request.version_name,
        enable_isolation: request.enable_isolation,
        user_id: request.user_id,
        allow_shared_fallback: request.allow_shared_fallback,
    };

    let path = std::path::PathBuf::from(request.file_path);
    if !path.exists() {
        return Err("文件不存在".to_string());
    }
    let path_for_log = path.display().to_string();

    crate::tasks::runtime::run_io_blocking(move || {
        check_import_file(&path, &options).map_err(|e| e.to_string())
    })
    .await
    .map_err(|error| {
        error!(
            "Check import conflict task failed: path={}, error={error:?}",
            path_for_log
        );
        format!("Task failed: {:?}", error)
    })?
}
