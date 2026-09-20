// src-tauri/src/commands/assets.rs
use crate::core::minecraft::import::{
    ImportCheckResult, PackagePreview, check_import_file, import_files_batch,
    import_files_batch_cancellable, inspect_archive,
};
use crate::core::minecraft::paths::{BuildType, Edition, GamePathOptions, resolve_target_parent};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use tracing::{debug, error};

#[derive(Debug, Deserialize, Clone)]
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

fn resolve_delete_asset_path(payload: &DeleteAssetPayload) -> Result<PathBuf, String> {
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
        build_type: payload.build_type.clone(),
        edition: payload.edition.clone(),
        version_name: payload.version_name.clone(),
        enable_isolation: payload.enable_isolation,
        user_id: payload.user_id.clone(),
        allow_shared_fallback: false,
    };

    let is_shared_preferred = matches!(
        payload.delete_type.as_str(),
        "resourcePacks" | "behaviorPacks" | "skins"
    );
    let parent_dir = resolve_target_parent(&options, dir_name, is_shared_preferred)
        .ok_or_else(|| "Could not resolve target directory".to_string())?;
    Ok(parent_dir.join(&payload.name))
}

fn delete_task_token(task_id: &str) -> String {
    let mut token = String::with_capacity(task_id.len().min(64));
    for ch in task_id.chars().take(64) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            token.push(ch);
        } else {
            token.push('_');
        }
    }
    if token.is_empty() {
        "task".to_string()
    } else {
        token
    }
}

fn rollback_asset_deletions(staged: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    let mut errors = Vec::new();
    for (original, tombstone) in staged.iter().rev() {
        if !tombstone.exists() {
            continue;
        }
        if original.exists() {
            errors.push(format!("无法回滚 {}：原路径已重新出现", original.display()));
            continue;
        }
        if let Err(error) = fs::rename(tombstone, original) {
            errors.push(format!("恢复 {} 失败: {error}", original.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

enum DeleteAssetsOutcome {
    Completed(String),
    Cancelled(String),
}

fn delete_assets_transactionally(
    payloads: Vec<DeleteAssetPayload>,
    task_id: &str,
) -> Result<DeleteAssetsOutcome, String> {
    if crate::tasks::task_manager::is_cancelled(task_id) {
        return Ok(DeleteAssetsOutcome::Cancelled("资源删除已取消".to_string()));
    }
    let mut targets = Vec::with_capacity(payloads.len());
    for payload in &payloads {
        let target = resolve_delete_asset_path(payload)?;
        if target.exists() && !targets.contains(&target) {
            targets.push(target);
        }
    }
    if targets.is_empty() {
        return Ok(DeleteAssetsOutcome::Completed("所选资源已不存在".to_string()));
    }

    let token = delete_task_token(task_id);
    let mut planned = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        let parent = target
            .parent()
            .ok_or_else(|| format!("资源目录没有父目录: {}", target.display()))?;
        let name = target
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("asset");
        let tombstone = parent.join(format!(".{name}.bmcb-delete-{token}-{index}"));
        if tombstone.exists() {
            return Err(format!("资源删除暂存目录已存在: {}", tombstone.display()));
        }
        planned.push((target.clone(), tombstone));
    }

    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(planned.len());
    for (target, tombstone) in planned {
        if crate::tasks::task_manager::is_cancelled(task_id) {
            let cancelled = "资源删除已取消".to_string();
            if let Err(rollback_error) = rollback_asset_deletions(&staged) {
                return Err(format!("{cancelled}；{rollback_error}"));
            }
            return Ok(DeleteAssetsOutcome::Cancelled(cancelled));
        }

        if let Err(error) = fs::rename(&target, &tombstone) {
            let rollback_error = rollback_asset_deletions(&staged).err();
            return Err(match rollback_error {
                Some(rollback_error) => format!(
                    "暂存资源删除失败 {}: {error}；{rollback_error}",
                    target.display()
                ),
                None => format!("暂存资源删除失败 {}: {error}", target.display()),
            });
        }
        staged.push((target, tombstone));
    }

    // All selected resources are now atomically absent from their canonical locations.
    // Complete cleanup even when cancellation arrives after this commit boundary.
    for (_, tombstone) in &staged {
        fs::remove_dir_all(tombstone).map_err(|error| {
            format!("清理资源删除暂存目录失败 {}: {error}", tombstone.display())
        })?;
    }
    Ok(DeleteAssetsOutcome::Completed(format!(
        "已删除 {} 个资源",
        staged.len()
    )))
}

pub fn start_delete_game_assets_task(
    payloads: Vec<DeleteAssetPayload>,
) -> Result<String, String> {
    if payloads.is_empty() {
        return Err("没有选择要删除的资源".to_string());
    }
    let detail = format!("{} 个资源", payloads.len());
    let task_id = crate::tasks::task_manager::create_task_with_details(
        None,
        "删除游戏资源",
        Some(detail),
        "deleting_assets",
        None,
        false,
    );
    crate::tasks::task_manager::register_task_cooperative_cancel(task_id.clone());

    let worker_task_id = task_id.clone();
    let blocking_task_id = task_id.clone();
    let workflow = crate::tasks::runtime::spawn_io(async move {
        let result = crate::tasks::runtime::run_io_blocking(move || {
            delete_assets_transactionally(payloads, &blocking_task_id)
        })
        .await;

        match result {
            Ok(Ok(DeleteAssetsOutcome::Completed(message))) => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "completed",
                    Some(message),
                );
            }
            Ok(Ok(DeleteAssetsOutcome::Cancelled(message))) => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "cancelled",
                    Some(message),
                );
            }
            Ok(Err(error)) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                "error",
                Some(error),
            ),
            Err(error) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                "error",
                Some(error),
            ),
        }
    })
    .map_err(|error| {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        error
    })?;

    let monitor_task_id = task_id.clone();
    if let Err(error) = crate::tasks::runtime::spawn_io(async move {
        if let Err(error) = workflow.await {
            crate::tasks::task_manager::finish_task(
                &monitor_task_id,
                "error",
                Some(format!("资源删除任务异常结束: {error}")),
            );
        }
    }) {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        return Err(error);
    }

    Ok(task_id)
}

