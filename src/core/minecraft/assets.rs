// src-tauri/src/commands/assets.rs
use crate::core::minecraft::import::{
    ImportCheckResult, PackagePreview, check_import_file, import_files_batch,
    import_files_batch_cancellable, inspect_archive,
};
use crate::core::minecraft::paths::{BuildType, Edition, GamePathOptions, resolve_target_parent};
use futures_util::stream::{self, StreamExt as _};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;
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

#[derive(Debug)]
pub(crate) struct ImportFileInspection {
    pub(crate) path: PathBuf,
    pub(crate) result: Result<PackagePreview, String>,
}

#[derive(Default)]
struct ImportInspectionResultStore {
    order: VecDeque<String>,
    results: HashMap<String, Vec<ImportFileInspection>>,
}

const IMPORT_INSPECTION_RESULT_LIMIT: usize = 16;

static IMPORT_INSPECTION_RESULTS: Lazy<Mutex<ImportInspectionResultStore>> =
    Lazy::new(|| Mutex::new(ImportInspectionResultStore::default()));

fn store_import_inspection_result(task_id: String, result: Vec<ImportFileInspection>) {
    let evicted = {
        let mut store = IMPORT_INSPECTION_RESULTS.lock().unwrap();
        store.order.retain(|existing| existing != &task_id);
        store.order.push_back(task_id.clone());
        store.results.insert(task_id, result);

        let mut evicted = Vec::new();
        while store.order.len() > IMPORT_INSPECTION_RESULT_LIMIT {
            if let Some(oldest) = store.order.pop_front() {
                store.results.remove(&oldest);
                evicted.push(oldest);
            }
        }
        evicted
    };

    for task_id in evicted {
        let _ = crate::tasks::task_manager::remove_task(&task_id);
    }
}

