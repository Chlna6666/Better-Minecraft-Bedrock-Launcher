use crate::tasks::TaskStage;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tracing::{info, warn};

use crate::tasks::task_manager::{
    self, ProgressUpdate, TaskFinishStatus, TaskOptions, TaskScope, TaskSnapshot, TaskType,
};

static NEXT_GAME_INSTALL_ID: AtomicU64 = AtomicU64::new(1);
const INSTALL_PROGRESS_TOTAL: u64 = 10_000;

#[derive(Clone, Debug)]
pub enum GamePackageSource {
    Appx { package_id: String },
    Gdk { url: String },
}

#[derive(Clone, Debug)]
pub struct GameInstallRequest {
    pub package_key: String,
    pub version_label: String,
    pub file_name: String,
    pub install_folder: String,
    pub md5: Option<String>,
    pub force_download: bool,
    pub levilamina_version: Option<String>,
    pub source: GamePackageSource,
}

pub fn start(request: GameInstallRequest) -> Result<Arc<str>, String> {
    let operation_id = Arc::<str>::from(format!(
        "game-install-{}",
        NEXT_GAME_INSTALL_ID.fetch_add(1, Ordering::Relaxed)
    ));
    task_manager::create_task(
        TaskOptions::new(
            t!("TaskTitle.install_minecraft").to_string(),
            TaskStage::PREPARING_GAME_INSTALL,
        )
        .id(operation_id.to_string())
        .detail(request.version_label.clone())
        .total(INSTALL_PROGRESS_TOTAL)
        .pausable(true)
        .kind(TaskType::Workflow),
    )?;
    let scope = TaskScope::new(&operation_id)?;
    task_manager::set_task_message(&operation_id, Some("正在准备安装".to_string()));
    task_manager::register_cooperative_cancel(operation_id.to_string());

    let active_child_task = Arc::new(Mutex::new(None::<String>));
    let cancel_child = Arc::clone(&active_child_task);
    task_manager::register_cancel_hook(operation_id.to_string(), move || {
        let child = cancel_child.lock().ok().and_then(|guard| guard.clone());
        if let Some(child) = child {
            task_manager::cancel_task(&child);
        }
    });

    let worker_operation_id = Arc::clone(&operation_id);
    let worker_child_task = Arc::clone(&active_child_task);
    let workflow = match scope.spawn_io(async move {
        let outcome = run(&request, &worker_operation_id, &worker_child_task).await;
        match outcome {
            Ok(local_path) => {
                // A successful return means every child transaction crossed its commit point.
                // A late cancellation must not rewrite a fully committed install as incomplete.
                task_manager::finish_task(
                    &worker_operation_id,
                    TaskFinishStatus::Completed,
                    Some(local_path),
                );
                info!(
                    operation_id = %worker_operation_id,
                    "game install completed; invalidating local version catalog"
                );
                crate::core::version::catalog_events::notify_local_versions_changed();
            }
            Err(message) => {
                warn!(
                    operation_id = %worker_operation_id,
                    %message,
                    "game install workflow failed"
                );
                if task_manager::is_cancelled(&worker_operation_id) {
                    task_manager::finish_task(
                        &worker_operation_id,
                        TaskFinishStatus::Cancelled,
                        Some(message),
                    );
                } else {
                    task_manager::finish_task(
                        &worker_operation_id,
                        TaskFinishStatus::Error,
                        Some(message),
                    );
                }
            }
        }
    }) {
        Ok(workflow) => workflow,
        Err(error) => {
            task_manager::finish_task(&operation_id, TaskFinishStatus::Error, Some(error.clone()));
            return Err(error);
        }
    };
    let monitor_operation_id = Arc::clone(&operation_id);
    crate::tasks::runtime::spawn_io(async move {
        if let Err(error) = workflow.await
            && !error.is_cancelled()
        {
            let message = format!("安装工作流异常结束: {error}");
            if !task_manager::is_cancelled(&monitor_operation_id) {
                task_manager::finish_task(
                    &monitor_operation_id,
                    TaskFinishStatus::Error,
                    Some(message),
                );
            }
        }
    })?;

    Ok(operation_id)
}
async fn run(
    request: &GameInstallRequest,
    operation_id: &str,
    active_child_task: &Arc<Mutex<Option<String>>>,
) -> Result<String, String> {
    info!(
        operation_id,
        package_key = request.package_key,
        file_name = request.file_name,
        "game install workflow started"
    );
    let local_path = if request.force_download {
        None
    } else {
        crate::downloads::api::local_download_path(request.file_name.clone(), request.md5.clone())
            .await?
    };

    let download_weight = if request.levilamina_version.is_some() {
        6_500
    } else {
        7_500
    };
    let extract_weight = 2_500;

    // Start the LeviLamina task before the game download so its package
    // downloads overlap with the game package transfer. Installation into the
    // game directory is held behind `game_ready` until extraction completes.
    let (game_ready_tx, game_ready_rx) = tokio::sync::watch::channel(false);
    let levilamina_handle = match &request.levilamina_version {
        Some(loader_version) => {
            let game_directory =
                crate::utils::file_ops::bmcbl_subdir("versions").join(&request.install_folder);
            Some(
                crate::core::levilamina::start_install_under_parent_when_game_ready(
                    crate::core::levilamina::LeviLaminaInstallRequest::Loader {
                        game_directory,
                        game_version: request.version_label.clone(),
                        loader_version: loader_version.clone(),
                    },
                    operation_id,
                    game_ready_rx,
                )?,
            )
        }
        None => None,
    };

    let package_path = match local_path {
        Some(path) => {
            set_game_install_progress(operation_id, download_weight, TaskStage::DOWNLOADING_GAME);
            path
        }
        None => download_package(request, operation_id, active_child_task, download_weight).await?,
    };
    check_cancelled(operation_id)?;
    wait_workflow_active(operation_id).await?;
    let extract_task_id = start_extract(request, &package_path).await?;
    // Archive task inherits the game workflow scope before publishing its first event.
    set_active_child_task(active_child_task, Some(extract_task_id.clone()));
    set_game_install_stage(
        operation_id,
        TaskStage::INSTALLING_GAME,
        "正在解压并安装游戏".to_string(),
    );

    let extract_snapshot = wait_for_child_progress(
        &extract_task_id,
        operation_id,
        download_weight,
        extract_weight,
        TaskStage::INSTALLING_GAME,
    )
    .await?;
    set_active_child_task(active_child_task, None);
    require_completed(&extract_snapshot, "安装")?;
    check_cancelled(operation_id)?;
    wait_workflow_active(operation_id).await?;
    // The game directory now exists: release the LeviLamina install gate.
    let _ = game_ready_tx.send(true);
    if let Some(handle) = levilamina_handle {
        set_game_install_stage(
            operation_id,
            TaskStage::INSTALLING_LEVILAMINA,
            "正在安装 LeviLamina".to_string(),
        );
        set_active_child_task(active_child_task, Some(handle.task_id.to_string()));
        let loader_snapshot = wait_for_child_progress(
            handle.task_id.as_ref(),
            operation_id,
            download_weight + extract_weight,
            INSTALL_PROGRESS_TOTAL - download_weight - extract_weight,
            TaskStage::INSTALLING_LEVILAMINA,
        )
        .await?;
        set_active_child_task(active_child_task, None);
        require_completed(&loader_snapshot, "LeviLamina 安装")?;
    }
    info!(
        operation_id,
        extract_task_id, "game install workflow completed"
    );
    Ok(package_path)
}