pub fn start_import_assets_task(
    request: ImportAssetsRequest,
    title: impl Into<String>,
) -> Result<String, String> {
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
    crate::tasks::task_manager::register_task_cooperative_cancel(task_id.clone());
    let worker_task_id = task_id.clone();

    let workflow = match crate::tasks::runtime::spawn_io(async move {
        crate::tasks::task_manager::reset_progress(
            &worker_task_id,
            None,
            Some("installing_assets"),
        );
        let result = import_assets_for_task(request, Some(worker_task_id.clone())).await;

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
            Err(error) if crate::tasks::task_manager::is_cancelled(&worker_task_id) => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "cancelled",
                    Some(error.clone()),
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
    }) {
        Ok(workflow) => workflow,
        Err(error) => {
            crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
            return Err(error);
        }
    };

    let monitor_task_id = task_id.clone();
    if let Err(error) = crate::tasks::runtime::spawn_io(async move {
        if let Err(error) = workflow.await
            && !error.is_cancelled()
            && !crate::tasks::task_manager::is_cancelled(&monitor_task_id)
        {
            crate::tasks::task_manager::finish_task(
                &monitor_task_id,
                "error",
                Some(format!("资源导入任务异常结束: {error}")),
            );
        }
    }) {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        return Err(error);
    }

    Ok(task_id)
}

pub async fn import_assets(request: ImportAssetsRequest) -> Result<ImportAssetsResult, String> {
    import_assets_for_task(request, None).await
}

pub(crate) async fn import_assets_for_task(
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