async fn inspect_import_path(
    path: PathBuf,
    lang: Option<String>,
) -> Result<PackagePreview, String> {
    let path_for_log = path.display().to_string();
    let started_at = Instant::now();

    // ZIP directory traversal, manifest/icon reads and compound fallback extraction are
    // archive work. Keep them off the general IO pool and GPUI foreground executor.
    let result = crate::tasks::runtime::run_archive_blocking(move || {
        if !path.exists() {
            return Err("文件不存在".to_string());
        }
        inspect_archive(&path, lang.as_deref()).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| {
        error!(
            "Inspect import file task failed: path={}, error={error:?}",
            path_for_log
        );
        format!("Task failed: {error:?}")
    })?;

    debug!(
        "Inspect import file completed: path={}, elapsed_ms={}",
        path_for_log,
        started_at.elapsed().as_millis()
    );
    result
}

pub async fn inspect_import_file(
    file_path: String,
    lang: Option<String>,
) -> Result<PackagePreview, String> {
    let path = PathBuf::from(file_path);
    debug!(
        "Inspect import file request: path={}, lang={}",
        path.display(),
        lang.as_deref().unwrap_or("default")
    );
    inspect_import_path(path, lang).await
}

/// Starts one TaskManager-owned batch inspection for Bedrock import archives.
///
/// The returned task id is the only lifecycle handle required by UI. ZIP work runs on the
/// application archive blocking pool with bounded file-level concurrency. TaskManager owns
/// cancellation, progress, logs and terminal state. Completed structured results are retrieved
/// exactly once with take_import_inspection_result.
pub(crate) fn start_import_inspection_task(
    file_paths: Vec<PathBuf>,
    lang: Option<String>,
) -> Result<String, String> {
    if file_paths.is_empty() {
        return Err("没有可解析的导入文件".to_string());
    }

    let total = file_paths.len();
    let task_id = crate::tasks::task_manager::create_task_with_details(
        None,
        "解析导入文件",
        Some(format!("{total} 个文件")),
        "queued",
        None,
        false,
    );
    let worker_task_id = task_id.clone();

    let spawn_result = crate::tasks::runtime::spawn_archive_task(task_id.clone(), async move {
        use crate::tasks::task_manager as task_manager;

        let paths_for_metadata = file_paths.clone();
        let sizes = match crate::tasks::runtime::run_io_blocking(move || {
            paths_for_metadata
                .iter()
                .map(|path| std::fs::metadata(path).map(|metadata| metadata.len()).unwrap_or(0))
                .collect::<Vec<_>>()
        })
        .await
        {
            Ok(sizes) => sizes,
            Err(error) => {
                task_manager::finish_task(
                    &worker_task_id,
                    "error",
                    Some(format!("读取导入文件大小失败：{error}")),
                );
                return;
            }
        };
        let total_bytes = sizes.iter().copied().fold(0u64, u64::saturating_add);

        task_manager::reset_progress(
            &worker_task_id,
            (total_bytes > 0).then_some(total_bytes),
            Some("inspecting_imports"),
        );
        task_manager::append_task_log(
            &worker_task_id,
            format!("开始并发解析 {total} 个导入文件"),
        );

        let concurrency = crate::tasks::runtime::archive_inspection_parallelism()
            .min(total)
            .max(1);
        task_manager::set_task_message(
            &worker_task_id,
            Some(format!("并发解析线程：{concurrency}")),
        );
        task_manager::set_task_visualization(
            &worker_task_id,
            Some(crate::tasks::task_manager::TaskVisualization {
                worker_total: u32::try_from(concurrency).ok(),
                unit_label: Some("files".to_string()),
                unit_total: u64::try_from(total).ok(),
                ..Default::default()
            }),
        );

        let mut inspections = stream::iter(file_paths.into_iter().enumerate())
            .map(|(index, path)| {
                let lang = lang.clone();
                let size = sizes.get(index).copied().unwrap_or(0);
                async move {
                    let result = inspect_import_path(path.clone(), lang).await;
                    (index, path, size, result)
                }
            })
            .buffer_unordered(concurrency);

        let cancel_control = task_manager::task_control(&worker_task_id);
        let mut completed = Vec::with_capacity(total);
        let mut failed = 0usize;
        let mut completed_files = 0usize;

        loop {
            let next = if let Some(control) = cancel_control.as_ref() {
                tokio::select! {
                    _ = control.wait_cancelled() => {
                        task_manager::finish_task(
                            &worker_task_id,
                            "cancelled",
                            Some("导入包解析已取消".to_string()),
                        );
                        return;
                    }
                    next = inspections.next() => next,
                }
            } else {
                inspections.next().await
            };

            let Some((index, path, size, result)) = next else {
                break;
            };
            completed_files = completed_files.saturating_add(1);

            if result.is_err() {
                failed = failed.saturating_add(1);
                task_manager::append_task_log(
                    &worker_task_id,
                    format!("解析失败：{}", path.display()),
                );
            } else {
                task_manager::append_task_log(
                    &worker_task_id,
                    format!("解析完成：{}", path.display()),
                );
            }

            completed.push((
                index,
                ImportFileInspection {
                    path,
                    result,
                },
            ));
            task_manager::update_progress(
                &worker_task_id,
                size,
                (total_bytes > 0).then_some(total_bytes),
                Some("inspecting_imports"),
            );
            task_manager::set_task_message(
                &worker_task_id,
                Some(format!(
                    "已解析 {completed_files}/{total}，失败 {failed}"
                )),
            );
            task_manager::set_task_visualization(
                &worker_task_id,
                Some(crate::tasks::task_manager::TaskVisualization {
                    worker_total: u32::try_from(concurrency).ok(),
                    unit_label: Some("files".to_string()),
                    unit_total: u64::try_from(total).ok(),
                    unit_done: u64::try_from(completed_files).ok(),
                    ..Default::default()
                }),
            );
        }

        if task_manager::is_cancelled(&worker_task_id) {
            task_manager::finish_task(
                &worker_task_id,
                "cancelled",
                Some("导入包解析已取消".to_string()),
            );
            return;
        }

        completed.sort_unstable_by_key(|(index, _)| *index);
        let result = completed
            .into_iter()
            .map(|(_, inspection)| inspection)
            .collect::<Vec<_>>();
        store_import_inspection_result(worker_task_id.clone(), result);

        task_manager::finish_task(
            &worker_task_id,
            "completed",
            Some(format!("解析完成：{total} 个文件，{failed} 个失败")),
        );
    });

    if let Err(error) = spawn_result {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        return Err(error);
    }

    Ok(task_id)
}

/// Takes the completed structured result for an import-inspection task.
///
/// Results are single-consumer and removed from the bounded core-side store after this call.
/// The corresponding finished TaskManager entry is removed as part of the handoff.
pub(crate) fn take_import_inspection_result(
    task_id: &str,
) -> Option<Vec<ImportFileInspection>> {
    let result = {
        let mut store = IMPORT_INSPECTION_RESULTS.lock().unwrap();
        store.order.retain(|existing| existing != task_id);
        store.results.remove(task_id)
    };
    let _ = crate::tasks::task_manager::remove_task(task_id);
    result
}

/// Discards a stored result for a stale terminal import-inspection task.
pub(crate) fn discard_import_inspection_result(task_id: &str) {
    let is_terminal = crate::tasks::task_manager::get_snapshot_arc(task_id)
        .is_some_and(|snapshot| snapshot.is_terminal());
    if !is_terminal {
        return;
    }

    {
        let mut store = IMPORT_INSPECTION_RESULTS.lock().unwrap();
        store.order.retain(|existing| existing != task_id);
        store.results.remove(task_id);
    }
    let _ = crate::tasks::task_manager::remove_task(task_id);
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