async fn download_package(
    request: &GameInstallRequest,
    operation_id: &str,
    active_child_task: &Arc<Mutex<Option<String>>>,
    download_weight: u64,
) -> Result<String, String> {
    set_game_install_stage(
        operation_id,
        TaskStage::DOWNLOADING_GAME,
        "正在下载游戏包".to_string(),
    );
    let task_id = match &request.source {
        GamePackageSource::Appx { package_id } => {
            crate::downloads::api::download_appx(
                package_id.clone(),
                request.file_name.clone(),
                request.md5.clone(),
                Some(request.force_download),
                None,
            )
            .await?
        }
        GamePackageSource::Gdk { url } => {
            crate::downloads::api::download_game_resource(
                url.clone(),
                request.file_name.clone(),
                request.md5.clone(),
                Some(request.force_download),
                None,
            )
            .await?
        }
    };

    if !task_manager::set_task_labels(
        &task_id,
        request.file_name.as_str(),
        Some(request.version_label.clone()),
    ) {
        warn!(
            task_id,
            "download task disappeared before labels were applied"
        );
    }
    // Download task is created inside the game workflow scope.
    set_active_child_task(active_child_task, Some(task_id.clone()));

    let snapshot = wait_for_child_progress(
        &task_id,
        operation_id,
        0,
        download_weight,
        TaskStage::DOWNLOADING_GAME,
    )
    .await?;
    set_active_child_task(active_child_task, None);
    require_completed(&snapshot, "下载")?;
    snapshot
        .message
        .as_ref()
        .map(ToString::to_string)
        .ok_or_else(|| "下载完成但任务没有返回文件路径".to_string())
}

async fn start_extract(request: &GameInstallRequest, package_path: &str) -> Result<String, String> {
    match request.source {
        GamePackageSource::Gdk { .. } => {
            crate::core::minecraft::gdk::unpack::start_unpack_gdk_task(
                package_path,
                &request.install_folder,
            )
        }
        GamePackageSource::Appx { .. } => {
            crate::archive::api::extract_zip_appx(
                format!("{}.appx", request.install_folder),
                package_path.to_string(),
                crate::archive::api::AppxExtractOptions {
                    conflict_policy: crate::archive::zip::ZipConflictPolicy::Replace,
                    remove_signature: true,
                },
            )
            .await
        }
    }
}

fn set_active_child_task(active_child_task: &Arc<Mutex<Option<String>>>, task_id: Option<String>) {
    if let Ok(mut current) = active_child_task.lock() {
        *current = task_id;
    }
}

fn require_completed(snapshot: &TaskSnapshot, operation: &str) -> Result<(), String> {
    if snapshot.status.as_ref() == "completed" {
        return Ok(());
    }
    // A cancelled child means the user stopped the workflow, not that it broke:
    // report a cancellation message so the finish path records `cancelled`.
    if snapshot.status.as_ref() == "cancelled" {
        return Err("安装已取消".to_string());
    }
    Err(format!(
        "{operation}{}: {}",
        snapshot.status,
        snapshot.message.as_deref().unwrap_or("没有错误详情")
    ))
}

fn set_game_install_stage(operation_id: &str, stage: TaskStage, message: String) {
    // Cumulative workflow progress must survive every phase transition.
    task_manager::update_progress(
        operation_id,
        ProgressUpdate::advance(0)
            .total(INSTALL_PROGRESS_TOTAL)
            .stage(stage.clone()),
    );
    task_manager::set_task_message(operation_id, Some(message));
}

fn set_game_install_progress(operation_id: &str, target: u64, stage: TaskStage) {
    if let Some(current) = task_manager::snapshot_arc(operation_id) {
        let delta = target
            .min(INSTALL_PROGRESS_TOTAL)
            .saturating_sub(current.done);
        task_manager::update_progress(
            operation_id,
            ProgressUpdate::advance(delta)
                .total(INSTALL_PROGRESS_TOTAL)
                .stage(stage.clone()),
        );
    }
}

fn check_cancelled(operation_id: &str) -> Result<(), String> {
    if task_manager::is_cancelled(operation_id) {
        Err("安装已取消".to_string())
    } else {
        Ok(())
    }
}

/// Waits until the workflow is no longer paused. Used at stage boundaries so a
/// pause taken between two phases holds before the next phase starts.
async fn wait_workflow_active(operation_id: &str) -> Result<(), String> {
    if let Some(control) = task_manager::task_control(operation_id) {
        if !task_manager::wait_until_active(&control).await {
            return Err("安装已取消".to_string());
        }
    }
    Ok(())
}

/// A phase with no known byte total advances only after its child reports success.
async fn wait_for_child_progress(
    child_id: &str,
    operation_id: &str,
    offset: u64,
    weight: u64,
    stage: TaskStage,
) -> Result<Arc<TaskSnapshot>, String> {
    let mut events = task_manager::subscribe_task_events();
    loop {
        // Pause gate: the workflow itself has no worker loop, so park here while
        // the parent is paused. The child (download) is paused by propagation,
        // and its state-change events wake this loop so the gate is reached.
        if let Some(control) = task_manager::task_control(operation_id) {
            // A child created while the workflow was already paused missed the
            // downward propagation; pause it here before parking.
            if task_manager::is_paused(operation_id) {
                task_manager::pause_task(child_id);
            }
            if !task_manager::wait_until_active(&control).await {
                return Err("已取消".to_string());
            }
        }
        let snapshot = task_manager::snapshot_arc(child_id)
            .ok_or_else(|| format!("子任务已不存在: {child_id}"))?;
        let ratio = if snapshot.status.as_ref() == "completed" {
            1.0
        } else {
            snapshot.percent.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0
        };
        set_game_install_progress(
            operation_id,
            offset + (ratio * weight as f64) as u64,
            stage.clone(),
        );
        if snapshot.is_terminal() {
            // A child cancelled on its own (e.g. cancelled from another entry
            // point) must cancel the workflow too: otherwise the parent would
            // report a failed install for what was a user cancellation.
            if snapshot.status.as_ref() == "cancelled" && !task_manager::is_cancelled(operation_id) {
                task_manager::cancel_task(operation_id);
            }
            return Ok(snapshot);
        }
        check_cancelled(operation_id)?;
        match events.recv().await {
            Ok(task_manager::TaskEvent::Removed(id)) if id.as_ref() == child_id => {
                return Err(format!("子任务已移除: {child_id}"));
            }
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return Err("任务事件通道意外关闭".to_string());
            }
        }
    }
}
